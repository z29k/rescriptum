//! A directory of TOML files — the original layout, and still the right one when
//! dropping a file onto a NAS is all the administration you need.

use super::{
    RawDefault, RawGroup, RawMachine, Snapshot, Store, StoreWrite, Version, invalid_format,
    invalid_id, valid_format, valid_id,
};
use crate::format::Kind;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Subdirectory holding group files. It is a subdirectory precisely so that groups are
/// never mistaken for machines by the filename match.
pub const GROUPS_DIR: &str = "groups";

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

    /// Write one document. Copies of the same stem in *other* formats are left alone:
    /// they are that machine's answer for a different operating system, not a stale
    /// duplicate.
    fn put(&self, dir: &Path, stem: &str, format: &str, body: &str) -> io::Result<()> {
        if !valid_format(format) {
            return Err(invalid_format(format));
        }
        write_atomic(&dir.join(format!("{stem}.{format}")), body)
    }

    fn remove(&self, dir: &Path, stem: &str, format: &str) -> io::Result<bool> {
        if !valid_format(format) {
            return Err(invalid_format(format));
        }
        remove_if_present(&dir.join(format!("{stem}.{format}")))
    }
}

/// Write via a temporary file and rename, so a reader never sees a half-written answer.
/// `rename` within a directory is atomic on POSIX.
fn write_atomic(path: &Path, body: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("toml.tmp.{}", std::process::id()));
    fs::write(&tmp, body)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn remove_if_present(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// The stem and format of a directory entry we can serve, or `None`.
fn answer_entry(entry: &fs::DirEntry, path: &Path) -> Option<(String, String)> {
    // `DirEntry::file_type` is free on Unix (it comes back with the readdir), while
    // `fs::metadata` is a stat syscall per entry. Only a symlink needs the stat, to
    // find out what it points at.
    let kind = entry.file_type().ok()?;
    let is_file = if kind.is_file() {
        true
    } else if kind.is_symlink() {
        fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
    } else {
        false
    };
    if !is_file {
        return None;
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Kind::for_extension(&ext)?;
    Some((path.file_stem()?.to_str()?.to_string(), ext))
}

impl Store for FileStore {
    fn version(&self) -> Version {
        // The directory's mtime moves whenever a file is added or removed. Editing a
        // file's *contents* does not move it — the reload backstop covers that.
        fs::metadata(&self.dir)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos().to_string())
    }

    fn snapshot(&self) -> io::Result<Snapshot> {
        let mut snapshot = Snapshot::default();

        // Groups first; a missing groups/ directory just means there are none.
        match fs::read_dir(self.dir.join(GROUPS_DIR)) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Some((name, format)) = answer_entry(&entry, &path) else {
                        continue;
                    };
                    match fs::read_to_string(&path) {
                        Ok(body) => snapshot.groups.push(RawGroup {
                            name,
                            format,
                            body,
                            origin: path.display().to_string(),
                        }),
                        Err(e) => snapshot.problems.push(format!("{}: {e}", path.display())),
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            // A NAS that has not been set up yet should not look different from one
            // with no matching file.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(snapshot),
            Err(e) => return Err(e),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Some((stem, format)) = answer_entry(&entry, &path) else {
                continue;
            };
            let body = match fs::read_to_string(&path) {
                Ok(body) => body,
                // One unreadable file must not fail every install.
                Err(e) => {
                    snapshot.problems.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            if stem.eq_ignore_ascii_case("default") {
                snapshot.fallbacks.push(RawDefault { format, body });
            } else {
                snapshot.machines.push(RawMachine {
                    id: stem,
                    format,
                    body,
                });
            }
        }

        Ok(snapshot)
    }

    fn describe(&self) -> String {
        format!("files:{}", self.dir.display())
    }
}

impl StoreWrite for FileStore {
    fn put_machine(&self, id: &str, format: &str, body: &str) -> io::Result<()> {
        // Checked here as well as at the API boundary: this is the layer that turns an
        // identifier into a path, so this is the layer that must not be fooled.
        if !valid_id(id) {
            return Err(invalid_id(id));
        }
        let dir = self.dir.clone();
        self.put(&dir, id, format, body)
    }

    fn delete_machine(&self, id: &str, format: &str) -> io::Result<bool> {
        if !valid_id(id) {
            return Err(invalid_id(id));
        }
        let dir = self.dir.clone();
        self.remove(&dir, id, format)
    }

    fn put_group(&self, name: &str, format: &str, body: &str) -> io::Result<()> {
        if !valid_id(name) {
            return Err(invalid_id(name));
        }
        let dir = self.dir.join(GROUPS_DIR);
        fs::create_dir_all(&dir)?;
        self.put(&dir, name, format, body)
    }

    fn delete_group(&self, name: &str, format: &str) -> io::Result<bool> {
        if !valid_id(name) {
            return Err(invalid_id(name));
        }
        let dir = self.dir.join(GROUPS_DIR);
        self.remove(&dir, name, format)
    }

    fn put_default(&self, format: &str, body: &str) -> io::Result<()> {
        let dir = self.dir.clone();
        self.put(&dir, "default", format, body)
    }

    fn delete_default(&self, format: &str) -> io::Result<bool> {
        let dir = self.dir.clone();
        self.remove(&dir, "default", format)
    }
}
