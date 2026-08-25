//! An optional file of configuration defaults, named by `RESCRIPTUM_ENV_FILE`.
//!
//! The configuration is still environment variables — this only says where a set of them
//! may be read from, for the deployments that have nowhere good to put them. systemd has
//! `EnvironmentFile=` and needs none of this; **DSM 7 has no systemd**, so its Task
//! Scheduler entry has to be `. /volume1/netboot/rescriptum.env && exec …`, and that
//! fails *silently*: drop the `.`, mistype a line, or get the permissions wrong, and the
//! shell sources nothing while the server comes up on its defaults — wrong answers
//! directory, no admin token, not a word in the log. Reading the file here means a file
//! that cannot be read is a **startup error** instead.
//!
//! Three rules that are the whole point:
//!
//! * **Never discovered, only named.** There is no `./.env`. This binary runs as root; if
//!   it picked a file up from whatever directory it happened to be launched in, anyone
//!   who could write there would own `RESCRIPTUM_ADMIN_TOKEN` — and therefore the root
//!   password of every machine installed afterwards.
//! * **The real environment wins.** The file supplies defaults. Something exported
//!   deliberately at launch is never silently overridden by a file.
//! * **This is not a shell.** No `${}` expansion, no command substitution, no line
//!   continuation. A configuration file with shell semantics is a configuration file that
//!   surprises you, and this one holds a credential.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The variable that points at the file. Deliberately the only way in.
pub const ENV_FILE: &str = "RESCRIPTUM_ENV_FILE";

/// Every variable this program reads, so a typo can be reported rather than ignored.
pub const KNOWN_KEYS: [&str; 13] = [
    "RESCRIPTUM_STORE",
    "RESCRIPTUM_ANSWERS_DIR",
    "RESCRIPTUM_DB_PATH",
    "RESCRIPTUM_LISTEN_ADDR",
    "RESCRIPTUM_WORKERS",
    "RESCRIPTUM_MAX_CONNECTIONS",
    "RESCRIPTUM_TIMEOUT_SECS",
    "RESCRIPTUM_ANSWER_TOKEN",
    "RESCRIPTUM_ADMIN_ADDR",
    "RESCRIPTUM_ADMIN_TOKEN",
    "RESCRIPTUM_CAPTURE_DIR",
    "RESCRIPTUM_LOG",
    "RESCRIPTUM_LOG_FILE",
];

/// A loaded file: the values it set, and anything worth saying about it out loud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFile {
    pub path: PathBuf,
    vars: BTreeMap<String, String>,
    /// Reported at startup. Never contains a value — this file holds the admin token.
    pub warnings: Vec<String>,
}

impl EnvFile {
    /// Read and parse the file, or say why not.
    ///
    /// Unreadable is an error, not a shrug: the caller asked for this file by name, and
    /// carrying on with defaults is precisely the silent failure this exists to remove.
    pub fn load(path: impl Into<PathBuf>) -> Result<EnvFile, String> {
        let path = path.into();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{ENV_FILE}={} cannot be read: {e}", path.display()))?;

        let vars = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;

        let mut warnings = Vec::new();
        for key in vars.keys() {
            if key == ENV_FILE {
                // It is a variable this program reads — just not from here, since the
                // file has to be found before it can be parsed. Saying "not a variable
                // this program reads" would send the reader looking for a typo.
                warnings.push(format!(
                    "{}: {ENV_FILE} has no effect inside the file it names — it is read                      from the environment, before this file is opened",
                    path.display()
                ));
            } else if !KNOWN_KEYS.contains(&key.as_str()) {
                warnings.push(format!(
                    "{}: {key} is not a variable this program reads — check the spelling",
                    path.display()
                ));
            }
        }
        if let Some(mode) = readable_by_others(&path) {
            // A warning rather than a refusal, for the same reason a short answer token
            // warns: refusing to start costs someone an install, and the file may hold
            // nothing secret at all.
            warnings.push(format!(
                "{} is mode {mode:04o} — it may hold RESCRIPTUM_ADMIN_TOKEN, so chmod 600 it",
                path.display()
            ));
        }

        Ok(EnvFile {
            path,
            vars,
            warnings,
        })
    }

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

/// `0o600` and friends return `None`; anything a group or the world can read returns the
/// mode, so it can be named in the warning.
#[cfg(unix)]
fn readable_by_others(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then_some(mode)
}

#[cfg(not(unix))]
fn readable_by_others(_path: &Path) -> Option<u32> {
    None
}

/// `KEY=value` per line. `#` starts a comment **only at the start of a line**.
///
/// There are deliberately no inline comments: an unquoted `#` is part of the value. The
/// alternative is truncating a token at a `#` it legitimately contains, silently, which
/// is a far worse failure than a comment ending up in a value — where it is loud, because
/// the value then fails to parse as an address, a number or a store name.
pub fn parse(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut vars: BTreeMap<String, String> = BTreeMap::new();

    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `export KEY=value` is what a shell-sourced file looks like, and the same file
        // should work both ways rather than making you keep two of them.
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);

        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "line {}: expected KEY=value, found {line:?}",
                n + 1
            ));
        };
        let key = key.trim();
        if !is_valid_key(key) {
            return Err(format!(
                "line {}: {key:?} is not a usable variable name",
                n + 1
            ));
        }
        if vars.contains_key(key) {
            // In a hand-edited six-line file this is a mistake, and guessing which one
            // was meant is exactly the silent behaviour this module exists to avoid.
            return Err(format!("line {}: {key} is set twice", n + 1));
        }

        vars.insert(key.to_string(), unquote(value.trim()));
    }

    Ok(vars)
}

/// A quoted value keeps its inner whitespace; an unquoted one is trimmed. No escape
/// sequences are interpreted — `\n` in a token stays two characters, as it should.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' || first == b'\'') && first == last {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> BTreeMap<String, String> {
        parse(text).expect("should parse")
    }

    #[test]
    fn plain_assignments_are_read() {
        let vars = parsed("RESCRIPTUM_STORE=sqlite\nRESCRIPTUM_DB_PATH=/srv/answers.db\n");
        assert_eq!(vars["RESCRIPTUM_STORE"], "sqlite");
        assert_eq!(vars["RESCRIPTUM_DB_PATH"], "/srv/answers.db");
    }

    #[test]
    fn the_same_file_works_sourced_or_read() {
        // DSM sources it with `.`, which needs `export`; systemd's EnvironmentFile does
        // not want it. One file has to serve both.
        let vars = parsed("export RESCRIPTUM_STORE=sqlite\nRESCRIPTUM_LISTEN_ADDR=0.0.0.0:8000\n");
        assert_eq!(vars["RESCRIPTUM_STORE"], "sqlite");
        assert_eq!(vars["RESCRIPTUM_LISTEN_ADDR"], "0.0.0.0:8000");
    }

    #[test]
    fn whole_line_comments_and_blank_lines_are_ignored() {
        let vars = parsed("# the store\n\n   # indented\nRESCRIPTUM_STORE=files\n\n");
        assert_eq!(vars.len(), 1);
        assert_eq!(vars["RESCRIPTUM_STORE"], "files");
    }

    #[test]
    fn a_hash_inside_a_value_is_part_of_the_value() {
        // The alternative is truncating a token at a character it may well contain,
        // silently, which is the worse of the two failures by a distance.
        let vars = parsed("RESCRIPTUM_ADMIN_TOKEN=abc#def\n");
        assert_eq!(vars["RESCRIPTUM_ADMIN_TOKEN"], "abc#def");
    }

    #[test]
    fn quotes_are_stripped_and_protect_whitespace() {
        let vars = parsed(
            "RESCRIPTUM_ADMIN_TOKEN=\" spaced \"\nRESCRIPTUM_STORE='sqlite'\nRESCRIPTUM_DB_PATH=  /srv/a.db  \n",
        );
        assert_eq!(vars["RESCRIPTUM_ADMIN_TOKEN"], " spaced ");
        assert_eq!(vars["RESCRIPTUM_STORE"], "sqlite");
        assert_eq!(vars["RESCRIPTUM_DB_PATH"], "/srv/a.db");
    }

    #[test]
    fn nothing_is_expanded() {
        // A configuration file that runs shell semantics is one that surprises you, and
        // this one holds a credential.
        let vars = parsed("RESCRIPTUM_DB_PATH=$HOME/answers.db\nRESCRIPTUM_ANSWER_TOKEN=a${x}b\n");
        assert_eq!(vars["RESCRIPTUM_DB_PATH"], "$HOME/answers.db");
        assert_eq!(vars["RESCRIPTUM_ANSWER_TOKEN"], "a${x}b");
    }

    #[test]
    fn an_empty_value_is_allowed_and_stays_empty() {
        // `Config::from_lookup` treats it as unset, which is the documented behaviour of
        // an exported-but-empty variable too.
        let vars = parsed("RESCRIPTUM_ANSWER_TOKEN=\n");
        assert_eq!(vars["RESCRIPTUM_ANSWER_TOKEN"], "");
    }

    #[test]
    fn a_whitespace_only_value_is_the_same_as_an_empty_one() {
        // `Config` treats both as unset, the way it treats an exported-but-empty
        // variable. A token of three spaces is a mistake, not an instruction.
        let vars = parsed("RESCRIPTUM_ANSWER_TOKEN=    \n");
        assert_eq!(vars["RESCRIPTUM_ANSWER_TOKEN"], "");
    }

    #[test]
    fn a_line_that_is_not_an_assignment_is_an_error() {
        let e = parse("RESCRIPTUM_STORE sqlite\n").expect_err("no equals sign");
        assert!(e.contains("line 1"), "{e}");
        assert!(e.contains("KEY=value"), "{e}");
    }

    #[test]
    fn an_unusable_name_is_an_error() {
        for bad in ["9NAME=x", "a-b=x", "=x", " =x"] {
            assert!(parse(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn a_key_set_twice_is_an_error_rather_than_a_guess() {
        let e = parse("RESCRIPTUM_STORE=files\nRESCRIPTUM_STORE=sqlite\n").expect_err("duplicate");
        assert!(e.contains("line 2"), "{e}");
        assert!(e.contains("set twice"), "{e}");
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_shrug() {
        let e = EnvFile::load("/nonexistent/rescriptum.env").expect_err("must fail");
        assert!(e.contains(ENV_FILE), "{e}");
        assert!(e.contains("cannot be read"), "{e}");
    }

    #[test]
    fn a_misspelled_variable_is_warned_about_rather_than_ignored() {
        // Ignoring it is the silent failure this module exists to remove: the operator
        // believes they set a token and did not.
        let dir = std::env::temp_dir().join(format!("pve-envfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("typo.env");
        std::fs::write(
            &path,
            "RESCRIPTUM_ADMIN_TOKENN=secret\nRESCRIPTUM_STORE=sqlite\n",
        )
        .unwrap();

        let file = EnvFile::load(&path).expect("parses");
        assert_eq!(file.get("RESCRIPTUM_STORE").as_deref(), Some("sqlite"));
        assert!(
            file.warnings
                .iter()
                .any(|w| w.contains("RESCRIPTUM_ADMIN_TOKENN")),
            "{:?}",
            file.warnings
        );
        // The warning names the key, never the value.
        assert!(
            !file.warnings.iter().any(|w| w.contains("secret")),
            "a warning must not print what the file holds"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_others_can_read_is_warned_about() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pve-envfile-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("open.env");
        std::fs::write(&path, "RESCRIPTUM_ADMIN_TOKEN=0123456789abcdef0\n").unwrap();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let open = EnvFile::load(&path).expect("parses");
        assert!(
            open.warnings.iter().any(|w| w.contains("0644")),
            "{:?}",
            open.warnings
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let tight = EnvFile::load(&path).expect("parses");
        assert!(tight.warnings.is_empty(), "{:?}", tight.warnings);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
