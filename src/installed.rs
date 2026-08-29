//! A machine reporting that it finished installing, and the claim on it being dropped.
//!
//! ## The loop this exists to close
//!
//! A machine is claimed for installation by an `.ipxe` answer named after it: the loader
//! asks `/ipxe/boot?mac=…`, gets a script that boots an installer, and installs. Then it
//! reboots — and if netboot is still first in its firmware's order, it arrives back at the
//! same question, gets the same script, and installs again. Forever, wiping the disk each
//! time.
//!
//! Every provisioning system answers this the same way: a machine is *armed* for install,
//! and something disarms it afterwards. The only question is who. Doing it by hand works
//! and is what this project shipped first, but it is a race — the machine reboots the
//! moment the installer finishes, so the window to rename a file is however long the
//! firmware takes to come back.
//!
//! **The machine is the one that knows.** Proxmox's answer format has a
//! `[post-installation-webhook]`, called with a JSON body **after a successful install and
//! before the reboot**, and that body carries the network interfaces — MAC addresses
//! included. So it is the same shape as the request that asked for the answer in the first
//! place, and `Facts` reads it with no new parsing at all.
//!
//! ## What it will and will not touch
//!
//! Narrow on purpose, because this is the one code path that changes the answer set in
//! response to something arriving over the network:
//!
//! - **Machine documents only.** Never a group, never a `default`. A group claims a whole
//!   rack, and one machine finishing its install must never be able to disarm its
//!   neighbours. This does not filter a resolution down to machines — it never looks at
//!   groups at all.
//! - **Format `ipxe` only.** That is the document that boots an installer. A machine's
//!   `.toml` is what the installer *reads once running*, and deleting it would take away
//!   the record of how the machine was built.
//! - **Moved, not deleted.** The document is re-put under an `installed-` prefix, which no
//!   longer matches the machine (the prefix is part of the normalized needle), and the
//!   original is removed. Re-arming is moving it back. Nothing is destroyed, which
//!   matters for a thing triggered by a network request.
//!
//!   With a directory per identity that prefix names a **sibling directory** —
//!   `installed-98-fa-9b-50-d8-10/boot.ipxe` — rather than a file inside the machine's
//!   own. That is deliberate twice over: the machine's directory stays the machine's
//!   configuration, and no new rule is needed to keep the disarmed document from
//!   answering, because it is the directory name that identifies a machine and this one
//!   identifies nothing.
//!
//! ## Off unless configured
//!
//! No token, no endpoint — not an open one, absent. The token is Proxmox's own
//! `auth-token`, which it puts in the body as a top-level `token` field rather than in a
//! header (unlike the answer token, which is a bearer). Compared in constant time, for the
//! same reason the admin API's is.

use crate::facts::Facts;
use crate::select::{Answers, normalize};
use crate::store::StoreWrite;
use std::io;

/// The prefix a disarmed document is moved under.
///
/// It has to be a **prefix**, not a suffix: matching is a substring test of the
/// normalized id against the normalized request, so `98fa9b50d810installed` would still
/// contain nothing the machine sends — but neither would a suffix survive somebody adding
/// a second one. A prefix reads as a state in a directory listing, which is what an
/// operator wants when they come to re-arm it.
pub const DISARMED: &str = "installed-";

/// What was done, for the log line and the response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disarmed {
    /// The machine documents that were claiming this machine, and where each went.
    pub moved: Vec<(String, String)>,
}

impl Disarmed {
    pub fn describe(&self) -> String {
        if self.moved.is_empty() {
            "nothing was claiming it".to_string()
        } else {
            self.moved
                .iter()
                .map(|(from, to)| format!("{from} -> {to} (ipxe)"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

/// Whether the body's `token` field is the one configured, compared in constant time.
///
/// An ordinary `==` returns on the first differing byte, which hands the token over one
/// byte at a time to anyone who can time the responses. The admin API learned this
/// already; there is no reason for a second place to learn it again.
pub fn token_matches(body: &[u8], expected: &str) -> bool {
    let Some(found) = token_in(body) else {
        return false;
    };
    constant_time_eq(found.as_bytes(), expected.as_bytes())
}

/// The `token` field, read from the JSON body as an untyped value.
///
/// No derive, no struct: the same rule the answer path follows. Proxmox documents this
/// body's contents as liable to grow, and a type here would be an assumption about a
/// schema that is not ours.
fn token_in(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value.get("token")?.as_str().map(str::to_string)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Length is not secret — a token's length leaks from the request size anyway — but
    // the comparison still runs over the whole of the longer one so that an early return
    // never depends on content.
    let mut diff = (a.len() ^ b.len()) as u8;
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

/// Which machine documents claim this machine for installation.
///
/// The identity rule, and only the identity rule: a document whose normalized id appears
/// in what the machine sent. That is the same test selection uses, written out here
/// rather than borrowed, because borrowing `resolve` would bring groups, defaults and
/// format aliasing along with it — and every one of those is something this must not act
/// on.
fn claiming(answers: &Answers, facts: &Facts) -> io::Result<Vec<(String, String)>> {
    let haystack = facts.haystack();
    let mut out = Vec::new();
    for (id, format) in answers.machine_documents()? {
        if format != "ipxe" {
            continue;
        }
        let needle = normalize(id.as_bytes());
        // An empty needle would match everything, which is how one badly named document
        // disarms a fleet. It cannot happen through the file store — a name that
        // normalizes to nothing has no alphanumerics — but this is the wrong place to
        // rely on that.
        if !needle.is_empty() && haystack.contains(&needle) {
            out.push((id, format));
        }
    }
    Ok(out)
}

/// Drop the claim, having been told by the machine that it is installed.
///
/// Returns what moved. **Nothing matching is a success, not an error**: the webhook is
/// allowed to arrive twice, and the second time there is simply nothing left to do.
pub fn disarm(answers: &Answers, store: &dyn StoreWrite, facts: &Facts) -> io::Result<Disarmed> {
    let mut moved = Vec::new();
    for (id, format) in claiming(answers, facts)? {
        let body = read_machine(store, &id, &format)?;
        let Some(body) = body else { continue };
        let to = format!("{DISARMED}{id}");
        // **Put before delete.** If the put fails the machine stays armed and the caller
        // is told, which is the safe half of the failure: an install that happens twice
        // is recoverable, an answer document that vanished is not.
        store.put_machine(&to, &format, &body)?;
        store.delete_machine(&id, &format)?;
        moved.push((id, to));
    }
    Ok(Disarmed { moved })
}

fn read_machine(store: &dyn StoreWrite, id: &str, format: &str) -> io::Result<Option<String>> {
    Ok(store
        .snapshot()?
        .machines
        .into_iter()
        .find(|m| m.id == id && m.format == format)
        .map(|m| m.body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::file::FileStore;
    use std::sync::Arc;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rescriptum-installed-{}-{name}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// A webhook body the shape Proxmox documents: the interfaces, with their MACs.
    fn webhook(mac: &str) -> Vec<u8> {
        format!(
            r#"{{"token":"s3cr3t","fqdn":"node01.example.com",
                "network_interfaces":[{{"name":"eno1","mac":"{mac}"}}],
                "disks":[{{"path":"/dev/sda","size":512110190592}}]}}"#
        )
        .into_bytes()
    }

    /// Write one document where the layout keeps it: `<id>.<ext>` names the machine and
    /// the format, and the file inside its directory is named for us.
    fn document(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let named = std::path::Path::new(name);
        let (id, ext) = (
            named.file_stem().unwrap().to_str().unwrap(),
            named.extension().unwrap().to_str().unwrap(),
        );
        let dir = dir.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.{ext}", crate::format::canonical_stem(ext)));
        std::fs::write(&path, body).unwrap();
        path
    }

    fn answers_for(dir: &std::path::Path) -> (Answers, Arc<FileStore>) {
        let store = Arc::new(FileStore::new(dir));
        (Answers::new(store.clone()), store)
    }

    #[test]
    fn the_machine_that_reported_stops_being_claimed() {
        let dir = scratch("basic");
        document(&dir, "98-fa-9b-50-d8-10.ipxe", "#!ipxe\nchain x\n");
        document(&dir, "98-fa-9b-50-d8-10.toml", "[global]\n");
        let (answers, store) = answers_for(&dir);

        let facts = Facts::new(None, &webhook("98:fa:9b:50:d8:10"));
        let done = disarm(&answers, store.as_ref(), &facts).expect("disarm");
        assert_eq!(
            done.moved,
            vec![(
                "98-fa-9b-50-d8-10".to_string(),
                "installed-98-fa-9b-50-d8-10".to_string()
            )]
        );

        // The claim is gone…
        assert!(!dir.join("98-fa-9b-50-d8-10/boot.ipxe").exists());
        // …the document is not, and re-arming is moving it back.
        assert!(dir.join("installed-98-fa-9b-50-d8-10/boot.ipxe").exists());
        // **And the machine's own answer is untouched.** Deleting it would throw away the
        // record of how this machine was built, and the installer is the thing that reads
        // it — not the loader. It is also why the disarmed document goes to a directory
        // of its own: the machine's directory is still the machine's.
        assert!(dir.join("98-fa-9b-50-d8-10/proxmox.toml").exists());
    }

    #[test]
    fn a_disarmed_document_no_longer_claims_the_machine() {
        // The property the prefix exists for. Without it the rename is decoration and the
        // machine reinstalls anyway, which is the whole failure being fixed.
        let dir = scratch("nomatch");
        document(&dir, "98-fa-9b-50-d8-10.ipxe", "#!ipxe\n");
        let (answers, store) = answers_for(&dir);
        let facts = Facts::new(None, &webhook("98:fa:9b:50:d8:10"));

        disarm(&answers, store.as_ref(), &facts).expect("disarm");
        let (answers, store) = answers_for(&dir);
        let again = disarm(&answers, store.as_ref(), &facts).expect("second");
        assert!(
            again.moved.is_empty(),
            "the moved document still matches: {:?}",
            again.moved
        );
    }

    #[test]
    fn a_group_is_never_touched_however_it_matches() {
        // **The one that would be a disaster.** A group claims a rack; one machine
        // finishing its install must not disarm its neighbours. This is why the lookup
        // never consults groups rather than filtering them out afterwards.
        let dir = scratch("group");
        document(
            &dir.join("groups"),
            "rack-a.ipxe",
            "# answer: members = 98:fa:9b:50:d8:10\n#!ipxe\n",
        );
        std::fs::create_dir_all(dir.join("default")).unwrap();
        std::fs::write(dir.join("default/boot.ipxe"), "#!ipxe\n").unwrap();
        let (answers, store) = answers_for(&dir);

        let facts = Facts::new(None, &webhook("98:fa:9b:50:d8:10"));

        // **First prove the fixture bites.** Without this the assertion below passes for
        // a group that was never loaded, which would report a guarantee that does not
        // exist — the exact shape of test this project has been caught by before.
        let claimed = answers
            .resolve(&facts)
            .expect("resolve")
            .expect("the group must claim this machine");
        assert_eq!(claimed.group.as_deref(), Some("rack-a"));

        let done = disarm(&answers, store.as_ref(), &facts).expect("disarm");
        assert!(done.moved.is_empty(), "{:?}", done.moved);
        assert!(dir.join("groups/rack-a/boot.ipxe").exists());
        assert!(dir.join("default/boot.ipxe").exists());
    }

    #[test]
    fn a_machine_nothing_claims_is_not_an_error() {
        // The webhook may arrive twice, and a machine may have been installed from the
        // menu rather than from a claim. Neither is a failure.
        let dir = scratch("none");
        document(&dir, "aa-bb-cc-dd-ee-ff.ipxe", "#!ipxe\n");
        let (answers, store) = answers_for(&dir);
        let facts = Facts::new(None, &webhook("98:fa:9b:50:d8:10"));
        let done = disarm(&answers, store.as_ref(), &facts).expect("disarm");
        assert!(done.moved.is_empty());
        assert!(dir.join("aa-bb-cc-dd-ee-ff/boot.ipxe").exists());
    }

    #[test]
    fn the_token_is_read_from_the_body_and_compared_whole() {
        // Proxmox puts it in the JSON body as a top-level `token`, not in a header —
        // unlike the answer token, which is a bearer. Fetched from the wiki rather than
        // remembered, because getting this wrong is an endpoint nothing can authenticate.
        assert!(token_matches(&webhook("98:fa:9b:50:d8:10"), "s3cr3t"));
        assert!(!token_matches(&webhook("98:fa:9b:50:d8:10"), "s3cr3"));
        assert!(!token_matches(&webhook("98:fa:9b:50:d8:10"), "s3cr3t "));
        assert!(!token_matches(&webhook("98:fa:9b:50:d8:10"), ""));
        // Not JSON at all, and a body with no token: both are a refusal, never a pass.
        assert!(!token_matches(b"not json", "s3cr3t"));
        assert!(!token_matches(br#"{"fqdn":"x"}"#, "s3cr3t"));
    }

    #[test]
    fn constant_time_comparison_agrees_with_the_ordinary_one() {
        for (a, b) in [
            ("", ""),
            ("a", "a"),
            ("a", "b"),
            ("", "a"),
            ("a", ""),
            ("abcdef", "abcdeg"),
            ("abcdef", "abcdef"),
        ] {
            assert_eq!(
                constant_time_eq(a.as_bytes(), b.as_bytes()),
                a == b,
                "{a:?} vs {b:?}"
            );
        }
    }
}
