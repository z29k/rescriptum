//! Following the server's log from another process.
//!
//! **The server hands its log to nothing.** `log.rs` writes lines to stderr or to the file
//! `RESCRIPTUM_LOG_FILE` names; there is no ring buffer, no endpoint, no socket. So
//! anything that wants to show the log is reading a file, and that has consequences worth
//! stating rather than discovering:
//!
//! - **No `RESCRIPTUM_LOG_FILE`, no log to follow.** If logging goes to stderr, a separate
//!   process cannot see it. Say exactly that and name the setting to change — an empty
//!   pane that looks broken is worse than an explanation.
//! - **Tail from the end.** A NAS that has been provisioning for a year has a large log.
//!   Seek to the end, keep a bounded buffer, poll for growth.
//! - **Handle rotation.** If the file changes underneath — logrotate, or the DSM package's
//!   own rotation — reopen rather than following a deleted inode forever. This is the
//!   classic `tail -f` bug and **it is silent**: the screen simply stops updating.
//! - **Filtering is client-side.** `RESCRIPTUM_LOG=problems` filters at the *source* and
//!   changing it means restarting the server. These two exist for different reasons; do
//!   not conflate them.
//! - **A log line is the parse surface**, so the log format becomes an interface. Accept
//!   that deliberately, keep the parser forgiving, and treat an unparsable line as text to
//!   display rather than as an error.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How much of the end of the file to read on opening. A first screen's worth, without
/// reading a year of provisioning to find it.
pub const WINDOW: u64 = 64 * 1024;

/// One line, parsed as far as it usefully can be.
///
/// Every field is optional on purpose. A line this does not understand is still a line
/// somebody needs to read, and refusing it would hide exactly the unusual event they are
/// looking for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    pub raw: String,
    pub timestamp: Option<String>,
    /// `None` for a server line, which carries `-` where a peer would be.
    pub peer: Option<String>,
    pub status: Option<u16>,
    /// The **extension** — `toml`, `ipxe`, `preseed`. Which is the whole reason
    /// `Resolution::how` reports the extension rather than the family: from the family,
    /// a machine fetching its boot script and the same machine fetching its answer
    /// document are one event.
    pub format: Option<String>,
    pub machine: Option<String>,
    pub group: Option<String>,
}

impl Line {
    /// Whether `RESCRIPTUM_LOG=problems` would have kept this one. Applied here rather
    /// than at the source, because changing the source means restarting the server.
    pub fn is_problem(&self) -> bool {
        match self.status {
            // `0` means the exchange never reached a status — a connection that timed out
            // mid-body, say — and counts as a problem, exactly as `log::request` treats it.
            Some(s) => s == 0 || s >= 400,
            // A server line is startup, a warning or an error: all diagnostic.
            None => true,
        }
    }

    pub fn parse(raw: &str) -> Line {
        let mut line = Line {
            raw: raw.to_string(),
            ..Default::default()
        };
        let mut rest = raw;

        // `YYYY-MM-DDTHH:MM:SSZ`, fixed width, so this is a shape check rather than a
        // date parse — there is no date crate here and this does not add one.
        if let Some((head, tail)) = rest.split_once(' ')
            && head.len() == 20
            && head.ends_with('Z')
        {
            line.timestamp = Some(head.to_string());
            rest = tail;
        }

        if let Some((head, tail)) = rest.split_once(' ') {
            if head != "-" {
                line.peer = Some(head.to_string());
            }
            rest = tail;
        }

        for token in rest.split_whitespace() {
            if let Some(v) = token.strip_prefix("format=") {
                line.format = Some(v.to_string());
            } else if let Some(v) = token.strip_prefix("machine=") {
                line.machine = Some(v.to_string());
            } else if let Some(v) = token.strip_prefix("group=") {
                line.group = Some(v.to_string());
            } else if line.status.is_none()
                && token.len() == 3
                && let Ok(code) = token.parse::<u16>()
                && (100..600).contains(&code)
            {
                line.status = Some(code);
            }
        }
        line
    }

    /// Whether this line is about a given machine, matched the way everything else here
    /// matches: normalized, so separator style never decides.
    pub fn mentions(&self, id: &str) -> bool {
        let wanted = crate::select::normalize(id.as_bytes());
        if wanted.is_empty() {
            return false;
        }
        crate::select::normalize(self.raw.as_bytes()).contains(&wanted)
    }
}

/// What to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    All,
    /// The same rule `RESCRIPTUM_LOG=problems` applies, but here and now.
    Problems,
    /// One machine, however its identifier is spelled.
    Machine(String),
}

impl Filter {
    pub fn keeps(&self, line: &Line) -> bool {
        match self {
            Filter::All => true,
            Filter::Problems => line.is_problem(),
            Filter::Machine(id) => line.mentions(id),
        }
    }
}

/// A file being followed.
pub struct Tail {
    path: PathBuf,
    file: File,
    /// What the file was when it was opened, so a replacement can be noticed.
    identity: Option<(u64, u64)>,
    position: u64,
    lines: VecDeque<Line>,
    capacity: usize,
    /// Set when the file was replaced under us, so a caller can say so once.
    pub rotated: bool,
}

impl Tail {
    /// Open a log and read the tail of it.
    ///
    /// `capacity` bounds what is kept in memory: this follows a file that may be
    /// gigabytes, and holding all of it to show forty lines would be the wrong trade on a
    /// NAS with 512 MB.
    pub fn open(path: impl Into<PathBuf>, capacity: usize) -> std::io::Result<Tail> {
        let path = path.into();
        let mut tail = Tail {
            file: File::open(&path)?,
            identity: identity(&path),
            path,
            position: 0,
            lines: VecDeque::new(),
            capacity,
            rotated: false,
        };
        tail.seek_to_window()?;
        tail.read_new()?;
        Ok(tail)
    }

    /// Where the log would be, or why there is none to follow.
    ///
    /// A pane that is empty because logging goes to stderr looks identical to one that is
    /// broken, so this returns the sentence to show instead of an empty list.
    pub fn describe(log_file: Option<&Path>) -> Result<&Path, String> {
        match log_file {
            None => Err(
                "this server logs to stderr, so no other process can read it. \
                         `rescriptum config set RESCRIPTUM_LOG_FILE=/path/to/log` gives it \
                         somewhere to follow"
                    .to_string(),
            ),
            Some(p) if p == Path::new("stderr") || p == Path::new("stdout") => Err(format!(
                "RESCRIPTUM_LOG_FILE={} is a stream, not a file, so no other process can \
                 read it",
                p.display()
            )),
            Some(p) => Ok(p),
        }
    }

    fn seek_to_window(&mut self) -> std::io::Result<()> {
        let len = self.file.metadata()?.len();
        let from = len.saturating_sub(WINDOW);
        self.position = self.file.seek(SeekFrom::Start(from))?;
        // Starting mid-file almost certainly lands mid-line. Drop the partial one rather
        // than showing half a record as if it were whole.
        if from > 0 {
            let mut discard = Vec::new();
            let mut reader = BufReader::new(&self.file);
            let n = reader.read_until(b'\n', &mut discard)?;
            self.position += n as u64;
            self.file.seek(SeekFrom::Start(self.position))?;
        }
        Ok(())
    }

    /// Read whatever has been appended, and notice if the file was replaced.
    ///
    /// Returns how many lines were added.
    pub fn poll(&mut self) -> std::io::Result<usize> {
        // **Two ways a rotation shows up**, and only checking one of them is how this
        // silently stops updating. logrotate's `create` replaces the file, so the identity
        // changes; its `copytruncate` keeps the same file and empties it, so the identity
        // is unchanged and the length went backwards.
        let now = identity(&self.path);
        let shrank = self
            .file
            .metadata()
            .map(|m| m.len() < self.position)
            .unwrap_or(false);

        if (now.is_some() && now != self.identity) || shrank {
            match File::open(&self.path) {
                Ok(file) => {
                    self.file = file;
                    self.identity = now;
                    self.position = 0;
                    self.file.seek(SeekFrom::Start(0))?;
                    self.rotated = true;
                }
                // The new file may not be there yet — logrotate moves before it creates.
                // Keep the old handle and try again next time rather than giving up.
                Err(_) => return Ok(0),
            }
        }

        self.read_new()
    }

    fn read_new(&mut self) -> std::io::Result<usize> {
        self.file.seek(SeekFrom::Start(self.position))?;
        let mut buffer = String::new();
        let read = self.file.read_to_string(&mut buffer)?;
        if read == 0 {
            return Ok(0);
        }

        // A line that is still being written has no newline yet. Leave it in the file by
        // rewinding past it, so it is read whole next time rather than shown in halves.
        let complete = match buffer.rfind('\n') {
            Some(i) => &buffer[..=i],
            None => return Ok(0),
        };
        self.position += complete.len() as u64;

        let mut added = 0;
        for raw in complete.lines() {
            if raw.is_empty() {
                continue;
            }
            if self.lines.len() == self.capacity {
                self.lines.pop_front();
            }
            self.lines.push_back(Line::parse(raw));
            added += 1;
        }
        Ok(added)
    }

    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter()
    }

    pub fn filtered<'a>(&'a self, filter: &'a Filter) -> impl Iterator<Item = &'a Line> {
        self.lines.iter().filter(move |l| filter.keeps(l))
    }
}

#[cfg(unix)]
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("rescriptum-tail-{}-{name}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir.join("rescriptum.log")
    }

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open");
        f.write_all(text.as_bytes()).expect("write");
    }

    const ANSWERED: &str = "2026-08-30T09:00:00Z 10.0.0.7:41234 POST /answer body=512 200 \
                            format=toml machine=98fa9b50d810 group=rack-a bytes=1024";
    const BOOTED: &str = "2026-08-30T09:00:01Z 10.0.0.7:41235 GET /ipxe/boot body=0 200 \
                          format=ipxe machine=98fa9b50d810 bytes=300";
    const MISSED: &str = "2026-08-30T09:00:02Z 10.0.0.9:41236 POST /answer body=512 404 \
                          no answer file applies for mac=11:22:33:44:55:66";
    const WARNED: &str = "2026-08-30T09:00:03Z - warning: /srv/answers cannot be read";

    #[test]
    fn an_answer_line_is_parsed_into_its_parts() {
        let l = Line::parse(ANSWERED);
        assert_eq!(l.timestamp.as_deref(), Some("2026-08-30T09:00:00Z"));
        assert_eq!(l.peer.as_deref(), Some("10.0.0.7:41234"));
        assert_eq!(l.status, Some(200));
        assert_eq!(l.machine.as_deref(), Some("98fa9b50d810"));
        assert_eq!(l.group.as_deref(), Some("rack-a"));
        assert_eq!(l.format.as_deref(), Some("toml"));
        assert!(!l.is_problem());
    }

    /// **The distinction the whole `format_name` change exists for.** A machine fetching
    /// its boot script and the same machine fetching its answer document are the two ends
    /// of an install; from the family label both said `text`.
    #[test]
    fn a_boot_script_is_distinguishable_from_an_answer_document() {
        assert_eq!(Line::parse(BOOTED).format.as_deref(), Some("ipxe"));
        assert_eq!(Line::parse(ANSWERED).format.as_deref(), Some("toml"));
    }

    #[test]
    fn a_server_line_has_no_peer_and_counts_as_a_problem() {
        let l = Line::parse(WARNED);
        assert!(l.peer.is_none(), "{l:?}");
        assert!(l.is_problem());
    }

    #[test]
    fn a_404_now_names_the_machine_that_asked() {
        let l = Line::parse(MISSED);
        assert_eq!(l.status, Some(404));
        assert!(l.is_problem());
        assert!(l.mentions("11:22:33:44:55:66"));
        // Normalized on both sides, so separator style never decides.
        assert!(l.mentions("112233445566"));
    }

    /// An unparsable line is text to display, never an error — refusing it would hide the
    /// unusual event somebody is looking for.
    #[test]
    fn a_line_this_does_not_understand_is_still_a_line() {
        let l = Line::parse("something nobody planned for");
        assert!(l.timestamp.is_none());
        assert_eq!(l.raw, "something nobody planned for");
        assert!(l.is_problem(), "unknown shapes are shown, not dropped");
    }

    #[test]
    fn filters_are_applied_here_rather_than_at_the_source() {
        let lines: Vec<Line> = [ANSWERED, BOOTED, MISSED, WARNED]
            .iter()
            .map(|l| Line::parse(l))
            .collect();

        let problems = lines.iter().filter(|l| Filter::Problems.keeps(l)).count();
        assert_eq!(problems, 2, "the 404 and the warning");

        let mine = Filter::Machine("98:fa:9b:50:d8:10".to_string());
        assert_eq!(lines.iter().filter(|l| mine.keeps(l)).count(), 2);
        assert_eq!(lines.iter().filter(|l| Filter::All.keeps(l)).count(), 4);
    }

    #[test]
    fn following_a_file_picks_up_what_is_appended() {
        let path = scratch("append");
        append(&path, &format!("{ANSWERED}\n"));
        let mut tail = Tail::open(&path, 100).expect("open");
        assert_eq!(tail.lines().count(), 1);

        append(&path, &format!("{BOOTED}\n{MISSED}\n"));
        assert_eq!(tail.poll().expect("poll"), 2);
        assert_eq!(tail.lines().count(), 3);
        // And nothing new is nothing new, rather than the same lines again.
        assert_eq!(tail.poll().expect("poll"), 0);
    }

    /// A record still being written must not be shown in halves.
    #[test]
    fn a_partial_line_waits_for_its_newline() {
        let path = scratch("partial");
        append(&path, &format!("{ANSWERED}\n"));
        let mut tail = Tail::open(&path, 100).expect("open");

        append(
            &path,
            "2026-08-30T09:00:09Z 10.0.0.7:1 POST /answer body=1 2",
        );
        assert_eq!(tail.poll().expect("poll"), 0, "half a line is not a line");

        append(&path, "00 format=toml\n");
        assert_eq!(tail.poll().expect("poll"), 1);
        assert_eq!(tail.lines().last().expect("line").status, Some(200));
    }

    /// **The classic `tail -f` bug, and it is silent**: following a deleted inode forever
    /// while the screen quietly stops updating. This is logrotate's `create`.
    #[test]
    fn a_rotated_file_is_reopened_rather_than_followed_into_the_void() {
        let path = scratch("rotate");
        append(&path, &format!("{ANSWERED}\n"));
        let mut tail = Tail::open(&path, 100).expect("open");
        assert_eq!(tail.lines().count(), 1);

        // What logrotate does: move the old one aside, create a new one.
        std::fs::rename(&path, path.with_extension("log.1")).expect("rotate");
        append(&path, &format!("{BOOTED}\n"));

        assert_eq!(tail.poll().expect("poll"), 1, "it must follow the new file");
        assert!(tail.rotated, "and say that it did");
        assert_eq!(
            tail.lines().last().expect("line").format.as_deref(),
            Some("ipxe")
        );
    }

    /// The other half: `copytruncate` keeps the same inode and empties it, so only the
    /// length going backwards gives it away. Checking one and not the other is how this
    /// stops updating for half the deployments.
    #[test]
    fn a_truncated_file_is_noticed_even_though_it_is_the_same_file() {
        let path = scratch("truncate");
        append(&path, &format!("{ANSWERED}\n{BOOTED}\n"));
        let mut tail = Tail::open(&path, 100).expect("open");
        assert_eq!(tail.lines().count(), 2);

        std::fs::write(&path, format!("{MISSED}\n")).expect("truncate");
        assert_eq!(tail.poll().expect("poll"), 1);
        assert!(tail.rotated);
    }

    /// A NAS that has been provisioning for a year has a large log, and holding all of it
    /// to show forty lines is the wrong trade on a machine with 512 MB.
    #[test]
    fn the_buffer_is_bounded_and_keeps_the_newest() {
        let path = scratch("bounded");
        for n in 0..50 {
            append(
                &path,
                &format!("2026-08-30T09:00:00Z 10.0.0.7:1 GET /x body=0 200 format=toml n={n}\n"),
            );
        }
        let tail = Tail::open(&path, 10).expect("open");
        assert_eq!(tail.lines().count(), 10);
        assert!(
            tail.lines().last().expect("line").raw.contains("n=49"),
            "the newest lines are the ones worth keeping"
        );
    }

    /// An empty pane that looks broken is worse than an explanation.
    #[test]
    fn there_being_no_file_to_follow_is_explained_rather_than_shown_empty() {
        let e = Tail::describe(None).expect_err("stderr is not followable");
        assert!(e.contains("RESCRIPTUM_LOG_FILE"), "{e}");
        assert!(e.contains("config set"), "it must say how to fix it: {e}");

        assert!(Tail::describe(Some(Path::new("stderr"))).is_err());
        assert!(Tail::describe(Some(Path::new("/var/log/x.log"))).is_ok());
    }
}
