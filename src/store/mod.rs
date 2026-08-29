//! Where answer documents come from.
//!
//! The store is deliberately thin: it hands back raw TOML text and nothing else.
//! Everything that decides behaviour — matching, `extends` chains, merging, rendering,
//! `check` — lives above it in `select.rs` and `merge.rs`, and is shared by every
//! backend. That is what keeps two stores from drifting apart, and it is why the same
//! behavioural test suite runs against both (see `tests/stores.rs`).

use std::io;

/// One machine's own answer document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMachine {
    /// The identifier a request is matched against — a MAC, however written, or any
    /// name when the document selects itself with `match`.
    pub id: String,
    /// The document's format, as a file extension: `toml`, `yaml`, `ks`, …
    pub format: String,
    pub body: String,
    /// Where this came from, for diagnostics — a path, or a database URL. With a
    /// directory per identity the filename is the operator's to choose, so the
    /// identifier alone no longer says which document failed to parse.
    pub origin: String,
}

/// One group document. `members`, `extends` and `match` are read from the body itself,
/// so the store never has to understand them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawGroup {
    pub name: String,
    pub format: String,
    pub body: String,
    /// Where this came from, for diagnostics — a path, or a database URL.
    pub origin: String,
}

/// The document served when nothing else applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDefault {
    pub format: String,
    pub body: String,
    /// Where this came from, for diagnostics — a path, or a database URL.
    pub origin: String,
}

/// Everything needed to answer requests, as of one point in time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub machines: Vec<RawMachine>,
    pub groups: Vec<RawGroup>,
    /// The `default` documents — one per format, since a TOML default must not be
    /// handed to a client that asked for kickstart.
    pub fallbacks: Vec<RawDefault>,
    /// Problems the store itself hit (an unreadable file, a row that will not parse).
    /// Reported rather than raised: one bad entry must not stop every install.
    pub problems: Vec<String>,
}

/// A cheap token that changes whenever the contents might have. `None` means "cannot
/// tell", which callers treat as "always reload".
pub type Version = Option<String>;

/// Is this safe to use as a machine id or group name?
///
/// These arrive from URL paths in the admin API and are written out as **filenames** by
/// `export` and by the file store — so an id like `../../etc/passwd` would escape the
/// directory. Allow only what a MAC address or a rack name needs, and nothing that can
/// traverse.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// Names the file store keeps for itself, at the top of the answers directory.
///
/// A machine called `groups` would be a directory called `groups`, which is where the
/// groups live — so its documents would come back as a rack's. Refused in **both**
/// stores rather than only the one that has the problem: a database that accepted the
/// name would export into a directory that cannot represent it, and `export` has to
/// stay a way out.
pub const RESERVED_MACHINE_IDS: [&str; 2] = ["groups", "default"];

/// Is this usable as a machine id? `valid_id`, minus the names the layout reserves.
pub fn valid_machine_id(id: &str) -> bool {
    valid_id(id)
        && !RESERVED_MACHINE_IDS
            .iter()
            .any(|r| id.eq_ignore_ascii_case(r))
}

/// The error to return when `valid_machine_id` says no.
pub fn invalid_machine_id(id: &str) -> io::Error {
    if valid_id(id) {
        return io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{id:?} is reserved: the answers directory keeps {} for itself",
                RESERVED_MACHINE_IDS.join(" and ")
            ),
        );
    }
    invalid_id(id)
}

/// The error to return when `valid_id` says no.
pub fn invalid_id(id: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "{id:?} is not a usable identifier: use letters, digits, and - _ . : only, \
             and no path separators"
        ),
    )
}

pub trait Store: Send + Sync {
    /// Cheap enough to call on every request — this is what makes caching safe.
    fn version(&self) -> Version;

    /// Read everything. Called only when `version` moved, or the backstop expired.
    fn snapshot(&self) -> io::Result<Snapshot>;

    /// Human-readable description of where this store points, for logs and `check`.
    fn describe(&self) -> String;
}

/// The write half, used by the admin API. Separate from `Store` because serving
/// answers never needs it — a read-only deployment simply does not provide one.
/// A document is keyed by **what it is for**, which is a machine *and* an operating
/// system — one document is that machine as Proxmox, another is the same hardware as
/// Debian. They are two answers to two different questions and both may exist at once,
/// so every operation names a format.
pub trait StoreWrite: Store {
    fn put_machine(&self, id: &str, format: &str, body: &str) -> io::Result<()>;
    fn delete_machine(&self, id: &str, format: &str) -> io::Result<bool>;
    fn put_group(&self, name: &str, format: &str, body: &str) -> io::Result<()>;
    fn delete_group(&self, name: &str, format: &str) -> io::Result<bool>;
    fn put_default(&self, format: &str, body: &str) -> io::Result<()>;
    fn delete_default(&self, format: &str) -> io::Result<bool>;
}

/// Is this a format we can actually serve? Checked on write so a document in a format
/// nobody can read never reaches the store in the first place.
pub fn valid_format(format: &str) -> bool {
    crate::format::Kind::for_extension(format).is_some()
}

pub fn invalid_format(format: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "{format:?} is not a format this server can serve \
             (toml, yaml, yml, json, ign, xml, ks, cfg, preseed, seed, ipxe)"
        ),
    )
}

pub mod file;
pub use file::FileStore;

#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_identifiers_are_accepted() {
        for id in [
            "98-fa-9b-50-d8-10",
            "98:fa:9b:50:d8:10",
            "98fa9b50d810",
            "rack-a",
            "rack_a.2",
        ] {
            assert!(valid_id(id), "{id} should be usable");
        }
    }

    #[test]
    fn anything_that_could_traverse_is_refused() {
        // These reach the filesystem through `export` and the file store.
        for id in [
            "../../etc/passwd",
            "..",
            ".",
            "a/b",
            "a\\b",
            "",
            "with space",
            "sub/../dir",
            "nul\0byte",
        ] {
            assert!(!valid_id(id), "{id:?} must be refused");
        }
    }

    #[test]
    fn absurdly_long_identifiers_are_refused() {
        assert!(!valid_id(&"a".repeat(129)));
        assert!(valid_id(&"a".repeat(128)));
    }
}
