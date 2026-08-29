//! What the test suites share. Today: writing an answer document where the layout keeps
//! it.
//!
//! One copy rather than one per suite, for the reason `loaders.rs` is one table read by
//! both TFTP and the DHCP snippet — four copies of a mapping are four chances for a
//! fixture to land somewhere the server does not look, and a test that seeds nothing
//! passes for the wrong reason.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// Seed one answer document from the way a test names it.
///
/// `98fa9b50d810.toml` is that machine as Proxmox, `groups/rack-a.toml` is a group, and
/// `default.toml` is the fallback — the vocabulary the tests were written in, and the
/// vocabulary an operator uses. Where it actually lands is the store's business, so this
/// goes through `StoreWrite` rather than reimplementing the mapping: a fixture then sits
/// exactly where a write from the admin API would put it, and cannot drift from it.
///
/// A name the store would refuse — an extension nobody serves — is written literally
/// where it was asked for. Those fixtures exist precisely to prove a stray file answers
/// nothing, and rewriting them would take the point away.
pub fn seed(root: &Path, name: &str, body: &str) {
    use rescriptum::store::{FileStore, StoreWrite};

    let named = Path::new(name);
    let stem = named
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let ext = named
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let in_groups = named
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|p| p.to_str())
        == Some(rescriptum::store::file::GROUPS_DIR);

    if rescriptum::format::Kind::for_extension(ext).is_some() && !stem.is_empty() {
        let store = FileStore::new(root);
        let written = if in_groups {
            store.put_group(stem, ext, body)
        } else if stem.eq_ignore_ascii_case(rescriptum::store::file::DEFAULT_DIR) {
            store.put_default(ext, body)
        } else {
            store.put_machine(stem, ext, body)
        };
        if written.is_ok() {
            return;
        }
    }

    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture directory");
    }
    fs::write(&path, body).expect("fixture");
}

/// Where `seed` put one, so a test can edit or remove it afterwards.
pub fn document_path(root: &Path, name: &str) -> std::path::PathBuf {
    let named = Path::new(name);
    let stem = named
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let ext = named
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let parent = named.parent().filter(|p| !p.as_os_str().is_empty());
    let identity = match parent {
        Some(p) => root.join(p).join(stem),
        None => root.join(stem),
    };
    identity.join(format!("{}.{ext}", rescriptum::format::canonical_stem(ext)))
}
