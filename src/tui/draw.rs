//! The part that paints, and the loop that drives it.
//!
//! Everything it needs to decide is in [`super`], which has no terminal dependency at all
//! and is tested in every build. What is here is ratatui and crossterm and nothing else:
//! set the terminal up, wait for an event, ask the state machine what it means, do that,
//! draw. It is deliberately the thinnest layer in this program.
//!
//! Three rules from the plan are enforced by construction rather than by care:
//!
//! - **The panic hook is installed *before* raw mode.** The release profile keeps
//!   unwinding rather than aborting, so a panic mid-draw must not leave the operator with
//!   a dead terminal and no echo. Installing it afterwards leaves a window where exactly
//!   that happens.
//! - **Redraw on an event, not on a timer.** A 60 fps loop on an ARMv7 NAS burns a core
//!   for a screen nobody is looking at. The poll below has a long timeout, and the only
//!   thing on a clock is the log tail.
//! - **No network IO on the draw path.** `App::on_key` returns an [`Action`]; the loop
//!   performs it between frames. One unreachable BMC cannot freeze a screen because the
//!   screen was never what was waiting.

use super::remote::Remote;
use super::{Action, App, Key, Pane};
use crate::config::Config;
use crate::select::Answers;
use crate::tail::{Filter, Tail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, execute};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap};
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

/// How long to wait for a key before looking at the log again. Seconds, not frames.
const POLL: Duration = Duration::from_millis(500);
/// How often the log tail is re-read while the log screen is open.
const TAIL_INTERVAL: Duration = Duration::from_secs(1);

type Term = Terminal<CrosstermBackend<Stdout>>;

/// What the screens render. Refreshed by [`Action::Reload`], never mid-draw.
struct Model {
    machines: Vec<crate::cli::fleet::Machine>,
    groups: Vec<crate::cli::fleet::Group>,
    problems: Vec<String>,
    store: String,
    /// Set when something was done, so the operator sees the outcome of a keystroke.
    said: Option<String>,
}

/// Where a screen's contents come from.
///
/// Remote mode inherits SQLite-only from the API it speaks to — that is a property of the
/// admin API, not a new decision — and it reads three screens because that is what the API
/// has. The rest say so rather than inventing a read surface.
pub enum Source {
    Local(Answers),
    Remote(Remote),
}

impl Model {
    fn from(source: &Source) -> Model {
        match source {
            Source::Local(answers) => Model::load(answers),
            Source::Remote(remote) => Model {
                machines: remote.machines().unwrap_or_default(),
                groups: remote.groups().unwrap_or_default(),
                problems: remote.problems().unwrap_or_default(),
                store: remote.describe(),
                // A failure here is worth saying out loud: an empty fleet and an
                // unreachable one look identical otherwise.
                said: remote.machines().err(),
            },
        }
    }

    fn load(answers: &Answers) -> Model {
        Model {
            machines: crate::cli::fleet::machines(answers).unwrap_or_default(),
            groups: crate::cli::fleet::groups(answers).unwrap_or_default(),
            problems: answers.problems().unwrap_or_default(),
            store: answers.describe(),
            said: None,
        }
    }

    fn rows(&self, pane: Pane) -> usize {
        match pane {
            Pane::Machines => self.machines.len(),
            Pane::Groups => self.groups.len(),
            Pane::Problems => self.problems.len(),
            _ => 0,
        }
    }

    fn selected_id(&self, app: &App) -> Option<String> {
        match app.pane {
            Pane::Machines => self.machines.get(app.selected).map(|m| m.id.clone()),
            Pane::Groups => self.groups.get(app.selected).map(|g| g.name.clone()),
            _ => None,
        }
    }
}

/// Put the terminal back, whatever happened.
///
/// Called from the panic hook, from every early return, and around the `$EDITOR` suspend —
/// where the editor itself may die.
fn restore() {
    let _ = disable_raw_mode();
    let _ = io::stdout().execute(LeaveAlternateScreen);
    let _ = io::stdout().execute(crossterm::cursor::Show);
}

fn setup() -> io::Result<Term> {
    // **Before raw mode, not after.** Between the two is a window where a panic leaves a
    // terminal with no echo and no line discipline, and the operator has to type `reset`
    // blind.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(out))
}

/// `rescriptum tui` — the whole loop.
pub fn run(cfg: &Config, source: Source) -> io::Result<()> {
    let mut app = App {
        remote: matches!(source, Source::Remote(_)),
        ..App::default()
    };
    let mut model = Model::from(&source);
    // The log is a file this process reads; if there is none, the screen says so rather
    // than looking broken.
    // The log is a file on *this* machine. Over the wire there is none to follow, and the
    // admin API deliberately does not serve one — whether a fleet token should be able to
    // read a server's log is a decision nobody has made.
    let mut log: Result<Tail, String> = if app.remote {
        Err("local only — the admin API serves no log, deliberately".to_string())
    } else {
        match Tail::describe(cfg.log_file.as_deref()) {
            Ok(path) => Tail::open(path, 2000).map_err(|e| format!("{}: {e}", path.display())),
            Err(why) => Err(why),
        }
    };
    let mut last_tail = Instant::now();

    let mut term = setup()?;
    let outcome = loop {
        if let Err(e) = term.draw(|frame| render(frame, &app, &model, &log)) {
            break Err(e);
        }

        // A long poll rather than a frame clock: nothing moves unless something happened.
        match event::poll(POLL) {
            Ok(true) => {}
            Ok(false) => {
                if app.pane == Pane::Logs && last_tail.elapsed() >= TAIL_INTERVAL {
                    if let Ok(tail) = log.as_mut() {
                        let _ = tail.poll();
                    }
                    last_tail = Instant::now();
                }
                continue;
            }
            Err(e) => break Err(e),
        }

        let key = match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => translate(k),
            Ok(_) => continue,
            Err(e) => break Err(e),
        };
        let Some(key) = key else { continue };

        let rows = model.rows(app.pane);
        let selected = model.selected_id(&app);
        let Some(action) = app.on_key(key, rows, selected.as_deref()) else {
            continue;
        };

        model.said = None;
        match action {
            Action::Quit => break Ok(()),
            Action::Reload => model = Model::from(&source),
            Action::Render(id) => {
                model.said = Some(match &source {
                    Source::Local(answers) => rendered(answers, &id),
                    // `GET /resolve/{id}` would do it, but that is a second request shape
                    // and the plan's promise was one endpoint. Named rather than faked.
                    Source::Remote(_) => {
                        format!("{id}: `rescriptum render` shows this, locally")
                    }
                });
            }
            Action::Edit(id) => {
                // Suspending means leaving the alternate screen and giving the terminal
                // back; `restore` is what the editor needs and what a crash in it must not
                // skip.
                let Source::Local(answers) = &source else {
                    model.said = Some("local only".to_string());
                    continue;
                };
                restore();
                let said = edit(cfg, answers, &id);
                term = setup()?;
                let _ = term.clear();
                model = Model::load(answers);
                model.said = Some(said);
            }
            other => {
                model.said = Some(match &source {
                    Source::Local(answers) => perform(cfg, answers, &other),
                    Source::Remote(_) => "local only".to_string(),
                });
            }
        }
    };

    restore();
    outcome
}

/// Everything that is not a redraw and not an edit.
///
/// Slow actions land here, between frames, which is the whole point of `on_key` returning
/// a description rather than doing the work.
fn perform(cfg: &Config, answers: &Answers, action: &Action) -> String {
    let controllers = match cfg.controllers_file.as_deref() {
        Some(p) => match crate::controllers::load(p) {
            Ok(c) => c,
            Err(e) => return e,
        },
        None => {
            return "no controllers: RESCRIPTUM_CONTROLLERS_FILE names a file, and nothing does"
                .to_string();
        }
    };

    // A named alias rather than a closure type written inline: what this is, is "the one
    // thing to do to the controller once it has been found".
    type Deed = Box<dyn Fn(&crate::controllers::Controller) -> String>;

    let (id, run): (&str, Deed) = match action {
        Action::Probe => {
            let all: Vec<&crate::controllers::Controller> = controllers.iter().collect();
            let states = crate::power::probe(&all);
            let on = states
                .iter()
                .filter(|s| matches!(s, Ok(st) if st.state == crate::power::State::On))
                .count();
            let unreachable = states.iter().filter(|s| s.is_err()).count();
            return format!(
                "{} controller(s): {on} on, {unreachable} unreachable",
                all.len()
            );
        }
        Action::PowerOn(id) => (
            id,
            Box::new(|c| match crate::power::on(c) {
                Ok(()) => "power on sent".to_string(),
                Err(e) => e,
            }),
        ),
        Action::PowerOff { id, hard } => {
            let hard = *hard;
            (
                id,
                Box::new(move |c| match crate::power::off(c, hard) {
                    Ok(()) => if hard { "forced off" } else { "shutdown sent" }.to_string(),
                    Err(e) => e,
                }),
            )
        }
        Action::Pxe(id) => (
            id,
            Box::new(|c| match crate::power::pxe(c) {
                Ok(crate::power::Armed::Confirmed) => "one-time network boot armed".to_string(),
                Ok(crate::power::Armed::NotSupported) => {
                    "no boot override here — the server decides".to_string()
                }
                Ok(crate::power::Armed::Ignored) => {
                    "accepted and not applied — this controller reports success without \
                     doing anything"
                        .to_string()
                }
                Err(e) => e,
            }),
        ),
        Action::Arm(id) => {
            let store = match cfg.open_store() {
                Ok(s) => s,
                Err(e) => return e.to_string(),
            };
            return match crate::installed::rearm(store.as_ref(), id) {
                Ok(Some(under)) => format!("{id}: its boot script is back, as {under}"),
                Ok(None) => format!("{id}: nothing archived to put back"),
                Err(e) => format!("{id}: {e}"),
            };
        }
        Action::Disarm(id) => {
            let _ = answers;
            return format!(
                "{id}: disarming by hand is `POST /installed`'s job — a machine reports its \
                 own success"
            );
        }
        _ => return String::new(),
    };

    match controllers.find(id) {
        Some(c) => format!("{}: {}", c.id, run(c)),
        None => format!("{id}: no controller"),
    }
}

fn rendered(answers: &Answers, id: &str) -> String {
    match answers.resolve(&crate::facts::Facts::from_identity(id)) {
        Ok(Some(r)) => format!("{id}: {} ({} bytes)", r.how(), r.body.len()),
        Ok(None) => format!("{id}: nothing applies — the server would answer 404"),
        Err(e) => format!("{id}: {e}"),
    }
}

fn edit(cfg: &Config, answers: &Answers, id: &str) -> String {
    let store = match cfg.open_store() {
        Ok(s) => s,
        Err(e) => return e.to_string(),
    };
    let target = crate::guard::Target::Machine(id.to_string());
    let (format, before) = match store.snapshot() {
        Ok(s) => match s.machines.into_iter().find(|m| m.id == id) {
            Some(m) => (m.format, m.body),
            None => return format!("{id}: no document to edit"),
        },
        Err(e) => return e.to_string(),
    };

    let (editor, note) = crate::edit::editor(std::env::var("EDITOR").ok());
    let outcome =
        crate::edit::round_trip(answers, store.as_ref(), &target, &format, &before, |p| {
            match std::process::Command::new(&editor).arg(p).status() {
                Ok(s) if s.success() => Ok(()),
                Ok(s) => Err(format!("{editor} exited {}", s.code().unwrap_or(-1))),
                Err(e) => Err(format!("cannot run {editor}: {e}")),
            }
        });

    let said = match outcome {
        crate::edit::Edited::Unchanged => format!("{id}: unchanged, nothing written"),
        crate::edit::Edited::Stored(p) if p.is_empty() => format!("{id}: stored"),
        crate::edit::Edited::Stored(p) => format!("{id}: stored, {} problem(s) remain", p.len()),
        crate::edit::Edited::Refused(p) => {
            format!("{id}: refused and rolled back — {}", p.join("; "))
        }
        crate::edit::Edited::Failed(e) => format!("{id}: {e}"),
    };
    match note {
        Some(n) => format!("{said} ({n})"),
        None => said,
    }
}

/// crossterm's keys, reduced to the ones the state machine knows.
fn translate(k: KeyEvent) -> Option<Key> {
    Some(match k.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        // Ctrl-C is what a terminal user's fingers do; it means quit here too.
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => Key::Char('q'),
        KeyCode::Char(c) => Key::Char(c),
        _ => return None,
    })
}

fn render(frame: &mut Frame, app: &App, model: &Model, log: &Result<Tail, String>) {
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    let titles: Vec<&str> = Pane::ORDER.iter().map(|p| p.title()).collect();
    let selected = Pane::ORDER.iter().position(|p| *p == app.pane).unwrap_or(0);
    frame.render_widget(Tabs::new(titles).select(selected).divider(" "), areas[0]);

    let body = areas[1];
    match app.describe_pane(cfg!(feature = "boot")) {
        Some(note) => frame.render_widget(
            Paragraph::new(note)
                .wrap(Wrap { trim: true })
                .block(block(app)),
            body,
        ),
        None => match app.pane {
            Pane::Dashboard => dashboard(frame, body, model),
            Pane::Machines => machines(frame, body, app, model),
            Pane::Groups => groups(frame, body, app, model),
            Pane::Problems => problems(frame, body, app, model),
            Pane::Logs => logs(frame, body, app, log),
            Pane::Media | Pane::Boot => frame.render_widget(
                Paragraph::new(
                    "`rescriptum media list` and `boot check` say more than a pane could",
                )
                .block(block(app)),
                body,
            ),
        },
    }

    let hint = match &app.search {
        Some(text) => format!("/{text}"),
        None => match &model.said {
            Some(said) => said.clone(),
            None => "tab panes · ↑↓ move · enter render · e edit · o on · X force off · \
                     x pxe · s state · / filter · r reload · q quit"
                .to_string(),
        },
    };
    frame.render_widget(Paragraph::new(hint), areas[2]);
}

fn block(app: &App) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(app.pane.title().to_string())
}

fn dashboard(frame: &mut Frame, area: Rect, model: &Model) {
    let armed = model.machines.iter().filter(|m| m.armed).count();
    let by_group = model.machines.iter().filter(|m| m.armed_by_group).count();
    let disarmed = model.machines.iter().filter(|m| m.disarmed).count();

    let mut lines = vec![
        Line::from(model.store.clone()),
        Line::from(""),
        Line::from(format!(
            "{} machine(s), {} group(s)",
            model.machines.len(),
            model.groups.len()
        )),
        Line::from(format!(
            "{armed} armed, {disarmed} disarmed by a previous install"
        )),
    ];
    if by_group > 0 {
        // Its own line, because these reinstall on every network boot and their webhook
        // reports success while doing it.
        lines.push(Line::from(format!(
            "{by_group} armed by a group, which `POST /installed` cannot disarm"
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(if model.problems.is_empty() {
        "no problems".to_string()
    } else {
        format!(
            "{} problem(s) — see the problems pane",
            model.problems.len()
        )
    }));

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("dashboard")),
        area,
    );
}

fn machines(frame: &mut Frame, area: Rect, app: &App, model: &Model) {
    let mut app = app.clone();
    let window = app.visible(model.machines.len(), area.height.saturating_sub(2) as usize);
    let items: Vec<ListItem> = model.machines[window.clone()]
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let marker = if window.start + i == app.selected {
                "> "
            } else {
                "  "
            };
            let armed = match (m.armed, m.armed_by_group, m.disarmed) {
                (true, true, _) => " armed by a group (cannot disarm itself)",
                (true, false, _) => " armed",
                (false, _, true) => " disarmed",
                _ => "",
            };
            let group = m.group.as_deref().unwrap_or("-");
            ListItem::new(format!(
                "{marker}{} [{}] {group}{armed}",
                m.id,
                m.formats.join(",")
            ))
        })
        .collect();
    frame.render_widget(List::new(items).block(block(&app)), area);
}

fn groups(frame: &mut Frame, area: Rect, app: &App, model: &Model) {
    let mut app = app.clone();
    let window = app.visible(model.groups.len(), area.height.saturating_sub(2) as usize);
    let items: Vec<ListItem> = model.groups[window.clone()]
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let marker = if window.start + i == app.selected {
                "> "
            } else {
                "  "
            };
            let extends = if g.extends.is_empty() {
                String::new()
            } else {
                format!(" extends {}", g.extends.join(" -> "))
            };
            ListItem::new(format!(
                "{marker}{} [{}] {} member(s){extends}",
                g.name,
                g.format,
                g.members.len()
            ))
        })
        .collect();
    frame.render_widget(List::new(items).block(block(&app)), area);
}

fn problems(frame: &mut Frame, area: Rect, app: &App, model: &Model) {
    if model.problems.is_empty() {
        frame.render_widget(
            Paragraph::new("no problems — which is the normal state").block(block(app)),
            area,
        );
        return;
    }
    let mut app = app.clone();
    let window = app.visible(model.problems.len(), area.height.saturating_sub(2) as usize);
    let items: Vec<ListItem> = model.problems[window]
        .iter()
        .map(|p| ListItem::new(p.clone()))
        .collect();
    frame.render_widget(List::new(items).block(block(&app)), area);
}

fn logs(frame: &mut Frame, area: Rect, app: &App, log: &Result<Tail, String>) {
    let tail = match log {
        // Named, not blank: an empty pane that looks broken is worse than an explanation.
        Err(why) => {
            frame.render_widget(
                Paragraph::new(why.clone())
                    .wrap(Wrap { trim: true })
                    .block(block(app)),
                area,
            );
            return;
        }
        Ok(t) => t,
    };

    let height = area.height.saturating_sub(2) as usize;
    let kept: Vec<&crate::tail::Line> = tail.filtered(&app.filter).collect();
    let from = kept.len().saturating_sub(height);
    let items: Vec<ListItem> = kept[from..]
        .iter()
        .map(|l| ListItem::new(l.raw.clone()))
        .collect();

    let title = match &app.filter {
        Filter::All => "logs".to_string(),
        Filter::Problems => "logs — problems only".to_string(),
        Filter::Machine(id) => format!("logs — {id}"),
    };
    let mut b = Block::default().borders(Borders::ALL).title(title);
    if tail.rotated {
        b = b.title_bottom("the log was rotated and is being followed into the new file");
    }
    frame.render_widget(List::new(items).block(b), area);
}
