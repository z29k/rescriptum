//! Editing an answer document in the operator's own editor.
//!
//! Writing a TOML editor inside a terminal UI is months of work to arrive somewhere worse
//! than vim. Shelling out is also how the operator keeps their comments and their
//! formatting: the file store round-trips them, and the admin API returns documents as
//! written.
//!
//! Four things decide whether it works, and all four are here rather than in a screen:
//!
//! - **Through `StoreWrite` and the guard, never in place.** Editing the real path
//!   directly bypasses the rollback entirely — which is the one thing standing between a
//!   keystroke and a rack that cannot install.
//! - **An unchanged buffer is a no-op**, not a write. Quitting an editor must not touch an
//!   mtime, bump a version, or run the guard.
//! - **The document's filename survives.** `format::canonical_stem` only names a document
//!   nobody has named; an existing one is overwritten where it stands, so an operator's
//!   own name is kept. That is the property they will notice if it breaks.
//! - **`$EDITOR` unset is named, not guessed.** Fall back to `vi` — busybox has one on
//!   DSM — and say which one is being launched.

use crate::guard::{self, Outcome, Target};
use crate::select::Answers;
use crate::store::StoreWrite;
use std::path::{Path, PathBuf};

/// What an edit did.
#[derive(Debug)]
pub enum Edited {
    /// The buffer came back identical. **Nothing was written**, so no mtime moved, no
    /// version was bumped and the guard did not run.
    Unchanged,
    /// It was written, and the guard was happy. Carries whatever is still wrong with the
    /// answer set — including what was already wrong.
    Stored(Vec<String>),
    /// It would have broken the answer set, so it was put back.
    Refused(Vec<String>),
    /// The editor could not be run, the temporary file could not be handled, or the store
    /// refused it.
    Failed(String),
}

/// Which editor, and why that one.
///
/// Named rather than guessed: an operator whose `$EDITOR` is unset should be told what is
/// about to open, not surprised by it.
pub fn editor(from_env: Option<String>) -> (String, Option<String>) {
    match from_env
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
    {
        Some(e) => (e, None),
        None => (
            "vi".to_string(),
            Some("$EDITOR is not set, so vi is being used — busybox has one on DSM".to_string()),
        ),
    }
}

/// Where the scratch copy goes.
///
/// **Not in the answers directory.** Every servable file at the top of that directory is
/// an answer document, and a `.toml` dropped there would be reported as a misplaced one —
/// the same rule a configuration file has. The system temporary directory is where this
/// belongs, and the file is removed whatever happens.
///
/// **The process id alone is not enough**, which is the same trap `store::file` had: two
/// edits of one document in one process would share the path, and one would silently take
/// the other's buffer. A counter closes it.
static SCRATCH_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn scratch_path(target: &Target, format: &str) -> PathBuf {
    let id = match target {
        Target::Machine(id) | Target::Group(id) => id.as_str(),
        Target::Default => "default",
    };
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    std::env::temp_dir().join(format!(
        "rescriptum-edit-{}-{}-{safe}.{format}",
        std::process::id(),
        SCRATCH_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

/// Hand `before` to an editor, and store whatever comes back.
///
/// `launch` is the thing that actually runs the editor, taking the scratch path and
/// returning whether it exited cleanly. It is a parameter so that every rule above can be
/// tested without a terminal — which is the whole reason this is a module rather than part
/// of a screen.
pub fn round_trip(
    answers: &Answers,
    store: &dyn StoreWrite,
    target: &Target,
    format: &str,
    before: &str,
    launch: impl FnOnce(&Path) -> Result<(), String>,
) -> Edited {
    let path = scratch_path(target, format);
    if let Err(e) = std::fs::write(&path, before) {
        return Edited::Failed(format!("cannot write {}: {e}", path.display()));
    }

    let launched = launch(&path);
    let after = std::fs::read_to_string(&path);
    // Removed whatever happened: this is somebody's answer document, and it holds a root
    // password hash.
    let _ = std::fs::remove_file(&path);

    if let Err(e) = launched {
        return Edited::Failed(e);
    }
    let after = match after {
        Ok(text) => text,
        Err(e) => return Edited::Failed(format!("cannot read the edited file back: {e}")),
    };

    // Quitting an editor must not touch an mtime, bump a version, or run the guard.
    if after == before {
        return Edited::Unchanged;
    }

    match guard::write(answers, store, target, format, Some(&after)) {
        Outcome::Stored { problems } => Edited::Stored(problems),
        Outcome::Refused { introduced } => Edited::Refused(introduced),
        Outcome::Rejected(e) | Outcome::Unavailable(e) => Edited::Failed(e),
        // A put never reports this; naming it beats a wildcard that would hide a change.
        Outcome::NotFound | Outcome::Deleted { .. } => {
            Edited::Failed("the store reported a delete for a write".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FileStore;
    use std::sync::Arc;

    fn scratch(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rescriptum-edit-t-{}-{name}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn subject(dir: &Path) -> (Answers, Arc<FileStore>) {
        (
            Answers::new(Arc::new(FileStore::new(dir))),
            Arc::new(FileStore::new(dir)),
        )
    }

    #[test]
    fn an_unset_editor_is_named_rather_than_guessed() {
        let (which, note) = editor(None);
        assert_eq!(which, "vi");
        assert!(note.expect("a note").contains("$EDITOR is not set"));

        let (which, note) = editor(Some("  nvim ".to_string()));
        assert_eq!(which, "nvim");
        assert!(note.is_none());
        // An exported-but-empty variable counts as unset, the same rule the configuration
        // has everywhere else.
        assert_eq!(editor(Some(String::new())).0, "vi");
    }

    /// Every servable `.toml` at the top of the answers directory is an answer document,
    /// so a scratch copy dropped there would be reported as a misplaced one.
    #[test]
    fn the_scratch_copy_is_not_in_the_answers_directory() {
        let p = scratch_path(&Target::Machine("98:fa:9b:50:d8:10".to_string()), "toml");
        assert!(p.starts_with(std::env::temp_dir()), "{}", p.display());
        assert!(p.to_string_lossy().ends_with(".toml"));
        // The identifier is not pasted into a path as written.
        assert!(!p.to_string_lossy().contains(':'));

        // And two edits of one document never share a path: the process id alone would
        // let one silently take the other's buffer.
        let again = scratch_path(&Target::Machine("98:fa:9b:50:d8:10".to_string()), "toml");
        assert_ne!(p, again);
    }

    #[test]
    fn quitting_without_changing_anything_writes_nothing() {
        let dir = scratch("noop");
        let (answers, store) = subject(&dir);
        let target = Target::Machine("98fa9b50d810".to_string());
        guard::write(&answers, store.as_ref(), &target, "toml", Some("x = 1\n"));

        let path = dir.join("98fa9b50d810/proxmox.toml");
        let before_mtime = std::fs::metadata(&path)
            .expect("meta")
            .modified()
            .expect("mtime");

        let out = round_trip(&answers, store.as_ref(), &target, "toml", "x = 1\n", |_| {
            Ok(())
        });
        assert!(matches!(out, Edited::Unchanged), "{out:?}");
        assert_eq!(
            std::fs::metadata(&path)
                .expect("meta")
                .modified()
                .expect("mtime"),
            before_mtime,
            "an unchanged buffer must not touch the document"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edit_goes_through_the_guard_and_keeps_the_operators_filename() {
        let dir = scratch("stored");
        let (answers, store) = subject(&dir);
        let target = Target::Machine("98fa9b50d810".to_string());

        // A document the operator named themselves, which must survive the write.
        std::fs::create_dir_all(dir.join("98fa9b50d810")).expect("dir");
        std::fs::write(dir.join("98fa9b50d810/theirs.toml"), "x = 1\n").expect("write");

        let out = round_trip(&answers, store.as_ref(), &target, "toml", "x = 1\n", |p| {
            std::fs::write(p, "x = 2\n").map_err(|e| e.to_string())
        });
        assert!(matches!(out, Edited::Stored(_)), "{out:?}");

        assert!(
            dir.join("98fa9b50d810/theirs.toml").exists(),
            "the operator's own filename must survive"
        );
        assert!(
            !dir.join("98fa9b50d810/proxmox.toml").exists(),
            "and no second document should appear beside it"
        );
        let body = std::fs::read_to_string(dir.join("98fa9b50d810/theirs.toml")).expect("read");
        assert!(body.contains("x = 2"), "{body}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rollback is what stands between a keystroke and a rack that cannot install.
    #[test]
    fn an_edit_that_would_break_the_answer_set_is_refused_and_put_back() {
        let dir = scratch("refused");
        let (answers, store) = subject(&dir);
        let target = Target::Machine("98fa9b50d810".to_string());
        guard::write(&answers, store.as_ref(), &target, "toml", Some("x = 1\n"));

        let out = round_trip(&answers, store.as_ref(), &target, "toml", "x = 1\n", |p| {
            std::fs::write(p, "extends = \"nowhere\"\n").map_err(|e| e.to_string())
        });
        match &out {
            Edited::Refused(introduced) => {
                assert!(!introduced.is_empty(), "it must say what broke")
            }
            other => panic!("expected a refusal, got {other:?}"),
        }

        let body = std::fs::read_to_string(dir.join("98fa9b50d810/proxmox.toml")).expect("read");
        assert!(
            body.contains("x = 1"),
            "the rollback did not restore it: {body}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An editor that died must not be treated as an empty document.
    #[test]
    fn an_editor_that_fails_writes_nothing() {
        let dir = scratch("died");
        let (answers, store) = subject(&dir);
        let target = Target::Machine("98fa9b50d810".to_string());
        guard::write(&answers, store.as_ref(), &target, "toml", Some("x = 1\n"));

        let out = round_trip(&answers, store.as_ref(), &target, "toml", "x = 1\n", |p| {
            std::fs::write(p, "").expect("truncate");
            Err("vi was killed".to_string())
        });
        assert!(matches!(out, Edited::Failed(_)), "{out:?}");
        let body = std::fs::read_to_string(dir.join("98fa9b50d810/proxmox.toml")).expect("read");
        assert!(body.contains("x = 1"), "{body}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// It holds a root password hash: it must not be left in the temporary directory.
    #[test]
    fn the_scratch_copy_is_removed_whatever_happens() {
        let dir = scratch("cleanup");
        let (answers, store) = subject(&dir);
        let target = Target::Machine("98fa9b50d810".to_string());
        let mut seen = PathBuf::new();

        let out = round_trip(&answers, store.as_ref(), &target, "toml", "x = 1\n", |p| {
            seen = p.to_path_buf();
            Err("boom".to_string())
        });
        assert!(matches!(out, Edited::Failed(_)));
        assert!(!seen.exists(), "{} was left behind", seen.display());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
