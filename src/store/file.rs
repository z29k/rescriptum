//! A directory of answer documents — the original layout, and still the right one when
//! dropping a file onto a NAS is all the administration you need.
//!
//! **One directory per identity.** A machine is a directory named after it, holding one
//! document per format; groups and the fallback are the same shape, under names the
//! layout reserves:
//!
//! ```text
//! answers/
//!   groups/
//!     rack-a/
//!       proxmox.toml
//!       debian.preseed
//!   default/
//!     proxmox.toml
//!   98-fa-9b-50-d8-10/
//!     proxmox.toml          the machine as Proxmox
//!     debian.preseed        the same hardware as Debian
//!     boot.ipxe             what boots the installer
//! ```
//!
//! The **extension decides the format**, and the stem decides nothing at all — it is
//! there for whoever opens the directory, so `proxmox.toml` and `answer.toml` are the
//! same document to this server. That is precisely why two documents of one format in
//! one directory is a *reported problem* rather than a silent choice: there would be no
//! rule to pick between them that an operator could have predicted.
//!
//! A document left at the top of the answers directory — the layout before this one —
//! is reported and **not served**. Half-reading an old layout would mean a machine whose
//! answer moved silently between two files, which is the failure this server exists to
//! make impossible.

use super::{
    RawDefault, RawGroup, RawMachine, Snapshot, Store, StoreWrite, Version, invalid_format,
    invalid_id, invalid_machine_id, valid_format, valid_id, valid_machine_id,
};
use crate::format::{Kind, canonical_stem};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Subdirectory holding the groups. A subdirectory precisely so that a group is never
/// mistaken for a machine, and reserved so that a machine cannot claim the name.
pub const GROUPS_DIR: &str = "groups";

/// Subdirectory holding the fallback documents — one per format, since a TOML default
/// must not be handed to a client that asked for kickstart.
pub const DEFAULT_DIR: &str = "default";

pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn new(dir: impl Into<PathBuf>) -> FileStore {
        FileStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Write one document into an identity's directory.
    ///
    /// A document of this format already there is **overwritten where it stands**,
    /// whatever it is called: the stem is the operator's choice, not ours, and adding a
    /// second file rather than replacing the first would leave two documents claiming
    /// one format. Only when there is none does the canonical name get used.
    ///
    /// Other formats in the same directory are left alone — they are that identity's
    /// answers for other operating systems, not stale duplicates.
    fn put(&self, dir: &Path, format: &str, body: &str) -> io::Result<()> {
        if !valid_format(format) {
            return Err(invalid_format(format));
        }
        self.create_identity_dir(dir)?;
        let path = existing(dir, format)
            .unwrap_or_else(|| dir.join(format!("{}.{format}", canonical_stem(format))));
        write_atomic(&path, body)
    }

    /// Create an identity's directory so that whoever owns the answers directory can
    /// still reach what lands inside it.
    ///
    /// `create_dir_all` alone uses the caller's umask, which is wrong for the same reason
    /// `write_atomic` preserving nothing was wrong: the process doing the writing is not
    /// necessarily the user the service runs as. A `0600` document is no use inside a
    /// directory the service cannot traverse — that is the same outage, arriving one
    /// restart later and looking like something else.
    fn create_identity_dir(&self, dir: &Path) -> io::Result<()> {
        if dir.is_dir() {
            return Ok(());
        }
        fs::create_dir_all(dir)?;
        inherit(dir, &self.dir);
        Ok(())
    }

    fn remove(&self, dir: &Path, format: &str) -> io::Result<bool> {
        if !valid_format(format) {
            return Err(invalid_format(format));
        }
        let Some(path) = existing(dir, format) else {
            return Ok(false);
        };
        let removed = remove_if_present(&path)?;
        // An identity with no documents left is not an identity. `remove_dir` refuses a
        // directory that still holds anything, which is exactly the test wanted — a
        // README or an answer in another format keeps it.
        let _ = fs::remove_dir(dir);
        Ok(removed)
    }

    fn machine_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    fn group_dir(&self, name: &str) -> PathBuf {
        self.dir.join(GROUPS_DIR).join(name)
    }

    fn default_dir(&self) -> PathBuf {
        self.dir.join(DEFAULT_DIR)
    }
}

/// Counter behind the temporary name. The process id alone disambiguates processes and
/// not threads, so two concurrent writes to one `(id, format)` in one process shared a
/// path.
static TMP_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Write via a temporary file and rename, so a reader never sees a half-written answer.
/// `rename` within a directory is atomic on POSIX.
fn write_atomic(path: &Path, body: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let existing = fs::metadata(path).ok();
    fs::write(&tmp, body)?;
    if let Err(e) = preserve(&tmp, existing.as_ref(), path.parent()) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Carry the old document's mode and ownership onto the new one.
///
/// `envfile::write_atomic` learned this first, and the reason is the same here only
/// sharper: **an answer document holds a root password hash and SSH keys.** A `0600`
/// document rewritten under a default umask comes back `0644`, and the thing that widened
/// it was a convenience. A document that did not exist gets `0600`, with the *containing
/// directory's* owner rather than the writer's — so a command run by hand as root still
/// leaves a document the service can read.
#[cfg(unix)]
fn preserve(tmp: &Path, existing: Option<&fs::Metadata>, parent: Option<&Path>) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let mode = existing.map_or(0o600, |m| m.permissions().mode() & 0o7777);
    fs::set_permissions(tmp, fs::Permissions::from_mode(mode))?;

    // Only root can give a file away, and only root needs to: anyone else is already
    // writing as the owner. A refusal here is therefore not an error.
    let owner = existing.map(|m| (m.uid(), m.gid())).or_else(|| {
        parent
            .and_then(|p| fs::metadata(p).ok())
            .map(|m| (m.uid(), m.gid()))
    });
    if let Some((uid, gid)) = owner {
        let _ = std::os::unix::fs::chown(tmp, Some(uid), Some(gid));
    }
    Ok(())
}

#[cfg(not(unix))]
fn preserve(
    _tmp: &Path,
    _existing: Option<&fs::Metadata>,
    _parent: Option<&Path>,
) -> io::Result<()> {
    Ok(())
}

/// Give a freshly created path the mode and ownership of one that is already right.
#[cfg(unix)]
fn inherit(path: &Path, model: &Path) {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let Ok(meta) = fs::metadata(model) else {
        return;
    };
    let mode = meta.permissions().mode() & 0o7777;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    let _ = std::os::unix::fs::chown(path, Some(meta.uid()), Some(meta.gid()));
}

#[cfg(not(unix))]
fn inherit(_path: &Path, _model: &Path) {}

fn remove_if_present(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// What a directory entry actually is, following a symlink to find out.
#[derive(PartialEq, Eq)]
enum What {
    Dir,
    File,
}

fn what(entry: &fs::DirEntry, path: &Path) -> Option<What> {
    // `DirEntry::file_type` is free on Unix (it comes back with the readdir), while
    // `fs::metadata` is a stat syscall per entry. Only a symlink needs the stat, to
    // find out what it points at.
    let kind = entry.file_type().ok()?;
    if kind.is_dir() {
        return Some(What::Dir);
    }
    if kind.is_file() {
        return Some(What::File);
    }
    if kind.is_symlink() {
        let target = fs::metadata(path).ok()?;
        if target.is_dir() {
            return Some(What::Dir);
        }
        if target.is_file() {
            return Some(What::File);
        }
    }
    None
}

/// The entry's name, unless it is hidden.
///
/// A hidden entry is never somebody's answer, and one kind is actively dangerous: macOS
/// writes an AppleDouble `._<name>` beside a file whose extended attributes the
/// filesystem will not take, and `._proxmox.toml` is a second `.toml` in the directory
/// with a body that is binary — so the machine it was meant to configure gets a parse
/// error instead of its answer. Found on a real NAS whose answers directory was being
/// edited over SMB from a Mac.
fn visible_name(entry: &fs::DirEntry) -> Option<String> {
    let name = entry.file_name();
    let name = name.to_str()?;
    (!name.starts_with('.')).then(|| name.to_string())
}

/// The same question without the allocation, for the paths that never need the name.
///
/// A full reload reads every directory in the answers directory, so this runs once per
/// entry per second at the backstop — a `String` per entry is a lot of allocation to do
/// on a NAS for a name nobody reads.
fn is_visible(entry: &fs::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .is_some_and(|name| !name.starts_with('.'))
}

/// The format this path declares, if it is one we can serve.
fn servable(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Kind::for_extension(&ext)?;
    Some(ext)
}

/// One document found on disk.
struct Found {
    format: String,
    body: String,
    path: PathBuf,
}

/// Every document in one identity's directory, at most one per format.
///
/// Sorted by filename, so which document wins a duplicated format never depends on
/// readdir order — and the loser is reported rather than quietly dropped.
fn documents_in(dir: &Path, problems: &mut Vec<String>) -> Vec<Found> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            problems.push(format!("{}: {e}", dir.display()));
            return Vec::new();
        }
    };

    // Name and format kept alongside the path: the sort is by filename, and asking the
    // path for its extension a second time inside the loop would be a second parse of
    // every entry.
    let mut candidates: Vec<(std::ffi::OsString, String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_visible(&entry) || what(&entry, &path) != Some(What::File) {
            continue;
        }
        // An extension we do not serve is not a mistake — a README beside the answers is
        // an ordinary thing to keep there.
        let Some(format) = servable(&path) else {
            continue;
        };
        candidates.push((entry.file_name(), format, path));
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));

    let mut found: Vec<Found> = Vec::new();
    for (_, format, path) in candidates {
        if let Some(first) = found.iter().find(|f| f.format == format) {
            problems.push(format!(
                "{}: {} already answers for .{format} here — the stem is only a name, so \
                 two of one format have no order between them; this one is ignored",
                path.display(),
                first.path.display()
            ));
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(body) => found.push(Found { format, body, path }),
            // One unreadable document must not fail every install.
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    found
}

/// A servable document sitting where the old flat layout put it.
///
/// Named, with the move spelled out: an operator meeting this is mid-upgrade, and the
/// one thing they need is the new path for this exact file.
fn stray(path: &Path, root: &Path) -> String {
    let to = destination(path)
        .map(|to| to.strip_prefix(root).unwrap_or(&to).display().to_string())
        .unwrap_or_default();
    format!(
        "{}: an answer is a directory now — move it to {to} \
         (`rescriptum migrate` moves them all); it is not being served",
        path.display(),
    )
}

/// A document still in the flat layout, and where the layout keeps it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Where a document named `<stem>.<ext>` belongs, given the directory it sits in.
///
/// One rule with no exceptions: `default.toml` is a stem like any other, and so is a
/// group's name. The whole layout is this function.
fn destination(path: &Path) -> Option<PathBuf> {
    let format = servable(path)?;
    let stem = path.file_stem()?.to_str()?;
    Some(
        path.parent()?
            .join(stem)
            .join(format!("{}.{format}", canonical_stem(&format))),
    )
}

/// Every document still lying flat in an answers directory, with its destination.
///
/// The store's own knowledge of where a document goes, so `migrate` cannot compute a
/// different answer from the one `snapshot` reads back — the two would then disagree
/// about whether a directory had been migrated at all.
pub fn pending_moves(dir: &Path) -> io::Result<Vec<Move>> {
    let mut moves = Vec::new();
    for from in [dir.to_path_buf(), dir.join(GROUPS_DIR)] {
        let entries = match fs::read_dir(&from) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_visible(&entry) || what(&entry, &path) != Some(What::File) {
                continue;
            }
            if let Some(to) = destination(&path) {
                moves.push(Move { from: path, to });
            }
        }
    }
    moves.sort_by(|a, b| a.from.cmp(&b.from));
    Ok(moves)
}

/// The identity directories inside one directory, sorted, plus anything left flat.
fn identities(dir: &Path, root: &Path, problems: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            problems.push(format!("{}: {e}", dir.display()));
            return Vec::new();
        }
    };

    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = visible_name(&entry) else {
            continue;
        };
        match what(&entry, &path) {
            Some(What::Dir) => found.push((name, path)),
            Some(What::File) if servable(&path).is_some() => {
                problems.push(stray(&path, root));
            }
            _ => {}
        }
    }
    found.sort();
    found
}

impl Store for FileStore {
    fn version(&self) -> Version {
        // The directory's mtime moves whenever an identity is added or removed. A
        // document appearing *inside* one moves only that directory's mtime, which
        // this does not see — the reload backstop is what covers that, as it already
        // did for a file whose contents changed under an unmoved name.
        fs::metadata(&self.dir)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos().to_string())
    }

    fn snapshot(&self) -> io::Result<Snapshot> {
        let mut snapshot = Snapshot::default();

        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            // A NAS that has not been set up yet should not look different from one
            // with no matching answer.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(snapshot),
            Err(e) => return Err(e),
        };

        let mut machines: Vec<(String, PathBuf)> = Vec::new();
        let mut groups: Option<PathBuf> = None;
        let mut fallbacks: Option<PathBuf> = None;

        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = visible_name(&entry) else {
                continue;
            };
            match what(&entry, &path) {
                Some(What::Dir) if name.eq_ignore_ascii_case(GROUPS_DIR) => groups = Some(path),
                Some(What::Dir) if name.eq_ignore_ascii_case(DEFAULT_DIR) => fallbacks = Some(path),
                Some(What::Dir) => machines.push((name, path)),
                Some(What::File) if servable(&path).is_some() => {
                    snapshot.problems.push(stray(&path, &self.dir));
                }
                _ => {}
            }
        }
        machines.sort();

        // Groups first; a missing groups/ directory just means there are none.
        if let Some(dir) = groups {
            for (name, path) in identities(&dir, &self.dir, &mut snapshot.problems) {
                for doc in documents_in(&path, &mut snapshot.problems) {
                    snapshot.groups.push(RawGroup {
                        name: name.clone(),
                        format: doc.format,
                        body: doc.body,
                        origin: doc.path.display().to_string(),
                    });
                }
            }
        }

        if let Some(dir) = fallbacks {
            for doc in documents_in(&dir, &mut snapshot.problems) {
                snapshot.fallbacks.push(RawDefault {
                    format: doc.format,
                    body: doc.body,
                    origin: doc.path.display().to_string(),
                });
            }
        }

        for (id, path) in machines {
            for doc in documents_in(&path, &mut snapshot.problems) {
                snapshot.machines.push(RawMachine {
                    id: id.clone(),
                    format: doc.format,
                    body: doc.body,
                    origin: doc.path.display().to_string(),
                });
            }
        }

        Ok(snapshot)
    }

    fn describe(&self) -> String {
        format!("files:{}", self.dir.display())
    }
}

/// The document of this format already in `dir`, if there is one.
///
/// Sorted, for the same reason `documents_in` sorts: with two of one format the one that
/// answers requests and the one a write replaces have to be the same file.
fn existing(dir: &Path, format: &str) -> Option<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !is_visible(&entry) || what(&entry, &path) != Some(What::File) {
            continue;
        }
        if servable(&path).is_some_and(|f| f == format) {
            paths.push(path);
        }
    }
    paths.sort();
    paths.into_iter().next()
}

impl StoreWrite for FileStore {
    fn put_machine(&self, id: &str, format: &str, body: &str) -> io::Result<()> {
        // Checked here as well as at the API boundary: this is the layer that turns an
        // identifier into a path, so this is the layer that must not be fooled.
        if !valid_machine_id(id) {
            return Err(invalid_machine_id(id));
        }
        self.put(&self.machine_dir(id), format, body)
    }

    fn delete_machine(&self, id: &str, format: &str) -> io::Result<bool> {
        if !valid_machine_id(id) {
            return Err(invalid_machine_id(id));
        }
        self.remove(&self.machine_dir(id), format)
    }

    fn put_group(&self, name: &str, format: &str, body: &str) -> io::Result<()> {
        if !valid_id(name) {
            return Err(invalid_id(name));
        }
        self.put(&self.group_dir(name), format, body)
    }

    fn delete_group(&self, name: &str, format: &str) -> io::Result<bool> {
        if !valid_id(name) {
            return Err(invalid_id(name));
        }
        self.remove(&self.group_dir(name), format)
    }

    fn put_default(&self, format: &str, body: &str) -> io::Result<()> {
        self.put(&self.default_dir(), format, body)
    }

    fn delete_default(&self, format: &str) -> io::Result<bool> {
        self.remove(&self.default_dir(), format)
    }
}
