//! An optional configuration file in TOML, named by `RESCRIPTUM_CONFIG`.
//!
//! The same job as [`crate::envfile`], in a shape meant to be read. It exists for one
//! platform and one act: somebody editing the file by hand on a NAS, in File Station or
//! over SMB, where `RESCRIPTUM_ANSWERS_DIR=/volume1/…` is a poor thing to hand a person.
//! The name misleads too — people read "environment variable" and go looking for a shell
//! to export it in, when on DSM it has been a file all along.
//!
//! **The configuration is still the same settings.** This module maps a document onto
//! their `RESCRIPTUM_*` names and does nothing else: every value reaches
//! `Config::from_lookup` under the key it would have had in the environment, so there is
//! exactly one place that decides what a setting *means*, and this format cannot grow
//! behaviour the environment does not have. That is what keeps two configuration files
//! from becoming two configurations.
//!
//! The rules are `envfile`'s, deliberately unchanged:
//!
//! * **Named, never discovered.** There is no `./rescriptum.toml`. This binary runs as
//!   root; a file picked up from whatever directory it was launched in would hand
//!   `admin.token` — and therefore the root password of every machine installed
//!   afterwards — to anyone who could write there.
//! * **The real environment wins.** Both files supply defaults.
//! * **A file that was asked for and cannot be read is a startup error**, never a
//!   warning. The silent path is what these files exist to remove.
//!
//! And one rule the second format adds: **where both files set the same thing, this one
//! wins**, because a deployment that names both is moving *to* TOML rather than sitting
//! between the two. `Config::from_env` says so out loud at startup rather than quietly
//! picking one.
//!
//! Unlike the env file this one has escapes, so no value has to be refused for what it
//! contains — a token with a `#` in it, a title with a quote, and a path with a space are
//! all writable and all read back unchanged.

use std::collections::BTreeMap;
use std::path::PathBuf;
use toml_edit::{DocumentMut, Item, Table, Value};

/// The variable that points at the file. Deliberately the only way in.
pub const CONFIG_FILE: &str = "RESCRIPTUM_CONFIG";

/// One setting, under both of its names.
pub struct Mapped {
    /// What it is called everywhere else in this program.
    pub key: &'static str,
    /// Where it lives in the document, dotted. One or two segments; the table is the
    /// grouping the environment cannot express.
    pub path: &'static str,
    /// Written as a bare integer rather than a quoted string, when the value is one.
    /// `workers = 2` is the point of the format; `workers = "2"` reads like a mistake.
    pub numeric: bool,
}

/// Every setting, in the document's own order.
///
/// **The names shed the `RESCRIPTUM_` prefix and gain tables**, which was the open
/// question in the plan and is settled here by what the file is for: it exists to be
/// read, `answers_dir` reads better than `RESCRIPTUM_ANSWERS_DIR`, and a table is the
/// only thing that says `store.kind` and `store.db_path` belong together. The
/// one-to-one mapping with the environment lives in this table instead of in the
/// spelling, which is the right place for it — `config --value` and the panel both go
/// through it, so nobody has to hold two names in their head.
pub const MAPPING: [Mapped; 31] = [
    Mapped {
        key: "RESCRIPTUM_ANSWERS_DIR",
        path: "answers_dir",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_LISTEN_ADDR",
        path: "listen_addr",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_LOG",
        path: "log",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_LOG_FILE",
        path: "log_file",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_PUBLIC_HOST",
        path: "public_host",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_USER",
        path: "user",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_GROUP",
        path: "group",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_STORE",
        path: "store.kind",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_DB_PATH",
        path: "store.db_path",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_WORKERS",
        path: "server.workers",
        numeric: true,
    },
    Mapped {
        key: "RESCRIPTUM_MAX_CONNECTIONS",
        path: "server.max_connections",
        numeric: true,
    },
    Mapped {
        key: "RESCRIPTUM_TIMEOUT_SECS",
        path: "server.timeout_secs",
        numeric: true,
    },
    Mapped {
        key: "RESCRIPTUM_ADMIN_ADDR",
        path: "admin.addr",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_ADMIN_TOKEN",
        path: "admin.token",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_ANSWER_TOKEN",
        path: "answer.token",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_CAPTURE_DIR",
        path: "answer.capture_dir",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_CONTROLLERS_FILE",
        path: "power.controllers_file",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_MEDIA_DIR",
        path: "media.dir",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_MEDIA_ADDR",
        path: "media.addr",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_MEDIA_TIMEOUT_SECS",
        path: "media.timeout_secs",
        numeric: true,
    },
    Mapped {
        key: "RESCRIPTUM_MEDIA_MAX_CONNECTIONS",
        path: "media.max_connections",
        numeric: true,
    },
    Mapped {
        key: "RESCRIPTUM_BOOT_DIR",
        path: "boot.dir",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_BOOT_ALLOW",
        path: "boot.allow",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_BOOT_UNCLAIMED",
        path: "boot.unclaimed",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_BOOT_TIMEOUT_SECS",
        path: "boot.timeout_secs",
        numeric: true,
    },
    Mapped {
        key: "RESCRIPTUM_BOOT_LOGO",
        path: "boot.logo",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_BOOT_TITLE",
        path: "boot.title",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_TFTP_ADDR",
        path: "tftp.addr",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_TFTP_PORT_RANGE",
        path: "tftp.port_range",
        numeric: false,
    },
    Mapped {
        key: "RESCRIPTUM_TFTP_BLKSIZE",
        path: "tftp.blksize",
        numeric: true,
    },
    // Its own table rather than a line in `[boot]` or `[answer]`, because it is neither:
    // the token is Proxmox's, it arrives on the answer listener, and what it changes is a
    // boot claim. `POST /installed` is the thing being configured, and the section is
    // named after it.
    Mapped {
        key: "RESCRIPTUM_INSTALLED_TOKEN",
        path: "installed.token",
        numeric: false,
    },
];

/// The document's name for a setting, or `None` if this program does not read it.
pub fn path_for(key: &str) -> Option<&'static str> {
    MAPPING.iter().find(|m| m.key == key).map(|m| m.path)
}

fn mapped_path(path: &str) -> Option<&'static Mapped> {
    MAPPING.iter().find(|m| m.path == path)
}

/// The environment name for a setting written the way the document writes it. Used to
/// answer somebody who typed `config set answers_dir=…` with the name that works, rather
/// than with "not a setting this program reads" — they are looking at a file where that
/// *is* the name.
pub fn key_for(path: &str) -> Option<&'static str> {
    mapped_path(path).map(|m| m.key)
}

fn mapped_key(key: &str) -> Option<&'static Mapped> {
    MAPPING.iter().find(|m| m.key == key)
}

/// A loaded file: the settings it makes, under their environment names, and anything
/// worth saying about it out loud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TomlFile {
    pub path: PathBuf,
    vars: BTreeMap<String, String>,
    /// Reported at startup. Never contains a value — this file holds the admin token.
    pub warnings: Vec<String>,
}

impl TomlFile {
    /// Read and parse the file, or say why not.
    pub fn load(path: impl Into<PathBuf>) -> Result<TomlFile, String> {
        let path = path.into();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{CONFIG_FILE}={} cannot be read: {e}", path.display()))?;

        let (vars, mut warnings) = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        for warning in &mut warnings {
            *warning = format!("{}: {warning}", path.display());
        }
        if let Some(mode) = crate::envfile::readable_by_others(&path) {
            warnings.push(format!(
                "{} is mode {mode:04o} — it may hold admin.token, so chmod 600 it",
                path.display()
            ));
        }

        Ok(TomlFile {
            path,
            vars,
            warnings,
        })
    }

    /// By environment name, which is the only name the rest of the program knows.
    pub fn get(&self, key: &str) -> Option<String> {
        self.vars.get(key).cloned()
    }

    pub fn len(&self) -> usize {
        self.vars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }
}

/// Parse a document into settings under their environment names, plus what to say about
/// it.
///
/// A key this program does not read is a **warning naming it**, not an error: a typo has
/// to be visible — silently ignoring `answer_dir` is how somebody spends an afternoon
/// wondering why their answers directory moved back — but refusing to start over one
/// would take a fleet down for a spelling mistake. A *duplicate* key needs no rule here:
/// TOML forbids it, so the parser refuses the file before this function sees it, which is
/// the same answer `envfile::parse` gives by hand.
pub fn parse(text: &str) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| e.to_string().replace('\n', " "))?;

    let mut vars = BTreeMap::new();
    let mut warnings = Vec::new();
    walk(doc.as_table(), "", &mut vars, &mut warnings)?;
    Ok((vars, warnings))
}

fn walk(
    table: &Table,
    prefix: &str,
    vars: &mut BTreeMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    for (key, item) in table.iter() {
        let path = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        match item {
            Item::Table(inner) => walk(inner, &path, vars, warnings)?,
            Item::Value(Value::InlineTable(inner)) => {
                // `store = { kind = "sqlite" }` is the same configuration written on one
                // line. Reading it costs nothing and refusing it would be pedantry.
                for (key, value) in inner.iter() {
                    leaf(&format!("{path}.{key}"), value, vars, warnings)?;
                }
            }
            Item::Value(value) => leaf(&path, value, vars, warnings)?,
            Item::ArrayOfTables(_) => {
                warnings.push(format!("{path} is not a setting this program reads"));
            }
            Item::None => {}
        }
    }
    Ok(())
}

fn leaf(
    path: &str,
    value: &Value,
    vars: &mut BTreeMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let Some(mapped) = mapped_path(path) else {
        warnings.push(format!(
            "{path} is not a setting this program reads — check the spelling"
        ));
        return Ok(());
    };
    // A setting that exists but was given a list or a table is a mistake worth stopping
    // for: unlike a misspelled name it *was* aimed at something real, and carrying on
    // would serve the default while the file plainly says otherwise.
    let Some(rendered) = scalar(value) else {
        return Err(format!(
            "{path} takes one value, not {}",
            match value {
                Value::Array(_) => "a list",
                _ => "a table",
            }
        ));
    };
    // Empty is unset, decided here so that every reader agrees rather than each one
    // trimming for itself — and so that `unset`, which empties a line rather than
    // deleting it, means what it says all the way down to `len()`.
    if !rendered.trim().is_empty() {
        vars.insert(mapped.key.to_string(), rendered);
    }
    Ok(())
}

/// Every scalar becomes the text the environment would have carried, so a number written
/// as a number and a number written as a string mean the same thing — the file is for
/// people, and both are what a person writes.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.value().clone()),
        Value::Integer(i) => Some(i.value().to_string()),
        Value::Float(f) => Some(f.value().to_string()),
        Value::Boolean(b) => Some(b.value().to_string()),
        Value::Datetime(d) => Some(d.value().to_string()),
        Value::Array(_) | Value::InlineTable(_) => None,
    }
}

/// Apply changes to the **text** of a configuration file, leaving everything else exactly
/// as it is.
///
/// Keyed by environment name, like [`crate::envfile::rewrite`], so one caller can drive
/// either format. What differs is what the format allows: `toml_edit` edits the document
/// in place, so comments, ordering and spacing survive on their own rather than by
/// hand — and there is no value that cannot be written, because TOML has escapes.
///
/// `None` **empties a setting rather than deleting its line**, and that is deliberate:
/// removing the key would take the comment above it with it, and on a packaged install
/// those comments are the only documentation the configuration has. An empty value
/// already counts as unset everywhere else in this program — an exported-but-empty
/// variable is a mistake, not an instruction — so the file keeps saying what the setting
/// is while saying that nobody set it. A key that is not in the file at all stays absent.
pub fn rewrite(text: &str, changes: &BTreeMap<String, Option<String>>) -> Result<String, String> {
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|e| e.to_string().replace('\n', " "))?;

    for (key, change) in changes {
        let Some(mapped) = mapped_key(key) else {
            return Err(format!("{key} is not a setting this program reads"));
        };
        let segments: Vec<&str> = mapped.path.split('.').collect();
        let (last, tables) = segments
            .split_last()
            .expect("a path has at least one segment");

        match change {
            Some(value) => {
                let table = make_table(doc.as_table_mut(), tables, mapped.path)?;
                let mut new = render(value, mapped.numeric);
                // **Replace the value, never the entry.** A setting's explanation sits in
                // the *key's* decor, so inserting over an existing key throws away the
                // paragraph above it — which on a packaged install is the only
                // documentation the configuration has. Editing the value in place keeps
                // that, and keeps the spacing and any trailing comment with it.
                if let Some(Item::Value(existing)) = table.get_mut(last) {
                    *new.decor_mut() = existing.decor().clone();
                    *existing = new;
                } else {
                    table.insert(last, Item::Value(new));
                }
            }
            // Only a setting that is actually there means anything. Writing an empty
            // value for one nobody set would add noise rather than remove a setting.
            None => {
                if let Some(table) = find_table(doc.as_table_mut(), tables)
                    && let Some(Item::Value(existing)) = table.get_mut(last)
                {
                    let mut empty = Value::from("");
                    *empty.decor_mut() = existing.decor().clone();
                    *existing = empty;
                }
            }
        }
    }

    Ok(doc.to_string())
}

/// Walk to the table a setting lives in, creating what is missing. A new table is an
/// ordinary `[section]` rather than a dotted key, because this file is read by people.
fn make_table<'a>(
    mut table: &'a mut Table,
    segments: &[&str],
    path: &str,
) -> Result<&'a mut Table, String> {
    for segment in segments {
        let entry = table
            .entry(segment)
            .or_insert_with(|| Item::Table(Table::new()));
        table = entry.as_table_mut().ok_or_else(|| {
            format!("{segment} is not a table in this file, so {path} cannot be written")
        })?;
    }
    Ok(table)
}

/// The same walk, creating nothing: `None` as soon as a segment is missing or is not a
/// table.
fn find_table<'a>(mut table: &'a mut Table, segments: &[&str]) -> Option<&'a mut Table> {
    for segment in segments {
        table = table.get_mut(segment)?.as_table_mut()?;
    }
    Some(table)
}

/// A number written as a number, when the setting is one and the value is one. Anything
/// else is a string — including a numeric setting given something that is not a number,
/// which the file is entitled to hold and `from_lookup` is entitled to ignore, exactly as
/// it ignores `RESCRIPTUM_WORKERS=lots`.
fn render(value: &str, numeric: bool) -> Value {
    if numeric && let Ok(n) = value.trim().parse::<i64>() {
        return Value::from(n);
    }
    Value::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> BTreeMap<String, String> {
        parse(text).expect("parses").0
    }

    #[test]
    fn every_setting_is_mapped_exactly_once() {
        // The two tables are the same set under two spellings, and nothing else in the
        // program checks that. A setting missing here is one the file silently cannot
        // configure — the failure this format would otherwise ship with.
        for key in crate::envfile::KNOWN_KEYS {
            assert!(
                path_for(key).is_some(),
                "{key} has no place in the document"
            );
        }
        assert_eq!(MAPPING.len(), crate::envfile::KNOWN_KEYS.len());

        let mut paths: Vec<&str> = MAPPING.iter().map(|m| m.path).collect();
        paths.sort_unstable();
        let before = paths.len();
        paths.dedup();
        assert_eq!(before, paths.len(), "two settings share one path");
    }

    #[test]
    fn tables_become_environment_names() {
        let vars = parsed(
            r#"
            answers_dir = "/srv/answers"

            [store]
            kind = "sqlite"
            db_path = "/srv/answers.db"

            [server]
            workers = 2
            "#,
        );
        assert_eq!(
            vars.get("RESCRIPTUM_ANSWERS_DIR").map(String::as_str),
            Some("/srv/answers")
        );
        assert_eq!(
            vars.get("RESCRIPTUM_STORE").map(String::as_str),
            Some("sqlite")
        );
        assert_eq!(
            vars.get("RESCRIPTUM_DB_PATH").map(String::as_str),
            Some("/srv/answers.db")
        );
        // A number reaches `from_lookup` as the text the environment would have carried.
        assert_eq!(
            vars.get("RESCRIPTUM_WORKERS").map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn an_inline_table_is_the_same_configuration() {
        let vars = parsed(r#"store = { kind = "sqlite" }"#);
        assert_eq!(
            vars.get("RESCRIPTUM_STORE").map(String::as_str),
            Some("sqlite")
        );
    }

    #[test]
    fn a_dotted_key_is_the_same_configuration() {
        let vars = parsed(r#"store.kind = "sqlite""#);
        assert_eq!(
            vars.get("RESCRIPTUM_STORE").map(String::as_str),
            Some("sqlite")
        );
    }

    #[test]
    fn an_unknown_key_is_named_rather_than_ignored() {
        let (vars, warnings) = parse(
            r#"
            answer_dir = "/srv/answers"

            [store]
            knid = "sqlite"
            "#,
        )
        .expect("an unknown key does not stop the file being read");
        assert!(vars.is_empty(), "{vars:?}");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("answer_dir")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("store.knid")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_real_setting_given_a_list_is_an_error() {
        // Not a warning: unlike a misspelling this was aimed at something real, and
        // serving the default while the file plainly says otherwise is the silent
        // failure both file formats exist to remove.
        let e = parse("answers_dir = [\"/srv/answers\"]").expect_err("refused");
        assert!(e.contains("answers_dir"), "{e}");
        assert!(e.contains("not a list"), "{e}");
    }

    #[test]
    fn a_duplicate_key_is_refused_by_the_format_itself() {
        let e = parse("answers_dir = \"/a\"\nanswers_dir = \"/b\"\n").expect_err("refused");
        assert!(e.contains("answers_dir"), "{e}");
    }

    #[test]
    fn a_value_keeps_its_hash_and_its_quotes() {
        // The env file has to refuse both of these, because it has no escapes. This one
        // does, which is most of why it is nicer to edit by hand.
        let vars = parsed("[admin]\ntoken = \"a#b'c\\\"d\"\n");
        assert_eq!(
            vars.get("RESCRIPTUM_ADMIN_TOKEN").map(String::as_str),
            Some("a#b'c\"d")
        );
    }

    #[test]
    fn writing_keeps_the_comments_that_document_the_file() {
        let text = "# Where answers come from.\nanswers_dir = \"/srv/answers\"  # the share\n";
        let changes = BTreeMap::from([(
            "RESCRIPTUM_ANSWERS_DIR".to_string(),
            Some("/volume1/rescriptum/answers".to_string()),
        )]);
        let out = rewrite(text, &changes).expect("rewrites");
        assert!(out.contains("# Where answers come from."), "{out}");
        assert!(out.contains("/volume1/rescriptum/answers"), "{out}");
        assert!(out.contains("# the share"), "{out}");
        assert_eq!(
            parsed(&out)
                .get("RESCRIPTUM_ANSWERS_DIR")
                .map(String::as_str),
            Some("/volume1/rescriptum/answers")
        );
    }

    #[test]
    fn writing_creates_the_table_a_setting_lives_in() {
        let out = rewrite(
            "",
            &BTreeMap::from([("RESCRIPTUM_STORE".to_string(), Some("sqlite".to_string()))]),
        )
        .expect("rewrites");
        assert!(out.contains("[store]"), "{out}");
        assert_eq!(
            parsed(&out).get("RESCRIPTUM_STORE").map(String::as_str),
            Some("sqlite")
        );
    }

    #[test]
    fn a_numeric_setting_is_written_as_a_number() {
        let out = rewrite(
            "",
            &BTreeMap::from([("RESCRIPTUM_WORKERS".to_string(), Some("2".to_string()))]),
        )
        .expect("rewrites");
        assert!(out.contains("workers = 2"), "{out}");
        // And one that is not a number is still writable, and still ignored later —
        // exactly as `RESCRIPTUM_WORKERS=lots` is.
        let out = rewrite(
            "",
            &BTreeMap::from([("RESCRIPTUM_WORKERS".to_string(), Some("lots".to_string()))]),
        )
        .expect("rewrites");
        assert!(out.contains("workers = \"lots\""), "{out}");
    }

    #[test]
    fn unsetting_empties_the_line_and_keeps_its_paragraph() {
        let text = "# The admin API's bearer token.\n[admin]\ntoken = \"secret\"\n";
        let out = rewrite(
            text,
            &BTreeMap::from([("RESCRIPTUM_ADMIN_TOKEN".to_string(), None)]),
        )
        .expect("rewrites");
        assert!(out.contains("# The admin API's bearer token."), "{out}");
        assert!(out.contains("token = \"\""), "{out}");
        assert!(!out.contains("secret"), "{out}");
        // Empty is unset, the same way an exported-but-empty variable is.
        assert!(!parsed(&out).contains_key("RESCRIPTUM_ADMIN_TOKEN"));
    }

    #[test]
    fn unsetting_something_nobody_set_writes_nothing() {
        let out = rewrite(
            "answers_dir = \"/srv/answers\"\n",
            &BTreeMap::from([("RESCRIPTUM_ADMIN_TOKEN".to_string(), None)]),
        )
        .expect("rewrites");
        assert_eq!(out, "answers_dir = \"/srv/answers\"\n");
    }

    #[test]
    fn an_empty_value_counts_as_unset() {
        let vars = parsed("answers_dir = \"\"\n[admin]\ntoken = \"  \"\n");
        // Kept out at this level so every reader agrees, rather than each one trimming.
        assert!(!vars.contains_key("RESCRIPTUM_ANSWERS_DIR"), "{vars:?}");
        assert!(!vars.contains_key("RESCRIPTUM_ADMIN_TOKEN"), "{vars:?}");
    }
}
