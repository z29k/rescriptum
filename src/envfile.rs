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

/// Apply changes to the **text** of an env file, leaving everything else exactly as it is.
///
/// The file a program writes here is the same file a person edits by hand, and on a
/// packaged install it is the only documentation the configuration has — the template
/// `postinst` lays down explains every variable in comments above it. A writer that
/// regenerated the file would throw all of that away the first time anyone changed a
/// setting, so this one edits lines where they stand.
///
/// Three cases, tried in that order:
///
/// * a **live** assignment is replaced in place, keeping its indentation and its `export`;
/// * a **commented** one is uncommented and set — which is how the template's
///   `# RESCRIPTUM_STORE=sqlite` becomes a real setting instead of a duplicate appearing
///   at the bottom of the file with its explanation left behind;
/// * anything else is appended.
///
/// `None` comments a setting out rather than deleting it, so the paragraph explaining it
/// survives and setting it again lands back in the same place.
///
/// Keys are not checked here: whether a name is one this program reads is the caller's
/// question, and `KNOWN_KEYS` is where it is answered. What *is* refused here is a value
/// this file could not carry back — the parser has no escapes, so a value it would not
/// return unchanged must not be written rather than written and silently misread.
pub fn rewrite(text: &str, changes: &BTreeMap<String, Option<String>>) -> Result<String, String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut appended: Vec<String> = Vec::new();

    for (key, change) in changes {
        if !is_valid_key(key) {
            return Err(format!("{key:?} is not a usable variable name"));
        }

        let live = lines
            .iter()
            .position(|l| assignment_prefix(l, key).is_some());

        match change {
            Some(value) => {
                let rendered = render_value(key, value)?;
                if let Some(n) = live {
                    let prefix = assignment_prefix(&lines[n], key).unwrap_or_default();
                    lines[n] = format!("{prefix}{key}={rendered}");
                    continue;
                }
                let commented = lines
                    .iter()
                    .position(|l| commented_prefix(l, key).is_some());
                if let Some(n) = commented {
                    let prefix = commented_prefix(&lines[n], key).unwrap_or_default();
                    lines[n] = format!("{prefix}{key}={rendered}");
                    continue;
                }
                appended.push(format!("{key}={rendered}"));
            }
            None => {
                // Only a live line means anything here. An already-commented one is
                // already unset, and appending a comment saying so would be noise.
                if let Some(n) = live {
                    let indent: String =
                        lines[n].chars().take_while(|c| c.is_whitespace()).collect();
                    lines[n] = format!("{indent}# {}", &lines[n][indent.len()..]);
                }
            }
        }
    }

    let mut out = String::new();
    for line in lines.iter().chain(appended.iter()) {
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

/// Recognise `KEY=` on a line, and return everything that should stay in front of it —
/// the indentation and an `export` if the line carried one. `None` when this line is not
/// an assignment to this key.
fn assignment_prefix(line: &str, key: &str) -> Option<String> {
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    let rest = &line[indent.len()..];
    let (export, rest) = match rest.strip_prefix("export ") {
        Some(r) => ("export ", r.trim_start()),
        None => ("", rest),
    };
    // `RESCRIPTUM_STORE_X=1` must not answer for `RESCRIPTUM_STORE`: after the name only
    // whitespace and the `=` may follow.
    let rest = rest.strip_prefix(key)?;
    rest.trim_start().strip_prefix('=')?;
    Some(format!("{indent}{export}"))
}

/// The same, for a line that is commented out. `#KEY=`, `# KEY=` and `#  export KEY=` all
/// count — a template written by hand does not keep to one of them.
fn commented_prefix(line: &str, key: &str) -> Option<String> {
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    let rest = line[indent.len()..].strip_prefix('#')?.trim_start();
    let inner = assignment_prefix(rest, key)?;
    Some(format!("{indent}{inner}"))
}

/// How a value has to be written so that `parse` gives it back unchanged.
///
/// There are no escape sequences in this format, deliberately, which means some values
/// cannot be represented at all. Refusing those is the only honest option: writing one
/// and reading back something else is the silent failure this whole module exists to
/// remove.
fn render_value(key: &str, value: &str) -> Result<String, String> {
    if let Some(c) = value.chars().find(|c| c.is_control()) {
        return Err(format!(
            "{key}: a value cannot contain {c:?} — one line, one setting"
        ));
    }
    // A value that already looks quoted would come back stripped of its own first and
    // last character.
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' || first == b'\'') && first == last {
            return Err(format!(
                "{key}: a value that begins and ends with a quote cannot be written to \
                 this file — it would be read back without them"
            ));
        }
    }
    if value == value.trim() {
        return Ok(value.to_string());
    }
    // Only quoting protects the whitespace, and only a value without quotes of its own
    // can be quoted.
    if value.contains('"') {
        return Err(format!(
            "{key}: a value with leading or trailing whitespace cannot also contain a \
             double quote"
        ));
    }
    Ok(format!("\"{value}\""))
}

/// Replace a file's contents without a reader ever seeing half of them, and **without
/// changing who owns it**.
///
/// The rename is the atomic part, and it is the same trick the file store uses. The
/// ownership is the part that is easy to miss and expensive to get wrong: on a packaged
/// install this file is `0600` and owned by the service's own user, so a rewrite by root
/// that left a root-owned file behind would mean the service could no longer read its own
/// configuration — and the symptom would be a server that stops starting, one restart
/// later, for no reason anybody changed.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map_or_else(
        || std::ffi::OsString::from("env"),
        std::ffi::OsStr::to_os_string,
    );
    let mut tmp = std::ffi::OsString::from(".");
    tmp.push(&name);
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = dir.join(tmp);

    let existing = std::fs::metadata(path).ok();
    std::fs::write(&tmp, text)?;

    if let Err(e) = preserve(&tmp, existing.as_ref()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Carry the old file's mode and ownership onto the new one. A file that did not exist
/// gets `0600`: this one holds tokens, and inheriting a umask would be how it ends up
/// world-readable on somebody's NAS.
#[cfg(unix)]
fn preserve(tmp: &Path, existing: Option<&std::fs::Metadata>) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let mode = existing.map_or(0o600, |m| m.permissions().mode() & 0o7777);
    std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(mode))?;

    if let Some(meta) = existing {
        // Only root can give a file away, and only root needs to: anyone else is already
        // writing as the owner. A refusal here is therefore not an error.
        let _ = std::os::unix::fs::chown(tmp, Some(meta.uid()), Some(meta.gid()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn preserve(_tmp: &Path, _existing: Option<&std::fs::Metadata>) -> std::io::Result<()> {
    Ok(())
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

    // ---- rewriting -------------------------------------------------------

    fn changes(pairs: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
            .collect()
    }

    /// What `parse` makes of a rewrite — the only thing that actually matters about one.
    fn reparse(text: &str) -> BTreeMap<String, String> {
        parse(text).expect("a rewrite must produce a file this parser accepts")
    }

    #[test]
    fn a_setting_is_replaced_where_it_stands() {
        // The comment above a setting is the only documentation a packaged install has.
        // Regenerating the file would lose it the first time anybody changed anything.
        let before = "# Which store answers come from.\nRESCRIPTUM_STORE=files\n\n# The log.\nRESCRIPTUM_LOG=all\n";
        let after = rewrite(before, &changes(&[("RESCRIPTUM_STORE", Some("sqlite"))])).unwrap();

        assert!(
            after.contains("# Which store answers come from."),
            "{after}"
        );
        assert!(after.contains("# The log."), "{after}");
        assert_eq!(reparse(&after)["RESCRIPTUM_STORE"], "sqlite");
        assert_eq!(reparse(&after)["RESCRIPTUM_LOG"], "all");
        // Replaced, not appended: exactly one line mentions it.
        assert_eq!(
            after
                .lines()
                .filter(|l| l.contains("RESCRIPTUM_STORE"))
                .count(),
            1,
            "{after}"
        );
    }

    #[test]
    fn a_commented_setting_is_uncommented_rather_than_duplicated() {
        // The DSM template ships several of these — `# RESCRIPTUM_STORE=sqlite` with a
        // paragraph above explaining it. Appending a second one at the bottom of the file
        // would leave the explanation attached to the wrong line.
        let before = "# Not the default, because the package user cannot write it.\n# RESCRIPTUM_STORE=sqlite\n";
        let after = rewrite(before, &changes(&[("RESCRIPTUM_STORE", Some("sqlite"))])).unwrap();

        assert_eq!(reparse(&after)["RESCRIPTUM_STORE"], "sqlite");
        assert!(after.contains("# Not the default"), "{after}");
        assert_eq!(after.lines().count(), 2, "{after}");
    }

    #[test]
    fn a_setting_the_file_never_mentioned_is_appended() {
        let after = rewrite(
            "RESCRIPTUM_STORE=files\n",
            &changes(&[("RESCRIPTUM_LOG", Some("problems"))]),
        )
        .unwrap();
        assert_eq!(reparse(&after)["RESCRIPTUM_LOG"], "problems");
        assert_eq!(reparse(&after)["RESCRIPTUM_STORE"], "files");
    }

    #[test]
    fn indentation_and_export_survive() {
        // The same file has to keep working when it is sourced by a shell.
        let after = rewrite(
            "  export RESCRIPTUM_STORE=files\n",
            &changes(&[("RESCRIPTUM_STORE", Some("sqlite"))]),
        )
        .unwrap();
        assert_eq!(after, "  export RESCRIPTUM_STORE=sqlite\n");
        assert_eq!(reparse(&after)["RESCRIPTUM_STORE"], "sqlite");
    }

    #[test]
    fn a_similarly_named_setting_is_not_mistaken_for_this_one() {
        let after = rewrite(
            "RESCRIPTUM_LOG_FILE=/var/log/r.log\n",
            &changes(&[("RESCRIPTUM_LOG", Some("problems"))]),
        )
        .unwrap();
        let vars = reparse(&after);
        assert_eq!(vars["RESCRIPTUM_LOG_FILE"], "/var/log/r.log");
        assert_eq!(vars["RESCRIPTUM_LOG"], "problems");
    }

    #[test]
    fn unsetting_comments_the_line_out_and_keeps_it() {
        // So the paragraph above it survives, and setting it again lands back here.
        let before = "# The write API.\nRESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001\n";
        let after = rewrite(before, &changes(&[("RESCRIPTUM_ADMIN_ADDR", None)])).unwrap();

        assert!(
            !reparse(&after).contains_key("RESCRIPTUM_ADMIN_ADDR"),
            "{after}"
        );
        assert!(
            after.contains("# RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001"),
            "{after}"
        );

        let again = rewrite(
            &after,
            &changes(&[("RESCRIPTUM_ADMIN_ADDR", Some("127.0.0.1:9001"))]),
        )
        .unwrap();
        assert_eq!(reparse(&again)["RESCRIPTUM_ADMIN_ADDR"], "127.0.0.1:9001");
        assert_eq!(again.lines().count(), 2, "{again}");
    }

    #[test]
    fn a_value_that_would_not_survive_the_round_trip_is_refused() {
        // There are no escapes in this format. Writing one of these and reading back
        // something else is exactly the silent failure this module exists to remove.
        for bad in ["two\nlines", "tab\there", "\"quoted\"", "'quoted'"] {
            let e = rewrite(
                "RESCRIPTUM_STORE=files\n",
                &changes(&[("RESCRIPTUM_ADMIN_TOKEN", Some(bad))]),
            )
            .expect_err("{bad} should be refused");
            assert!(e.contains("RESCRIPTUM_ADMIN_TOKEN"), "{e}");
        }
    }

    #[test]
    fn whitespace_a_value_needs_is_quoted_and_comes_back() {
        let after = rewrite(
            "",
            &changes(&[("RESCRIPTUM_ADMIN_TOKEN", Some(" padded token "))]),
        )
        .unwrap();
        assert_eq!(reparse(&after)["RESCRIPTUM_ADMIN_TOKEN"], " padded token ");
    }

    #[cfg(unix)]
    #[test]
    fn an_atomic_write_keeps_the_mode_and_leaves_no_temporary_behind() {
        // A `0600` file that came back `0644` would put the admin token within reach of
        // every account on the machine, quietly.
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pve-envfile-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rescriptum.env");

        write_atomic(&path, "RESCRIPTUM_STORE=files\n").expect("writes");
        let fresh = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(fresh, 0o600, "a new file must not inherit the umask");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&path, "RESCRIPTUM_STORE=sqlite\n").expect("writes");
        let kept = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(kept, 0o640, "an existing file keeps the mode it had");

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "RESCRIPTUM_STORE=sqlite\n"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "rescriptum.env")
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files left behind: {leftovers:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
