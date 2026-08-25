//! One line per event. When a PXE install fails to start, this is the only diagnostic
//! anyone has — so it is deliberately boring and greppable, and it goes wherever you say.
//!
//! Two knobs, because the two questions are different. **What** (`RESCRIPTUM_LOG`): a
//! successful answer is one line, and at thirteen thousand requests a second that is the
//! only thing here with any volume, so `problems` keeps everything except the requests
//! that worked. **Where** (`RESCRIPTUM_LOG_FILE`): stderr by default, which is what a
//! supervisor wants; a path for the deployments that have no supervisor, DSM chief among
//! them.
//!
//! A write that fails is dropped rather than propagated. A provisioning server that dies
//! because its log disk filled up would fail every install in flight to report that it
//! could not report something.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// How much to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    /// Every request, plus everything else. The default, and the right one until the
    /// volume of successful answers becomes the problem.
    #[default]
    All,
    /// Startup, warnings, errors, and only the requests that did not succeed.
    Problems,
    /// Nothing.
    Off,
}

impl Level {
    /// Unknown values fall back to `All` rather than to silence: a typo must not be the
    /// reason nobody can see why a rollout failed.
    pub fn parse(value: &str) -> Option<Level> {
        match value.trim().to_ascii_lowercase().as_str() {
            "all" => Some(Level::All),
            "problems" => Some(Level::Problems),
            "off" | "none" => Some(Level::Off),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Level::All => "all",
            Level::Problems => "problems",
            Level::Off => "off",
        }
    }
}

/// Where the lines go.
enum Sink {
    Stderr,
    Stdout,
    File(Mutex<File>),
}

static LEVEL: AtomicU8 = AtomicU8::new(0);
static SINK: OnceLock<Sink> = OnceLock::new();

/// Point logging at its destination. Called once, from `main`, as soon as the
/// configuration is known.
///
/// Opening the file is fatal when it fails, for the same reason a named env file is:
/// carrying on and writing somewhere else would be a silent, and later, surprise. Note
/// that anything logged *while* the configuration is being read has already gone to
/// stderr — before the destination is known, there is nowhere else it could go.
pub fn init(level: Level, target: Option<&Path>) -> Result<(), String> {
    LEVEL.store(level as u8, Ordering::Relaxed);

    let sink = match target.map(Path::as_os_str) {
        None => Sink::Stderr,
        Some(t) if t == "stderr" => Sink::Stderr,
        Some(t) if t == "stdout" => Sink::Stdout,
        Some(_) => {
            let path = target.expect("matched Some above");
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "RESCRIPTUM_LOG_FILE={}: cannot create {}: {e}",
                        path.display(),
                        parent.display()
                    )
                })?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| {
                    format!(
                        "RESCRIPTUM_LOG_FILE={} cannot be opened: {e}",
                        path.display()
                    )
                })?;
            Sink::File(Mutex::new(file))
        }
    };

    // `init` is called once; a second call keeping the first sink is the safe outcome.
    let _ = SINK.set(sink);
    Ok(())
}

fn level() -> Level {
    match LEVEL.load(Ordering::Relaxed) {
        1 => Level::Problems,
        2 => Level::Off,
        _ => Level::All,
    }
}

fn emit(line: &str) {
    match SINK.get() {
        Some(Sink::Stdout) => {
            let _ = writeln!(std::io::stdout(), "{line}");
        }
        Some(Sink::File(handle)) => {
            // A poisoned lock means another thread panicked mid-write. The file is still
            // a file; recovering beats losing every line after the first panic.
            let mut file = handle.lock().unwrap_or_else(|e| e.into_inner());
            let _ = writeln!(file, "{line}");
        }
        // Unset means `init` has not run yet, which is the case while the configuration
        // is still being read. stderr is the only sensible answer then.
        None | Some(Sink::Stderr) => {
            let _ = writeln!(std::io::stderr(), "{line}");
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ`, UTC, without pulling in a date crate.
pub fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// Howard Hinnant's days-from-civil, inverted. Valid for the proleptic Gregorian
/// calendar, which is rather more range than a NAS uptime needs.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Emit a request log line.
///
/// `status` is the HTTP status the client got, or `0` for an exchange that never reached
/// one — a connection that timed out mid-body, say. Anything below 400 is a request that
/// worked, and those are what `RESCRIPTUM_LOG=problems` drops.
pub fn request(peer: &str, status: u16, summary: &str) {
    let keep = match level() {
        Level::All => true,
        Level::Problems => status == 0 || status >= 400,
        Level::Off => false,
    };
    if keep {
        emit(&format!("{} {} {}", timestamp(), peer, summary));
    }
}

/// Emit a server-level line: startup, warnings, accept failures, pool saturation.
///
/// These are low-volume and every one of them is diagnostic, so `problems` keeps them.
pub fn server(message: &str) {
    if level() != Level::Off {
        emit(&format!("{} - {}", timestamp(), message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_epochs_format_correctly() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        // 2024 was a leap year: day 60 of that year is Feb 29.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn timestamp_has_the_expected_shape() {
        let t = timestamp();
        assert_eq!(t.len(), 20, "{t}");
        assert!(t.ends_with('Z'), "{t}");
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], "T");
    }

    #[test]
    fn levels_are_read_from_their_names() {
        assert_eq!(Level::parse("all"), Some(Level::All));
        assert_eq!(Level::parse("  PROBLEMS "), Some(Level::Problems));
        assert_eq!(Level::parse("off"), Some(Level::Off));
        assert_eq!(Level::parse("none"), Some(Level::Off));
        // A typo must not silently turn logging off; the caller falls back to `all`.
        assert_eq!(Level::parse("verbose"), None);
        assert_eq!(Level::parse(""), None);
    }

    #[test]
    fn the_default_is_everything() {
        assert_eq!(Level::default(), Level::All);
    }
}
