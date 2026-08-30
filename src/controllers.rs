//! Where a machine's out-of-band controller is described — a BMC, a PiKVM, a PDU.
//!
//! **Not in the answer document.** The tempting design is a `[controller]` control key
//! beside `extends` and `members`, stripped before the answer is sent. It is refused: a
//! control key holding a BMC password is one strip bug away from handing that credential
//! to the machine currently being installed, and that machine is by definition in an
//! untrusted state. The blast radius is the whole fleet's power control.
//!
//! **Not in SQLite either.** `export <dir>` writes the store out as a directory and the
//! round trip is byte-identical by contract, so a controllers table would either be
//! exported — writing fleet power credentials into a directory somebody is about to copy
//! somewhere — or silently dropped, breaking the round trip that makes the database safe
//! to leave. Neither is acceptable, so it is a file.
//!
//! **Named, never discovered.** There is no `./controllers.toml`. A file picked up from
//! the working directory would hand fleet power to whoever can write there — the same
//! reasoning `RESCRIPTUM_ENV_FILE` already has.
//!
//! **The server never reads it.** `power` and `install` do. A malformed BMC credentials
//! file that stopped the answer listener would take a fleet's installs down for a reason
//! entirely unrelated to answering, which is the failure the "unset, it does not exist"
//! rule exists to prevent. Startup says only whether the file is there and whether its
//! mode is alarming; it never parses it.

use crate::select::normalize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use toml_edit::{DocumentMut, Item, Value};

/// What a Redfish service is rooted at when nobody says otherwise. PiKVM serves
/// `/api/redfish/v1` instead — and note its response bodies say `/redfish/v1` regardless,
/// which is why URLs are composed from this value and an id rather than followed out of
/// the body.
pub const DEFAULT_BASE: &str = "/redfish/v1";

/// A hung `pdu` script would otherwise hang `install` forever: `std::process::Command`
/// has no deadline of its own.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// How this controller's certificate is to be trusted.
///
/// **One of these is required**, and an entry carrying none is refused. Self-signed is
/// the normal case for a BMC, so defaulting to "do not verify" would be the convenient
/// choice — and it is the same convenience `media add` already refuses, where a URL
/// requires `--sha256` unless `--unverified` is passed. This link can power-cycle a rack;
/// one line of friction is the right price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tls {
    /// `verify = false`. Said out loud, once per controller, wherever this is used.
    Insecure,
    /// A fleet with its own CA, which large ones have.
    CaCert(PathBuf),
    /// The right answer for a self-signed BMC: free, because curl implements it, and the
    /// same shape as the `cert_fingerprint` Proxmox's own `auto-installer-mode.toml`
    /// already takes.
    PinnedPubKey(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redfish {
    pub url: String,
    pub base: String,
    pub user: String,
    pub pass: String,
    /// Set only where discovery cannot decide — a chassis, or a PiKVM with a switch,
    /// exposes several systems and picking the first would power somebody else's machine.
    pub system: Option<String>,
    pub tls: Tls,
}

/// The escape hatch: a switched PDU, `ipmitool`, `amtterm`, `wakeonlan`, and whatever
/// hardware exists in five years — without this project learning any of them.
///
/// Three rules keep it from being a hole, and all three are enforced here or at the call
/// site: **argv only, never a shell**; **no substitution from request facts**, so nothing
/// a machine sent over the network can reach an argument vector; and **a deadline**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandHook {
    pub on: Vec<String>,
    pub off: Vec<String>,
    /// Often empty, and that is not a gap: where one-time boot does not exist the boot
    /// order stays on PXE permanently and the *server* decides whether to install.
    pub pxe: Vec<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Redfish(Redfish),
    Command(CommandHook),
}

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Redfish(_) => "redfish",
            Kind::Command(_) => "command",
        }
    }

    /// Whether this controller can arm a one-time network boot at all.
    ///
    /// `false` is not a defect. Where one-time boot does not exist the boot order stays on
    /// PXE and `RESCRIPTUM_BOOT_UNCLAIMED` plus the `installed-` disarm decide whether a
    /// machine installs — a PiKVM plus rescriptum is a complete solution, and a BMC with
    /// one-time boot is belt and braces.
    pub fn can_pxe(&self) -> bool {
        match self {
            Kind::Redfish(_) => true,
            Kind::Command(c) => !c.pxe.is_empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controller {
    /// The key as written, for anything a person reads.
    pub id: String,
    /// The same key normalized, which is how it joins to the answers directory.
    pub identity: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, Default)]
pub struct Controllers {
    entries: Vec<Controller>,
}

impl Controllers {
    pub fn iter(&self) -> impl Iterator<Item = &Controller> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The controller for a machine, matched the way the answers directory matches.
    ///
    /// Both sides are normalized, so `98:fa:9b:50:d8:10` and `98fa9b50d810` are one
    /// machine on both sides. A second identity space would have been a mistake.
    pub fn find(&self, id: &str) -> Option<&Controller> {
        let wanted = normalize(id.as_bytes());
        self.entries.iter().find(|c| c.identity == wanted)
    }
}

/// Read and parse the file, or say why not.
///
/// **Group- or world-readable is refused here**, where `envfile` only warns. The
/// divergence is deliberate: refusing an env file would stop a server that is otherwise
/// healthy, while refusing this one costs a single interactive command. Note the blind
/// spot already recorded for the answers directory — mode bits lie on DSM, where an ACL
/// can grant access `st_mode` never mentions — so this is a warning about the *mode* plus
/// documentation, not a proof of privacy.
pub fn load(path: &Path) -> Result<Controllers, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("RESCRIPTUM_CONTROLLERS_FILE={} : {e}", path.display()))?;

    if let Some(mode) = crate::envfile::readable_by_others(path) {
        return Err(format!(
            "{} is mode {mode:04o} — it holds credentials that can power-cycle a rack; chmod 600 it",
            path.display()
        ));
    }

    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The parser, separated from the file so it can be tested without one.
pub fn parse(text: &str) -> Result<Controllers, String> {
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| e.to_string().replace('\n', " "))?;

    let mut entries: Vec<Controller> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (key, item) in doc.as_table().iter() {
        let table = item
            .as_table()
            .ok_or_else(|| format!("{key}: expected a table, `[\"{key}\"]`"))?;

        let identity = normalize(key.as_bytes());
        if identity.is_empty() {
            return Err(format!("{key}: this name normalizes to nothing"));
        }
        // Two entries that differ only in separator style are one machine here, exactly
        // as they are in the answers directory, so the second is a mistake rather than an
        // override nobody would see applied.
        if !seen.insert(identity.clone()) {
            return Err(format!("{key}: a second entry for the same machine"));
        }

        let kind = match string(table, key, "kind")?.as_deref() {
            Some("redfish") => Kind::Redfish(redfish(table, key)?),
            Some("command") => Kind::Command(command(table, key)?),
            Some(other) => {
                return Err(format!(
                    "{key}: kind = {other:?} is not one this program drives — \
                     \"redfish\" or \"command\""
                ));
            }
            None => return Err(format!("{key}: no `kind` — \"redfish\" or \"command\"")),
        };

        entries.push(Controller {
            id: key.to_string(),
            identity,
            kind,
        });
    }

    entries.sort_by(|a, b| a.identity.cmp(&b.identity));
    Ok(Controllers { entries })
}

fn redfish(table: &toml_edit::Table, key: &str) -> Result<Redfish, String> {
    let url = required(table, key, "url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!("{key}: url = {url:?} needs a scheme"));
    }
    // A URL, not a host — and a path here would be silently appended to every request.
    if url.trim_end_matches('/').matches('/').count() > 2 {
        return Err(format!(
            "{key}: url = {url:?} carries a path; put it in `base` instead"
        ));
    }

    let base = string(table, key, "base")?.unwrap_or_else(|| DEFAULT_BASE.to_string());
    let tls = tls(table, key)?;

    Ok(Redfish {
        url: url.trim_end_matches('/').to_string(),
        base: format!("/{}", base.trim_matches('/')),
        user: required(table, key, "user")?,
        pass: required(table, key, "pass")?,
        system: string(table, key, "system")?,
        tls,
    })
}

/// Exactly one of the three, and an entry with none is refused naming all three.
fn tls(table: &toml_edit::Table, key: &str) -> Result<Tls, String> {
    let verify = match table.get("verify") {
        None => None,
        Some(item) => Some(item.as_bool().ok_or_else(|| {
            format!("{key}: verify must be true or false, and only `false` means anything here")
        })?),
    };
    let cacert = string(table, key, "cacert")?;
    let pinned = string(table, key, "pinnedpubkey")?;

    if verify == Some(true) {
        // Checked before the count below, so that a deliberate `verify = true` gets the
        // answer to what it was reaching for rather than the generic "say something".
        // Not an error because the system trust store is meaningless — it is a real
        // answer, just not one a BMC's self-signed certificate can satisfy.
        return Err(format!(
            "{key}: verify = true is the default for a public certificate authority, which \
             a BMC does not have. Use `cacert` for your own CA, or `pinnedpubkey`"
        ));
    }

    let chosen = usize::from(verify == Some(false))
        + usize::from(cacert.is_some())
        + usize::from(pinned.is_some());
    if chosen == 0 {
        return Err(format!(
            "{key}: say how this controller's certificate is to be trusted — one of \
             `verify = false`, `cacert = \"…\"` or `pinnedpubkey = \"sha256//…\"`. \
             A BMC ships a self-signed certificate, so there is no safe default to pick \
             for you"
        ));
    }
    if chosen > 1 {
        return Err(format!(
            "{key}: `verify`, `cacert` and `pinnedpubkey` are three answers to one \
             question; give exactly one"
        ));
    }
    Ok(match (cacert, pinned) {
        (Some(path), _) => Tls::CaCert(PathBuf::from(path)),
        (_, Some(k)) => Tls::PinnedPubKey(k),
        _ => Tls::Insecure,
    })
}

fn command(table: &toml_edit::Table, key: &str) -> Result<CommandHook, String> {
    let hook = CommandHook {
        on: argv(table, key, "on")?,
        off: argv(table, key, "off")?,
        pxe: argv(table, key, "pxe")?,
        timeout: match table.get("timeout") {
            None => DEFAULT_COMMAND_TIMEOUT,
            Some(item) => {
                let secs = item
                    .as_integer()
                    .ok_or_else(|| format!("{key}: timeout is a number of seconds"))?;
                if secs <= 0 {
                    return Err(format!("{key}: timeout = {secs} would never run anything"));
                }
                Duration::from_secs(secs as u64)
            }
        },
    };
    if hook.on.is_empty() && hook.off.is_empty() {
        return Err(format!(
            "{key}: a command controller that can neither power on nor off does nothing"
        ));
    }
    Ok(hook)
}

/// An argument vector, **never a string to be split**.
///
/// No `sh -c`, no word splitting, no interpolation: the array is handed to `Command` as
/// written. Accepting a string here would be the moment a quoting bug became a shell
/// injection, on a server that runs as root.
fn argv(table: &toml_edit::Table, key: &str, field: &str) -> Result<Vec<String>, String> {
    match table.get(field) {
        None => Ok(Vec::new()),
        Some(Item::Value(Value::Array(array))) => {
            let mut out = Vec::with_capacity(array.len());
            for element in array {
                let s = element
                    .as_str()
                    .ok_or_else(|| format!("{key}: every element of `{field}` must be a string"))?;
                out.push(s.to_string());
            }
            Ok(out)
        }
        Some(Item::Value(Value::String(_))) => Err(format!(
            "{key}: `{field}` is an argument vector, not a command line — \
             [\"/usr/local/bin/pdu\", \"outlet\", \"7\", \"on\"]. Nothing here is passed \
             through a shell, so a string could not be split safely"
        )),
        Some(_) => Err(format!("{key}: `{field}` must be an array of strings")),
    }
}

fn string(table: &toml_edit::Table, key: &str, field: &str) -> Result<Option<String>, String> {
    match table.get(field) {
        None => Ok(None),
        Some(item) => match item.as_str() {
            Some(s) => Ok(Some(s.to_string())),
            None => Err(format!("{key}: `{field}` must be a string")),
        },
    }
}

fn required(table: &toml_edit::Table, key: &str, field: &str) -> Result<String, String> {
    string(table, key, field)?.ok_or_else(|| format!("{key}: no `{field}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Controller {
        let c = parse(text).expect("should parse");
        assert_eq!(c.len(), 1);
        c.iter().next().expect("one entry").clone()
    }

    fn err(text: &str) -> String {
        parse(text).expect_err("should be refused")
    }

    const REDFISH: &str = r#"
["98-fa-9b-50-d8-10"]
kind = "redfish"
url = "https://10.0.0.51"
user = "root"
pass = "calvin"
verify = false
"#;

    #[test]
    fn a_redfish_entry_reads_back() {
        let c = one(REDFISH);
        assert_eq!(c.id, "98-fa-9b-50-d8-10");
        assert_eq!(c.identity, "98fa9b50d810");
        let Kind::Redfish(r) = &c.kind else {
            panic!("expected redfish, got {:?}", c.kind)
        };
        assert_eq!(r.url, "https://10.0.0.51");
        // The base is what everything else is composed from, so its default matters.
        assert_eq!(r.base, DEFAULT_BASE);
        assert_eq!(r.tls, Tls::Insecure);
        assert!(r.system.is_none());
    }

    #[test]
    fn separator_style_does_not_make_a_second_machine() {
        // The same identity space as the answers directory, or the two sides of the join
        // drift apart in exactly the way that is hardest to notice.
        let c = parse(REDFISH).expect("parse");
        for spelling in ["98:fa:9b:50:d8:10", "98fa9b50d810", "98-FA-9B-50-D8-10"] {
            assert!(c.find(spelling).is_some(), "{spelling} should match");
        }
        assert!(c.find("aa:bb:cc:dd:ee:ff").is_none());
    }

    #[test]
    fn two_spellings_of_one_machine_are_refused() {
        let text = format!("{REDFISH}\n[\"98fa9b50d810\"]\nkind = \"redfish\"\n");
        assert!(err(&text).contains("a second entry"), "{}", err(&text));
    }

    /// The rule `media add` already has, applied here: the unsafe path is a deliberate
    /// act, because this link can power-cycle a rack.
    #[test]
    fn an_entry_that_says_nothing_about_the_certificate_is_refused() {
        let text = r#"
["98fa9b50d810"]
kind = "redfish"
url = "https://10.0.0.51"
user = "root"
pass = "calvin"
"#;
        let e = err(text);
        assert!(e.contains("verify"), "{e}");
        assert!(e.contains("cacert"), "{e}");
        assert!(e.contains("pinnedpubkey"), "{e}");
    }

    #[test]
    fn three_answers_to_one_question_are_refused() {
        let text = r#"
["98fa9b50d810"]
kind = "redfish"
url = "https://10.0.0.51"
user = "root"
pass = "calvin"
verify = false
pinnedpubkey = "sha256//abc"
"#;
        assert!(err(text).contains("exactly one"), "{}", err(text));
    }

    #[test]
    fn a_pinned_key_and_a_ca_are_both_understood() {
        let pinned = one(&REDFISH.replace("verify = false", "pinnedpubkey = \"sha256//abc\""));
        let Kind::Redfish(r) = &pinned.kind else {
            panic!("redfish")
        };
        assert_eq!(r.tls, Tls::PinnedPubKey("sha256//abc".to_string()));

        let ca = one(&REDFISH.replace("verify = false", "cacert = \"/etc/bmc-ca.pem\""));
        let Kind::Redfish(r) = &ca.kind else {
            panic!("redfish")
        };
        assert_eq!(r.tls, Tls::CaCert(PathBuf::from("/etc/bmc-ca.pem")));
    }

    /// `verify = true` is a real intention and a wrong one here, so it is named rather
    /// than quietly treated as "not false".
    #[test]
    fn verify_true_says_what_it_would_mean() {
        let e = err(&REDFISH.replace("verify = false", "verify = true"));
        assert!(e.contains("certificate authority"), "{e}");
    }

    /// A password is written by a person into a file and read back by a program. If those
    /// two disagree about backslashes and quotes, the BMC answers 401 and that reads as a
    /// wrong password rather than as a bug.
    #[test]
    fn a_password_carrying_a_quote_and_a_backslash_survives() {
        let text = r#"
["98fa9b50d810"]
kind = "redfish"
url = "https://10.0.0.51"
user = "root"
pass = 'a"b\c'
verify = false
"#;
        let Kind::Redfish(r) = &one(text).kind else {
            panic!("redfish")
        };
        assert_eq!(r.pass, r#"a"b\c"#);
    }

    #[test]
    fn the_base_is_normalized_so_pikvm_can_be_written_either_way() {
        for written in ["/api/redfish/v1", "api/redfish/v1", "/api/redfish/v1/"] {
            let text = REDFISH.replace(
                "kind = \"redfish\"",
                &format!("kind = \"redfish\"\nbase = \"{written}\""),
            );
            let Kind::Redfish(r) = &one(&text).kind else {
                panic!("redfish")
            };
            assert_eq!(r.base, "/api/redfish/v1", "{written}");
        }
    }

    #[test]
    fn a_url_carrying_a_path_is_refused_rather_than_silently_prefixed() {
        let e = err(&REDFISH.replace("https://10.0.0.51", "https://10.0.0.51/redfish/v1"));
        assert!(e.contains("base"), "{e}");
    }

    #[test]
    fn a_command_hook_reads_back_with_its_deadline() {
        let text = r#"
["aa-bb-cc-dd-ee-ff"]
kind = "command"
on = ["/usr/local/bin/pdu", "outlet", "7", "on"]
off = ["/usr/local/bin/pdu", "outlet", "7", "off"]
pxe = []
timeout = 45
"#;
        let c = one(text);
        let Kind::Command(h) = &c.kind else {
            panic!("expected command, got {:?}", c.kind)
        };
        assert_eq!(h.on, ["/usr/local/bin/pdu", "outlet", "7", "on"]);
        assert_eq!(h.timeout, Duration::from_secs(45));
        // Empty `pxe` is the ordinary case, not a gap: the boot order stays on PXE and
        // the server decides whether to install.
        assert!(h.pxe.is_empty());
        assert!(!c.kind.can_pxe());
    }

    #[test]
    fn a_command_written_as_a_line_is_refused_rather_than_split() {
        // Splitting it would be the moment a quoting bug became a shell injection, on a
        // server that runs as root.
        let text = r#"
["aa-bb-cc-dd-ee-ff"]
kind = "command"
on = "/usr/local/bin/pdu outlet 7 on"
"#;
        let e = err(text);
        assert!(e.contains("argument vector"), "{e}");
    }

    #[test]
    fn a_command_that_can_do_nothing_is_refused() {
        assert!(err("[\"aa-bb-cc-dd-ee-ff\"]\nkind = \"command\"\n").contains("does nothing"));
    }

    #[test]
    fn an_unknown_kind_is_named() {
        let e = err("[\"aa\"]\nkind = \"ipmi\"\n");
        assert!(e.contains("ipmi"), "{e}");
        assert!(e.contains("redfish"), "{e}");
    }

    #[test]
    fn entries_come_back_in_a_stable_order() {
        let text =
            format!("{REDFISH}\n[\"aa-bb-cc-dd-ee-ff\"]\nkind = \"command\"\non = [\"x\"]\n");
        let ids: Vec<String> = parse(&text)
            .expect("parse")
            .iter()
            .map(|c| c.identity.clone())
            .collect();
        assert_eq!(ids, ["98fa9b50d810", "aabbccddeeff"]);
    }

    /// Refused at use, where `envfile` only warns. Refusing an env file would stop a
    /// server that is otherwise healthy; refusing this one costs a single interactive
    /// command, and the file holds credentials that can power-cycle a rack.
    #[cfg(unix)]
    #[test]
    fn a_file_others_can_read_is_refused_rather_than_warned_about() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("rescriptum-ctl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("controllers.toml");
        std::fs::write(&path, REDFISH).expect("write");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert!(load(&path).is_ok(), "0600 must be accepted");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
        let e = load(&path).expect_err("0640 must be refused");
        assert!(e.contains("0640"), "{e}");
        assert!(e.contains("chmod 600"), "{e}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_names_the_variable_that_asked_for_it() {
        let e = load(Path::new("/nonexistent-root/controllers.toml")).expect_err("missing");
        assert!(e.contains("RESCRIPTUM_CONTROLLERS_FILE"), "{e}");
    }

    #[test]
    fn a_file_that_will_not_parse_says_so_on_one_line() {
        let e = err("this is not toml");
        assert!(!e.contains('\n'), "{e}");
    }
}
