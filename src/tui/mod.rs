//! The state a terminal interface keeps, and the rules about when it may do work.
//!
//! **ratatui is immediate-mode: the state is ours.** It draws what it is handed each
//! frame, so selection, scroll offset and the current pane live in a struct here rather
//! than inside a widget. There is no retained tree to look for.
//!
//! Which means the interesting half of the interface has nothing to do with drawing, and
//! is therefore here — compiled and tested in every build, including
//! `--no-default-features`. The draw layer on top is thin by construction, and the rules
//! below are what keep it that way:
//!
//! - **A key press never does the work.** `on_key` returns an [`Action`] describing what
//!   should happen; something outside the draw path performs it. That is what makes
//!   "never do network IO on a redraw" a property rather than a discipline — one
//!   unreachable BMC must not be able to freeze a screen.
//! - **Redraw on an event, not on a timer.** A 60 fps loop on an ARMv7 NAS burns a core
//!   for a screen nobody is looking at.
//! - **Nothing here is the only way to do anything.** Every action below has a command
//!   that does the same thing, because scripts, `deploy.sh` and CI cannot press keys.

/// The painting, and only the painting. Behind the feature, so the answer server a NAS
/// runs links none of it.
#[cfg(feature = "tui")]
pub mod draw;
/// Reading a deployment's fleet over the admin API, for `tui --remote`.
#[cfg(feature = "tui")]
pub mod remote;

use crate::tail::Filter;

/// The screens, in the order they are cycled through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// Is this thing working, and what is happening.
    Dashboard,
    Machines,
    Groups,
    Media,
    Boot,
    Logs,
    Problems,
}

impl Pane {
    pub const ORDER: [Pane; 7] = [
        Pane::Dashboard,
        Pane::Machines,
        Pane::Groups,
        Pane::Media,
        Pane::Boot,
        Pane::Logs,
        Pane::Problems,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Dashboard => "dashboard",
            Pane::Machines => "machines",
            Pane::Groups => "groups",
            Pane::Media => "media",
            Pane::Boot => "boot",
            Pane::Logs => "logs",
            Pane::Problems => "problems",
        }
    }

    /// Whether this screen needs the boot feature to say anything.
    ///
    /// A build without `boot` keeps the screen and explains itself, exactly as `cli`'s
    /// `media` and `boot` subcommands do — the surface stays described everywhere, and
    /// only the binary that cannot honour it objects.
    pub fn needs_boot(self) -> bool {
        matches!(self, Pane::Media | Pane::Boot)
    }

    fn next(self) -> Pane {
        let i = Pane::ORDER.iter().position(|p| *p == self).unwrap_or(0);
        Pane::ORDER[(i + 1) % Pane::ORDER.len()]
    }

    fn previous(self) -> Pane {
        let i = Pane::ORDER.iter().position(|p| *p == self).unwrap_or(0);
        Pane::ORDER[(i + Pane::ORDER.len() - 1) % Pane::ORDER.len()]
    }
}

/// A key, reduced to what this program cares about.
///
/// Its own type rather than crossterm's, so the state machine compiles and is tested
/// without a terminal library — and so a different one could be swapped underneath.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Tab,
    BackTab,
    Enter,
    Escape,
    Char(char),
}

/// Work the interface wants done — **returned, never performed here**.
///
/// Everything that touches the network, the store or a controller becomes one of these and
/// is carried out away from the draw path. A rack of dead BMCs then cannot freeze a
/// screen, because the screen was never the thing waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Re-read the store. Cheap, local, and the only one safe to do often.
    Reload,
    /// Ask every controller whether it is on. **Bounded, concurrent, and on demand** —
    /// never what a redraw does.
    Probe,
    /// Show what one machine would be served.
    Render(String),
    /// Hand a document to `$EDITOR`, which means suspending the screen.
    Edit(String),
    Arm(String),
    Disarm(String),
    /// Power actions, which exist only where a controller does.
    PowerOn(String),
    PowerOff {
        id: String,
        hard: bool,
    },
    Pxe(String),
    /// Leave.
    Quit,
}

impl Action {
    /// Whether performing this can block on something outside this machine.
    ///
    /// The draw path may never run one of these, and the test below pins that no key
    /// press produces one synchronously.
    pub fn is_slow(&self) -> bool {
        matches!(
            self,
            Action::Probe
                | Action::PowerOn(_)
                | Action::PowerOff { .. }
                | Action::Pxe(_)
                | Action::Edit(_)
        )
    }
}

/// What the interface is showing and where it is looking.
#[derive(Debug, Clone)]
pub struct App {
    pub pane: Pane,
    /// Which row, within the current pane's list.
    pub selected: usize,
    /// The first visible row, kept so the selection is always on screen.
    pub offset: usize,
    pub filter: Filter,
    /// Set when a pane cannot say anything in this build, so it explains instead of
    /// looking broken.
    pub note: Option<String>,
    /// Typed into the filter box, when one is open.
    pub search: Option<String>,
    /// Reading a deployment over the admin API rather than a store on this machine.
    ///
    /// **Nothing powers anything in this mode**, and that is a property of the state
    /// machine rather than a rule somebody remembers. The answer endpoint is
    /// unauthenticated by necessity and the admin API decides what every machine installs;
    /// putting power control over the wire is how a provisioning server becomes a weapon.
    /// The plan says never, so `on_key` refuses rather than the loop declining later.
    pub remote: bool,
}

impl Default for App {
    fn default() -> App {
        App {
            pane: Pane::Dashboard,
            selected: 0,
            offset: 0,
            filter: Filter::All,
            note: None,
            search: None,
            remote: false,
        }
    }
}

impl App {
    /// Handle one key, given how many rows the current pane holds.
    ///
    /// `rows` is passed in rather than held, because the pane's contents belong to the
    /// model the interface renders and not to the interface.
    pub fn on_key(&mut self, key: Key, rows: usize, selected_id: Option<&str>) -> Option<Action> {
        // A filter box swallows keys while it is open, or typing `q` in it would quit.
        if let Some(text) = &mut self.search {
            match key {
                Key::Escape => {
                    self.search = None;
                    self.filter = Filter::All;
                }
                Key::Enter => {
                    let text = text.clone();
                    self.search = None;
                    self.filter = if text.is_empty() {
                        Filter::All
                    } else {
                        Filter::Machine(text)
                    };
                }
                Key::Char(c) => text.push(c),
                _ => {}
            }
            return None;
        }

        match key {
            Key::Tab => {
                self.go(self.pane.next());
                None
            }
            Key::BackTab => {
                self.go(self.pane.previous());
                None
            }
            Key::Up => {
                self.selected = self.selected.saturating_sub(1);
                None
            }
            Key::Down => {
                if rows > 0 {
                    self.selected = (self.selected + 1).min(rows - 1);
                }
                None
            }
            Key::Home => {
                self.selected = 0;
                None
            }
            Key::End => {
                self.selected = rows.saturating_sub(1);
                None
            }
            Key::PageUp => {
                self.selected = self.selected.saturating_sub(10);
                None
            }
            Key::PageDown => {
                if rows > 0 {
                    self.selected = (self.selected + 10).min(rows - 1);
                }
                None
            }
            Key::Char('q') => Some(Action::Quit),
            Key::Char('r') => Some(Action::Reload),
            Key::Char('/') => {
                self.search = Some(String::new());
                None
            }
            // Problems-only, because that is the pane where "show me only the bad ones"
            // is the whole point. Elsewhere it would silently mean something else.
            Key::Char('p') if self.pane == Pane::Logs => {
                self.filter = if self.filter == Filter::Problems {
                    Filter::All
                } else {
                    Filter::Problems
                };
                None
            }
            // **Power and editing are local only, and this arm is why no screen can
            // forget.** It sits above every action arm on purpose: placed lower, the
            // `Key::Char('e')` and `Key::Char('a')` arms would match first.
            _ if self.remote
                && matches!(
                    key,
                    Key::Char('o' | 'O' | 'X' | 'x' | 'e' | 's' | 'a' | 'd')
                ) =>
            {
                self.note = Some(
                    "local only — this is a remote view, and nothing here powers anything"
                        .to_string(),
                );
                None
            }

            // Everything below needs a row under the cursor, so a pane with nothing in it
            // does nothing rather than acting on row zero of an empty list.
            Key::Enter => selected_id.map(|id| Action::Render(id.to_string())),
            Key::Char('e') => selected_id.map(|id| Action::Edit(id.to_string())),
            Key::Char('a') => selected_id.map(|id| Action::Arm(id.to_string())),
            Key::Char('d') => selected_id.map(|id| Action::Disarm(id.to_string())),
            Key::Char('o') => selected_id.map(|id| Action::PowerOn(id.to_string())),
            Key::Char('O') => selected_id.map(|id| Action::PowerOff {
                id: id.to_string(),
                hard: false,
            }),
            // Upper case and a separate key: forcing a machine off mid-write is how a
            // filesystem gets repaired by hand later.
            Key::Char('X') => selected_id.map(|id| Action::PowerOff {
                id: id.to_string(),
                hard: true,
            }),
            Key::Char('x') => selected_id.map(|id| Action::Pxe(id.to_string())),
            Key::Char('s') => Some(Action::Probe),
            Key::Escape => {
                self.filter = Filter::All;
                None
            }
            _ => None,
        }
    }

    fn go(&mut self, pane: Pane) {
        self.pane = pane;
        // Row three of `machines` means nothing in `groups`.
        self.selected = 0;
        self.offset = 0;
        self.note = None;
    }

    /// The rows to draw, given the height available — and the offset adjusted so the
    /// selection is on screen.
    ///
    /// Scrolling belongs here rather than in the draw call, because "the selection is
    /// always visible" is a property of the state, and a draw that computed it would have
    /// to compute it identically in every pane.
    pub fn visible(&mut self, rows: usize, height: usize) -> std::ops::Range<usize> {
        if height == 0 || rows == 0 {
            self.offset = 0;
            return 0..0;
        }
        self.selected = self.selected.min(rows - 1);
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height;
        }
        // A list that shrank under a scrolled view must not leave a window past its end.
        self.offset = self.offset.min(rows.saturating_sub(height));
        let end = (self.offset + height).min(rows);
        self.offset..end
    }

    /// What this pane can say in this build.
    ///
    /// A screen that is empty because the feature is off looks identical to one that is
    /// broken, so it carries the sentence instead.
    pub fn describe_pane(&self, has_boot: bool) -> Option<&'static str> {
        if self.pane.needs_boot() && !has_boot {
            return Some(
                "this binary was built without the `boot` feature, so it has no media or \
                 boot to show",
            );
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::default()
    }

    /// **The rule that keeps one unreachable BMC from freezing a screen.** A key press
    /// produces a description of work; something off the draw path does it.
    #[test]
    fn no_key_press_performs_slow_work_itself() {
        let mut a = app();
        a.pane = Pane::Machines;
        for key in [
            Key::Char('s'),
            Key::Char('o'),
            Key::Char('X'),
            Key::Char('x'),
            Key::Char('e'),
        ] {
            let action = a.on_key(key, 3, Some("98fa9b50d810"));
            let action = action.expect("these keys mean something on a machine row");
            assert!(
                action.is_slow(),
                "{key:?} should be recognised as work to do elsewhere: {action:?}"
            );
        }
    }

    #[test]
    fn a_pane_with_nothing_in_it_acts_on_nothing() {
        // Row zero of an empty list is not a machine, and arming it would be a surprise.
        let mut a = app();
        a.pane = Pane::Machines;
        for key in [Key::Enter, Key::Char('e'), Key::Char('a'), Key::Char('o')] {
            assert_eq!(a.on_key(key, 0, None), None, "{key:?}");
        }
    }

    #[test]
    fn moving_between_panes_forgets_the_row() {
        // Row three of `machines` means nothing in `groups`.
        let mut a = app();
        a.on_key(Key::Tab, 9, None);
        a.on_key(Key::Down, 9, None);
        a.on_key(Key::Down, 9, None);
        assert_eq!(a.selected, 2);
        a.on_key(Key::Tab, 9, None);
        assert_eq!(a.selected, 0);
        assert_eq!(a.offset, 0);
    }

    #[test]
    fn the_panes_cycle_in_both_directions() {
        let mut a = app();
        assert_eq!(a.pane, Pane::Dashboard);
        a.on_key(Key::BackTab, 0, None);
        assert_eq!(a.pane, Pane::Problems, "wrapping backwards");
        a.on_key(Key::Tab, 0, None);
        assert_eq!(a.pane, Pane::Dashboard, "and forwards again");
    }

    #[test]
    fn the_selection_cannot_leave_the_list() {
        let mut a = app();
        for _ in 0..20 {
            a.on_key(Key::Down, 3, None);
        }
        assert_eq!(a.selected, 2);
        for _ in 0..20 {
            a.on_key(Key::Up, 3, None);
        }
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn scrolling_keeps_the_selection_on_screen() {
        let mut a = app();
        a.selected = 0;
        assert_eq!(a.visible(100, 10), 0..10);

        a.selected = 15;
        let window = a.visible(100, 10);
        assert!(window.contains(&15), "{window:?}");
        assert_eq!(window.len(), 10);

        a.selected = 2;
        let window = a.visible(100, 10);
        assert!(window.contains(&2), "scrolling back up: {window:?}");
    }

    /// A list that shrank under a scrolled view — a machine removed while the screen was
    /// halfway down it — must not leave a window past the end.
    #[test]
    fn a_list_that_shrinks_does_not_leave_the_window_past_its_end() {
        let mut a = app();
        a.selected = 90;
        a.visible(100, 10);
        let window = a.visible(12, 10);
        assert!(window.end <= 12, "{window:?}");
        assert!(
            a.selected < 12,
            "the selection followed the list: {}",
            a.selected
        );
    }

    #[test]
    fn an_empty_list_has_an_empty_window_rather_than_a_panic() {
        let mut a = app();
        a.selected = 5;
        assert_eq!(a.visible(0, 10), 0..0);
        assert_eq!(a.visible(10, 0), 0..0);
    }

    /// Typing `q` into a filter box must not quit.
    #[test]
    fn a_filter_box_swallows_the_keys_that_would_otherwise_be_commands() {
        let mut a = app();
        assert_eq!(a.on_key(Key::Char('/'), 3, None), None);
        for c in "qro".chars() {
            assert_eq!(a.on_key(Key::Char(c), 3, None), None, "{c} must be typed");
        }
        assert_eq!(a.search.as_deref(), Some("qro"));

        a.on_key(Key::Enter, 3, None);
        assert_eq!(a.filter, Filter::Machine("qro".to_string()));
        assert!(a.search.is_none());
    }

    #[test]
    fn an_empty_search_clears_the_filter_rather_than_matching_nothing() {
        let mut a = app();
        a.filter = Filter::Machine("x".to_string());
        a.on_key(Key::Char('/'), 3, None);
        a.on_key(Key::Enter, 3, None);
        assert_eq!(a.filter, Filter::All);
    }

    /// Client-side, and only where it means something: `RESCRIPTUM_LOG=problems` filters
    /// at the source and needs a restart, which is a different thing entirely.
    #[test]
    fn problems_only_toggles_and_only_on_the_log_screen() {
        let mut a = app();
        a.pane = Pane::Logs;
        a.on_key(Key::Char('p'), 5, None);
        assert_eq!(a.filter, Filter::Problems);
        a.on_key(Key::Char('p'), 5, None);
        assert_eq!(a.filter, Filter::All, "it toggles back");

        let mut a = app();
        a.pane = Pane::Machines;
        a.on_key(Key::Char('p'), 5, None);
        assert_eq!(a.filter, Filter::All, "elsewhere it must mean nothing");
    }

    /// A screen that is empty because the feature is off looks identical to one that is
    /// broken, so it says which.
    #[test]
    fn a_pane_that_needs_a_feature_it_does_not_have_explains_itself() {
        let mut a = app();
        a.pane = Pane::Media;
        assert!(a.describe_pane(false).is_some());
        assert!(a.describe_pane(true).is_none());

        a.pane = Pane::Machines;
        assert!(
            a.describe_pane(false).is_none(),
            "machines needs no feature"
        );
    }

    /// **Nothing powers anything over the wire.** The answer endpoint is unauthenticated
    /// by necessity and the admin API sets the root password of every machine installed
    /// afterwards; power control near either is how a provisioning server becomes a
    /// weapon. Refused in the state machine, so no screen can forget.
    #[test]
    fn a_remote_view_powers_nothing_and_says_so() {
        let mut a = app();
        a.remote = true;
        a.pane = Pane::Machines;
        for key in [
            Key::Char('o'),
            Key::Char('O'),
            Key::Char('X'),
            Key::Char('x'),
            Key::Char('s'),
        ] {
            assert_eq!(
                a.on_key(key, 3, Some("98fa9b50d810")),
                None,
                "{key:?} must do nothing remotely"
            );
            assert!(
                a.note.as_deref().is_some_and(|n| n.contains("local only")),
                "and it must say why: {:?}",
                a.note
            );
        }
    }

    /// Editing suspends the screen and hands a document to `$EDITOR` on *this* machine.
    /// Over the wire that is somebody else's document and somebody else's editor.
    #[test]
    fn a_remote_view_does_not_edit_either() {
        let mut a = app();
        a.remote = true;
        a.pane = Pane::Machines;
        assert_eq!(a.on_key(Key::Char('e'), 3, Some("98fa9b50d810")), None);
    }

    /// Reading still works, or the mode would be pointless.
    #[test]
    fn a_remote_view_still_navigates_and_renders() {
        let mut a = app();
        a.remote = true;
        a.pane = Pane::Machines;
        assert_eq!(
            a.on_key(Key::Enter, 3, Some("98fa9b50d810")),
            Some(Action::Render("98fa9b50d810".to_string()))
        );
        assert_eq!(a.on_key(Key::Char('r'), 3, None), Some(Action::Reload));
        assert_eq!(a.on_key(Key::Char('q'), 3, None), Some(Action::Quit));
    }

    #[test]
    fn q_quits_and_r_reloads_and_neither_is_slow() {
        let mut a = app();
        assert_eq!(a.on_key(Key::Char('q'), 0, None), Some(Action::Quit));
        let reload = a.on_key(Key::Char('r'), 0, None).expect("reload");
        assert_eq!(reload, Action::Reload);
        // Re-reading the store is local, so it is the one thing safe to do often.
        assert!(!reload.is_slow());
    }
}
