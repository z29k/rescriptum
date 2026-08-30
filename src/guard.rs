//! A write that cannot leave the answer set broken.
//!
//! **This is the property the admin API is built on, and it is not an HTTP property.** A
//! cycle between groups, or a machine pointing at a group that no longer exists, does not
//! fail at write time — it fails when a rack tries to install. Catching it here is the
//! difference between a refusal now and a failed provisioning run at three in the morning.
//!
//! It lives outside `admin` because more than one thing needs it. The admin API maps each
//! outcome below onto a status code and a JSON envelope; a local editor maps them onto
//! what it shows an operator. Neither of them should own the rule.

use crate::select::Answers;
use crate::store::StoreWrite;

/// What is being written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Machine(String),
    Group(String),
    /// The fallback, which has no identifier of its own — only a format.
    Default,
}

impl Target {
    pub fn label(&self) -> &'static str {
        match self {
            Target::Machine(_) => "machine",
            Target::Group(_) => "group",
            Target::Default => "default",
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Target::Machine(id) | Target::Group(id) => id,
            Target::Default => "",
        }
    }
}

/// What a guarded write did.
///
/// Deliberately not a `Result`: "refused because it would break the answer set" is a
/// normal outcome with something to say, not an error, and flattening it into one would
/// lose the list of what broke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Written. `problems` is everything still wrong with the answer set — **including
    /// what was already wrong**, so a caller is not misled into thinking all is well just
    /// because their own write was clean.
    Stored {
        problems: Vec<String>,
    },
    Deleted {
        problems: Vec<String>,
    },
    /// It would have broken something, so it was put back. `introduced` is what would
    /// have broken — never the pre-existing problems, which are not this caller's doing.
    Refused {
        introduced: Vec<String>,
    },
    /// A delete of something that was not there.
    NotFound,
    /// The store refused the write itself: an identifier that would become a bad path, an
    /// unknown format. The caller's fault, and fixable by them.
    Rejected(String),
    /// The store could not be read. Not the caller's fault.
    Unavailable(String),
}

/// Apply a write, then check that it did not break the answer set.
///
/// **Both reads go back to the store.** The listing is cached behind the store's
/// `version`, and over the file store that version is the answers directory's mtime —
/// which does not move when a document is written *inside* an existing identity's
/// directory, and which a coarse-granularity filesystem may not move even for a new one
/// within the same second. Without forcing it, this would compare a write against itself,
/// find no difference, and keep something that breaks a rack.
///
/// Which side matters is worth stating: a stale `after` is a rollback that never runs and
/// fails **open**, while a stale `before` blames this write for a pre-existing problem and
/// fails **closed**. Both are forced, because the safe direction is cheap to buy twice.
pub fn write(
    answers: &Answers,
    store: &dyn StoreWrite,
    target: &Target,
    format: &str,
    body: Option<&str>,
) -> Outcome {
    answers.invalidate();
    let before = match answers.problems() {
        Ok(p) => p,
        Err(e) => return Outcome::Unavailable(e.to_string()),
    };

    // What was there before, so it can be put back.
    let previous = match store.snapshot() {
        Ok(s) => match target {
            Target::Machine(id) => s
                .machines
                .iter()
                .find(|m| &m.id == id && m.format == format)
                .map(|m| (m.format.clone(), m.body.clone())),
            Target::Group(name) => s
                .groups
                .iter()
                .find(|g| &g.name == name && g.format == format)
                .map(|g| (g.format.clone(), g.body.clone())),
            Target::Default => s
                .fallbacks
                .iter()
                .find(|d| d.format == format)
                .map(|d| (d.format.clone(), d.body.clone())),
        },
        Err(e) => return Outcome::Unavailable(e.to_string()),
    };

    let applied = match (target, body) {
        (Target::Machine(id), Some(b)) => store.put_machine(id, format, b).map(|()| true),
        (Target::Group(name), Some(b)) => store.put_group(name, format, b).map(|()| true),
        (Target::Default, Some(b)) => store.put_default(format, b).map(|()| true),
        (Target::Machine(id), None) => store.delete_machine(id, format),
        (Target::Group(name), None) => store.delete_group(name, format),
        (Target::Default, None) => store.delete_default(format),
    };
    let existed = match applied {
        Ok(v) => v,
        Err(e) => return Outcome::Rejected(e.to_string()),
    };

    if body.is_none() && !existed {
        return Outcome::NotFound;
    }

    answers.invalidate();
    let after = answers.problems().unwrap_or_default();
    let introduced: Vec<String> = after
        .iter()
        .filter(|p| !before.contains(p))
        .cloned()
        .collect();

    if !introduced.is_empty() {
        // Undo, so the store is never left in a state that breaks installs.
        let restored = match (target, &previous) {
            (Target::Machine(id), Some((f, b))) => store.put_machine(id, f, b),
            (Target::Group(name), Some((f, b))) => store.put_group(name, f, b),
            (Target::Default, Some((f, b))) => store.put_default(f, b),
            (Target::Machine(id), None) => store.delete_machine(id, format).map(drop),
            (Target::Group(name), None) => store.delete_group(name, format).map(drop),
            (Target::Default, None) => store.delete_default(format).map(drop),
        };
        if let Err(e) = restored {
            // Loud, because the consequence is otherwise silent: the store is now in a
            // state nobody asked for.
            crate::log::server(&format!(
                "could not roll back {} {:?}: {e} — the store may be inconsistent",
                target.label(),
                target.id()
            ));
        }
        return Outcome::Refused { introduced };
    }

    if body.is_some() {
        Outcome::Stored { problems: after }
    } else {
        Outcome::Deleted { problems: after }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FileStore;
    use std::sync::Arc;

    fn scratch(name: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rescriptum-guard-{}-{name}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn subject(dir: &std::path::Path) -> (Answers, Arc<FileStore>) {
        let store = Arc::new(FileStore::new(dir));
        let answers = Answers::new(Arc::new(FileStore::new(dir)));
        (answers, store)
    }

    #[test]
    fn a_clean_write_is_stored_and_reports_what_was_already_broken() {
        let dir = scratch("clean");
        let (answers, store) = subject(&dir);

        let out = write(
            &answers,
            store.as_ref(),
            &Target::Machine("98fa9b50d810".to_string()),
            "toml",
            Some("[global]\nkeyboard = \"fr\"\n"),
        );
        assert_eq!(out, Outcome::Stored { problems: vec![] });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The rollback, over the file store, inside the reload backstop** — which is the
    /// case the cached listing used to hide. Without `Answers::invalidate` this write
    /// compares equal to itself and is kept.
    #[test]
    fn a_write_that_would_break_the_answer_set_is_refused_and_rolled_back() {
        let dir = scratch("refused");
        let (answers, store) = subject(&dir);

        // A clean machine first, so the guard has a `before` with no problems.
        write(
            &answers,
            store.as_ref(),
            &Target::Machine("98fa9b50d810".to_string()),
            "toml",
            Some("[global]\nkeyboard = \"fr\"\n"),
        );

        // Now break it, immediately — well inside the backstop.
        let out = write(
            &answers,
            store.as_ref(),
            &Target::Machine("98fa9b50d810".to_string()),
            "toml",
            Some("extends = \"nowhere\"\n"),
        );
        match &out {
            Outcome::Refused { introduced } => {
                assert!(!introduced.is_empty(), "it must say what broke");
                assert!(
                    introduced.iter().any(|p| p.contains("nowhere")),
                    "{introduced:?}"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }

        // And the previous document is back, byte for byte.
        let body = std::fs::read_to_string(dir.join("98fa9b50d810/proxmox.toml")).expect("read");
        assert!(
            body.contains("keyboard"),
            "the rollback did not restore it: {body}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A write that *would have created* something is undone by deleting it, not by
    /// restoring a document that never existed.
    #[test]
    fn a_broken_first_write_leaves_nothing_behind() {
        let dir = scratch("first");
        let (answers, store) = subject(&dir);

        let out = write(
            &answers,
            store.as_ref(),
            &Target::Machine("98fa9b50d810".to_string()),
            "toml",
            Some("extends = \"nowhere\"\n"),
        );
        assert!(matches!(out, Outcome::Refused { .. }), "{out:?}");
        assert!(
            !dir.join("98fa9b50d810/proxmox.toml").exists(),
            "the rolled-back document is still there"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_something_that_is_not_there_is_not_found() {
        let dir = scratch("missing");
        let (answers, store) = subject(&dir);
        let out = write(
            &answers,
            store.as_ref(),
            &Target::Machine("98fa9b50d810".to_string()),
            "toml",
            None,
        );
        assert_eq!(out, Outcome::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_identifier_the_store_refuses_is_the_callers_fault() {
        let dir = scratch("bad-id");
        let (answers, store) = subject(&dir);
        let out = write(
            &answers,
            store.as_ref(),
            &Target::Machine("../escape".to_string()),
            "toml",
            Some("x = 1\n"),
        );
        assert!(matches!(out, Outcome::Rejected(_)), "{out:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
