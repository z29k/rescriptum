//! Recording what machines actually send.
//!
//! Everything this server does was built against documentation; until an installer has
//! really talked to it, that is a claim rather than a fact. Turning this on writes each
//! request body to a file, which serves three purposes: answering "what did node07
//! actually send?" when a rollout misbehaves, replaying a real request through
//! `render --body`, and giving this project a corpus of genuine fixtures instead of
//! ones someone imagined.
//!
//! It is off unless `RESCRIPTUM_CAPTURE_DIR` is set, and it is bounded: a provisioning server
//! that fills its own disk is worse than one that captures nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Stop after this many files. Reached, capturing stops and says so once; nothing
/// already written is ever deleted, because the whole point is to keep evidence.
pub const MAX_CAPTURES: usize = 1000;

pub struct Capture {
    dir: PathBuf,
    seq: AtomicUsize,
    /// Counted rather than re-read from the directory on every request.
    written: AtomicUsize,
    warned: AtomicUsize,
}

impl Capture {
    /// `None` when capturing is off.
    pub fn new(dir: Option<&Path>) -> Option<Capture> {
        let dir = dir?;
        if let Err(e) = fs::create_dir_all(dir) {
            crate::log::server(&format!(
                "capture: cannot use {}: {e} — capturing disabled",
                dir.display()
            ));
            return None;
        }
        // Count what is already there, so restarting does not blow past the cap.
        // Counted as captures, not as files: each one writes a `.body` and a `.meta`,
        // so counting entries would mean the cap said something different on a restart
        // than it did on a fresh start. The `.body` files *are* the captures.
        let existing = fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "body"))
                    .count()
            })
            .unwrap_or(0);
        crate::log::server(&format!(
            "capturing request bodies to {} ({existing} already there, cap {MAX_CAPTURES})",
            dir.display()
        ));
        Some(Capture {
            dir: dir.to_path_buf(),
            seq: AtomicUsize::new(0),
            written: AtomicUsize::new(existing),
            warned: AtomicUsize::new(0),
        })
    }

    /// Write one request. Never fails a request: a capture problem is logged and the
    /// install carries on.
    pub fn record(&self, peer: &str, method: &str, target: &str, body: &[u8], outcome: &str) {
        if self.written.load(Ordering::Relaxed) >= MAX_CAPTURES {
            if self.warned.fetch_add(1, Ordering::Relaxed) == 0 {
                crate::log::server(&format!(
                    "capture: {MAX_CAPTURES} captures reached in {} — capturing no more",
                    self.dir.display()
                ));
            }
            return;
        }
        self.written.fetch_add(1, Ordering::Relaxed);

        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        // The peer carries a port and, for IPv6, colons — neither belongs in a filename.
        let mut host: String = peer
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(peer)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        // Dots are kept so an IPv4 address stays readable, but never two in a row: a
        // filename carrying `..` is harmless here and still not worth writing.
        while host.contains("..") {
            host = host.replace("..", "-");
        }
        let stem = format!("{}-{host}-{n:04}", crate::log::timestamp().replace(':', ""));

        // The body verbatim, so `render --body` can replay it unchanged.
        let body_path = self.dir.join(format!("{stem}.body"));
        if let Err(e) = fs::write(&body_path, body) {
            crate::log::server(&format!(
                "capture: cannot write {}: {e}",
                body_path.display()
            ));
            return;
        }
        let meta = format!(
            "time: {}\npeer: {peer}\nrequest: {method} {target}\nbody-bytes: {}\noutcome: {outcome}\n",
            crate::log::timestamp(),
            body.len()
        );
        let meta_path = self.dir.join(format!("{stem}.meta"));
        if let Err(e) = fs::write(&meta_path, meta) {
            crate::log::server(&format!(
                "capture: cannot write {}: {e}",
                meta_path.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize as Counter;

    fn scratch(tag: &str) -> PathBuf {
        static N: Counter = Counter::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("pve-capture-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn capturing_is_off_unless_a_directory_is_given() {
        assert!(Capture::new(None).is_none());
    }

    #[test]
    fn a_request_is_written_body_and_metadata() {
        let dir = scratch("write");
        let capture = Capture::new(Some(&dir)).expect("should enable");
        capture.record(
            "10.0.0.42:51234",
            "POST",
            "/answer",
            b"{\"mac\":\"aa:bb\"}",
            "200",
        );

        let files: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(files.len(), 2, "{files:?}");

        let body = files
            .iter()
            .find(|f| f.ends_with(".body"))
            .expect("body file");
        // Verbatim, so it can be replayed through `render --body`.
        assert_eq!(fs::read(dir.join(body)).unwrap(), b"{\"mac\":\"aa:bb\"}");

        let meta = files
            .iter()
            .find(|f| f.ends_with(".meta"))
            .expect("meta file");
        let meta = fs::read_to_string(dir.join(meta)).unwrap();
        assert!(meta.contains("POST /answer"), "{meta}");
        assert!(meta.contains("10.0.0.42:51234"), "{meta}");
        assert!(meta.contains("outcome: 200"), "{meta}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_peer_address_never_escapes_into_the_filename() {
        let dir = scratch("peer");
        let capture = Capture::new(Some(&dir)).expect("should enable");
        capture.record("[fe80::1%eth0]:443", "POST", "/a", b"x", "200");
        capture.record("../../etc:1", "POST", "/a", b"x", "200");

        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(!name.contains(".."), "{name}");
            assert!(!name.contains('/'), "{name}");
            assert!(!name.contains(':'), "{name}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn capturing_stops_at_the_cap_rather_than_filling_the_disk() {
        let dir = scratch("cap");
        let capture = Capture::new(Some(&dir)).expect("should enable");
        for _ in 0..(MAX_CAPTURES + 50) {
            capture.record("10.0.0.1:1", "POST", "/a", b"x", "200");
        }
        // The cap counts captures; each writes a body and a meta.
        let count = fs::read_dir(&dir).unwrap().flatten().count();
        assert_eq!(count, MAX_CAPTURES * 2, "wrote {count} files");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restart_does_not_blow_past_the_cap() {
        // The cap is on the directory, not on one process's lifetime. A server restarted
        // every hour would otherwise fill the disk a thousand files at a time, which is
        // exactly the failure the cap exists to prevent.
        let dir = scratch("restart-cap");
        let first = Capture::new(Some(&dir)).expect("should enable");
        for _ in 0..MAX_CAPTURES {
            first.record("10.0.0.1:1", "POST", "/a", b"x", "200");
        }
        let after_first = fs::read_dir(&dir).unwrap().flatten().count();
        assert_eq!(
            after_first,
            MAX_CAPTURES * 2,
            "the first run should fill the cap"
        );

        // A second process over the same directory starts from what is already there.
        let second = Capture::new(Some(&dir)).expect("should enable");
        for _ in 0..50 {
            second.record("10.0.0.2:2", "POST", "/b", b"y", "200");
        }
        assert_eq!(
            fs::read_dir(&dir).unwrap().flatten().count(),
            after_first,
            "a restart must not start the count again"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_already_written_is_ever_removed() {
        // Rotating the directory is the operator's job. A capture that tidied up after
        // itself would delete the one body somebody was about to send with a bug report.
        let dir = scratch("keep");
        fs::create_dir_all(&dir).unwrap();
        let precious = dir.join("2020-01-01T000000Z-10.0.0.9-0000.body");
        fs::write(&precious, b"an older capture").unwrap();

        let capture = Capture::new(Some(&dir)).expect("should enable");
        for _ in 0..(MAX_CAPTURES + 10) {
            capture.record("10.0.0.1:1", "POST", "/a", b"x", "200");
        }
        assert_eq!(
            fs::read(&precious).expect("must still be there"),
            b"an older capture"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_body_is_recorded_byte_for_byte_even_when_it_is_not_text() {
        // `render --body` replays it, so anything the recorder normalises is a request
        // that can no longer be reproduced.
        let dir = scratch("verbatim");
        let capture = Capture::new(Some(&dir)).expect("should enable");
        let body: Vec<u8> = vec![0xff, 0xfe, 0x00, b'{', b'}', 0x80, b'\n'];
        capture.record("10.0.0.1:1", "POST", "/a", &body, "200");

        let written = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .find(|e| e.path().extension().is_some_and(|x| x == "body"))
            .expect("a .body file");
        assert_eq!(fs::read(written.path()).unwrap(), body);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unusable_directory_disables_capturing_instead_of_failing() {
        // A capture problem must never cost an install.
        let dir = scratch("bad");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not-a-directory");
        fs::write(&file, b"x").unwrap();
        assert!(Capture::new(Some(&file)).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
