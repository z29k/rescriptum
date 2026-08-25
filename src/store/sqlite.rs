//! A SQLite-backed store, for deployments administered over the API rather than by
//! editing files.
//!
//! SQLite is compiled into the binary (`rusqlite`'s `bundled` feature), so this stays
//! a single static executable with nothing to install alongside it.

use super::{
    RawDefault, RawGroup, RawMachine, Snapshot, Store, StoreWrite, Version, invalid_format,
    invalid_id, valid_format, valid_id,
};
use rusqlite::Connection;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Bump when the schema changes, and add the step in `migrate`.
///
/// One, because nothing has ever been released: the shapes this went through while it was
/// being written never left the repository, so carrying migrations from them would be
/// carrying code that cannot run. The machinery stays — a newer database is still refused
/// rather than guessed at, which is what protects a rollback once there *is* something to
/// roll back to.
const SCHEMA_VERSION: i64 = 1;

/// `groups` is a keyword in SQLite (window functions), so the table is not called that.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS machines (
    id         TEXT NOT NULL,
    format     TEXT NOT NULL DEFAULT 'toml',
    body       TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (id, format)
);
CREATE TABLE IF NOT EXISTS answer_groups (
    name       TEXT NOT NULL,
    format     TEXT NOT NULL DEFAULT 'toml',
    body       TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (name, format)
);
CREATE TABLE IF NOT EXISTS settings (
    key        TEXT NOT NULL,
    format     TEXT NOT NULL DEFAULT 'toml',
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (key, format)
);
";

const DEFAULT_KEY: &str = "default";

pub struct SqliteStore {
    path: PathBuf,
    conn: Mutex<Connection>,
    /// Bumped on every write we make. `version` reads this instead of querying, so the
    /// per-request check costs an atomic load rather than a lock and a statement.
    /// A change made by some other process is picked up by the reload backstop, the
    /// same way an edited file's contents are.
    revision: AtomicU64,
}

fn to_io(e: rusqlite::Error) -> io::Error {
    io::Error::other(format!("sqlite: {e}"))
}

impl SqliteStore {
    pub fn open(path: impl Into<PathBuf>) -> io::Result<SqliteStore> {
        let path = path.into();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path).map_err(to_io)?;
        Self::prepare(&conn)?;
        Ok(SqliteStore {
            path,
            conn: Mutex::new(conn),
            revision: AtomicU64::new(0),
        })
    }

    /// An in-memory database, for tests.
    pub fn in_memory() -> io::Result<SqliteStore> {
        let conn = Connection::open_in_memory().map_err(to_io)?;
        Self::prepare(&conn)?;
        Ok(SqliteStore {
            path: PathBuf::from(":memory:"),
            conn: Mutex::new(conn),
            revision: AtomicU64::new(0),
        })
    }

    fn prepare(conn: &Connection) -> io::Result<()> {
        // WAL lets readers proceed while a write is in flight — the admin API must
        // never stall an install.
        let _: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .unwrap_or_else(|_| "unknown".to_string());
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
            .map_err(to_io)?;
        Self::migrate(conn)
    }

    /// Bring the database up to `SCHEMA_VERSION`.
    ///
    /// `user_version` 0 means "nothing here yet", so the current schema is simply
    /// created. A database left by an older binary is stepped forward one version at a
    /// time. A database from a *newer* binary is refused outright: guessing at a schema
    /// we do not know is how a fleet's configuration gets corrupted.
    fn migrate(conn: &Connection) -> io::Result<()> {
        let current: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(to_io)?;

        // Refused rather than guessed at: a database written by a newer binary may hold
        // columns this one would silently ignore, and silently ignoring part of an answer
        // set is how a machine gets installed wrongly.
        if current > SCHEMA_VERSION {
            return Err(io::Error::other(format!(
                "database schema is version {current}, this binary understands {SCHEMA_VERSION} \
                 — it was written by a newer rescriptum"
            )));
        }

        // `user_version` 0 means "nothing here yet", so the current schema is simply
        // created. When a second version exists, its step goes between here and the stamp
        // below, guarded by `if current < 2`.
        if current == 0 {
            conn.execute_batch(SCHEMA).map_err(to_io)?;
        }

        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
            .map_err(to_io)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A poisoned lock means a previous caller panicked mid-statement. SQLite's own
        // state is still consistent, so carry on rather than failing every install.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Every machine id currently stored, for `check` and the API's listings.
    pub fn machine_ids(&self) -> io::Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT id FROM machines ORDER BY id")
            .map_err(to_io)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(to_io)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(to_io)
    }
}

impl Store for SqliteStore {
    fn version(&self) -> Version {
        Some(self.revision.load(Ordering::Relaxed).to_string())
    }

    fn snapshot(&self) -> io::Result<Snapshot> {
        let conn = self.lock();
        let mut snapshot = Snapshot::default();

        let mut stmt = conn
            .prepare("SELECT id, format, body FROM machines ORDER BY id, format")
            .map_err(to_io)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(RawMachine {
                    id: r.get(0)?,
                    format: r.get(1)?,
                    body: r.get(2)?,
                })
            })
            .map_err(to_io)?;
        for row in rows {
            snapshot.machines.push(row.map_err(to_io)?);
        }

        let mut stmt = conn
            .prepare("SELECT name, format, body FROM answer_groups ORDER BY name, format")
            .map_err(to_io)?;
        let rows = stmt
            .query_map([], |r| {
                let name: String = r.get(0)?;
                Ok(RawGroup {
                    origin: format!("db:answer_groups/{name}"),
                    name,
                    format: r.get(1)?,
                    body: r.get(2)?,
                })
            })
            .map_err(to_io)?;
        for row in rows {
            snapshot.groups.push(row.map_err(to_io)?);
        }

        let mut stmt = conn
            .prepare("SELECT format, value FROM settings WHERE key = ?1 ORDER BY format")
            .map_err(to_io)?;
        let rows = stmt
            .query_map([DEFAULT_KEY], |r| {
                Ok(RawDefault {
                    format: r.get(0)?,
                    body: r.get(1)?,
                })
            })
            .map_err(to_io)?;
        for row in rows {
            snapshot.fallbacks.push(row.map_err(to_io)?);
        }

        Ok(snapshot)
    }

    fn describe(&self) -> String {
        format!("sqlite:{}", self.path.display())
    }
}

impl StoreWrite for SqliteStore {
    fn put_machine(&self, id: &str, format: &str, body: &str) -> io::Result<()> {
        if !valid_id(id) {
            return Err(invalid_id(id));
        }
        if !valid_format(format) {
            return Err(invalid_format(format));
        }
        self.lock()
            .execute(
                "INSERT INTO machines (id, format, body, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id, format) DO UPDATE SET body = ?3, updated_at = ?4",
                (id, format, body, crate::log::timestamp()),
            )
            .map_err(to_io)?;
        self.bump();
        Ok(())
    }

    fn delete_machine(&self, id: &str, format: &str) -> io::Result<bool> {
        if !valid_id(id) {
            return Err(invalid_id(id));
        }
        let n = self
            .lock()
            .execute(
                "DELETE FROM machines WHERE id = ?1 AND format = ?2",
                (id, format),
            )
            .map_err(to_io)?;
        self.bump();
        Ok(n > 0)
    }

    fn put_group(&self, name: &str, format: &str, body: &str) -> io::Result<()> {
        if !valid_id(name) {
            return Err(invalid_id(name));
        }
        if !valid_format(format) {
            return Err(invalid_format(format));
        }
        self.lock()
            .execute(
                "INSERT INTO answer_groups (name, format, body, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(name, format) DO UPDATE SET body = ?3, updated_at = ?4",
                (name, format, body, crate::log::timestamp()),
            )
            .map_err(to_io)?;
        self.bump();
        Ok(())
    }

    fn delete_group(&self, name: &str, format: &str) -> io::Result<bool> {
        if !valid_id(name) {
            return Err(invalid_id(name));
        }
        let n = self
            .lock()
            .execute(
                "DELETE FROM answer_groups WHERE name = ?1 AND format = ?2",
                (name, format),
            )
            .map_err(to_io)?;
        self.bump();
        Ok(n > 0)
    }

    fn put_default(&self, format: &str, body: &str) -> io::Result<()> {
        if !valid_format(format) {
            return Err(invalid_format(format));
        }
        self.lock()
            .execute(
                "INSERT INTO settings (key, format, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(key, format) DO UPDATE SET value = ?3, updated_at = ?4",
                (DEFAULT_KEY, format, body, crate::log::timestamp()),
            )
            .map_err(to_io)?;
        self.bump();
        Ok(())
    }

    fn delete_default(&self, format: &str) -> io::Result<bool> {
        let n = self
            .lock()
            .execute(
                "DELETE FROM settings WHERE key = ?1 AND format = ?2",
                (DEFAULT_KEY, format),
            )
            .map_err(to_io)?;
        self.bump();
        Ok(n > 0)
    }
}
