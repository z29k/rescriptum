//! Talking to a Redfish service, through `curl`.
//!
//! **Why curl and not a TLS crate.** Redfish is HTTPS in practice and BMCs ship
//! self-signed certificates. There is no TLS in this binary on purpose — `rustls` plus
//! `webpki` is about a megabyte on armv7, which would roughly double a 2.8 MB binary for
//! one feature. `boot::fetch` already shells out for exactly this reason.
//!
//! **This is curl-only, which is a narrower claim than `fetch` makes.** `fetch` falls back
//! to `wget`; nothing here can. A Redfish call is a POST or a PATCH with a JSON body,
//! custom headers, and a credential that must stay out of the process table — wget does
//! not do that combination. So the error says *curl*, names why, and stops.
//!
//! **The credential never reaches `argv`.** A password in an argument vector is visible to
//! every user on the box through `/proc/<pid>/cmdline`. `curl --config -` reads its
//! options from stdin instead, so `ps` shows only `curl --config -`.
//!
//! Four things about driving curl this way that decide whether it works:
//!
//! - **`--config -` occupies stdin**, so the request body cannot also arrive there. It
//!   goes *inside* the config file, which means it meets the same quoting rules the
//!   password does — hence one escaper used for every value rather than two that can
//!   disagree.
//! - **`--fail` must not be used.** It suppresses the response body on an error status,
//!   and that body is `error.@Message.ExtendedInfo[].Message`: a sentence the vendor wrote
//!   about what went wrong. Discarding it is how somebody ends up reading a packet
//!   capture.
//! - **`--write-out` goes to stdout, where the body already is.** Writing the status there
//!   naively welds three digits onto the end of a JSON document. A newline plus the code,
//!   split from the right, keeps both.
//! - **`Content-Type: application/json` has to be set by hand**, because `--data` sends a
//!   form content type and a Redfish service answers that with 415.

use crate::controllers::{Redfish, Tls};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// A BMC that accepts a connection and never answers is ordinary. Without a deadline
/// `install` would hang with no output.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// What one exchange produced. The status is kept separate from the body so that an error
/// body survives to be read — see the note about `--fail` above.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    /// `@odata.etag`, when the resource carried one. Some iLO and iDRAC builds answer a
    /// `PATCH` without `If-Match` with 412.
    pub etag: Option<String>,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The sentence the vendor wrote, when there is one.
    ///
    /// Surfacing "HTTP 400" and throwing this away is the difference between an operator
    /// fixing their request and an operator reading a packet capture.
    pub fn message(&self) -> Option<String> {
        let v: Value = serde_json::from_str(&self.body).ok()?;
        let info = v.get("error")?.get("@Message.ExtendedInfo")?.as_array()?;
        let messages: Vec<String> = info
            .iter()
            .filter_map(|m| m.get("Message")?.as_str().map(str::to_string))
            .collect();
        if messages.is_empty() {
            // Some services put a sentence directly on the error object instead.
            return v.get("error")?.get("message")?.as_str().map(str::to_string);
        }
        Some(messages.join("; "))
    }

    /// `<status> — <what the vendor said>`, for a message a person reads.
    pub fn describe(&self) -> String {
        match self.message() {
            Some(m) => format!("HTTP {} — {m}", self.status),
            None => format!("HTTP {}", self.status),
        }
    }
}

/// What a `PATCH` or a `POST` did, when the answer is that nobody knows.
///
/// A deadline says when to stop waiting; it says nothing about what happened. A
/// `ComputerSystem.Reset` that timed out may have powered the rack on, and a `Boot` PATCH
/// that timed out may or may not have taken. **So a write is never retried automatically**
/// — the caller is told the outcome is unknown and given the means to read it back.
#[derive(Debug)]
pub enum Failed {
    /// curl is not installed. There is no `wget` fallback here, unlike `boot::fetch`.
    NoCurl,
    /// The request did not complete. Whether it took effect is unknown.
    Unknown(String),
    /// It completed and the service refused it.
    Refused(Reply),
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::NoCurl => write!(
                f,
                "curl is not installed, and there is no TLS in this binary — \
                 a Redfish call needs a POST with a JSON body, custom headers and a \
                 credential kept out of the process table, which wget cannot do"
            ),
            Failed::Unknown(why) => write!(f, "{why}"),
            Failed::Refused(r) => write!(f, "{}", r.describe()),
        }
    }
}

/// Escape one value for curl's configuration-file syntax.
///
/// A value in double quotes understands `\\`, `\"`, `\t`, `\n`, `\r` and `\v`. A password
/// containing a backslash or a quote, written through unescaped, authenticates as
/// something else — and a BMC's answer to that is a 401, which reads as a wrong password
/// rather than as a bug.
///
/// **One escaper, used for every value**, the JSON body included. Two of these is how one
/// of them stays wrong.
pub fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{b}' => out.push_str("\\v"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The client for one controller.
pub struct Client<'a> {
    controller: &'a Redfish,
    timeout: Duration,
}

/// What `ps` would show. Held apart from the config text so a test can assert the
/// credential is in one and not the other.
pub fn argv() -> [&'static str; 3] {
    ["curl", "--config", "-"]
}

impl<'a> Client<'a> {
    pub fn new(controller: &'a Redfish) -> Client<'a> {
        Client {
            controller,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The options curl is handed on stdin. Public so that it can be tested without
    /// running anything, which is also how the credential's absence from `argv` is pinned.
    pub fn config(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        etag: Option<&str>,
    ) -> String {
        let mut out = String::new();
        let url = format!("{}{path}", self.controller.url);
        out.push_str(&format!("url = {}\n", quote(&url)));
        out.push_str(&format!("request = {}\n", quote(method)));
        out.push_str(&format!(
            "user = {}\n",
            quote(&format!(
                "{}:{}",
                self.controller.user, self.controller.pass
            ))
        ));
        // PiKVM authenticates with KVMD's own headers as well as Basic; sending both costs
        // nothing and means one code path reaches both populations.
        out.push_str(&format!(
            "header = {}\n",
            quote(&format!("X-KVMD-User: {}", self.controller.user))
        ));
        out.push_str(&format!(
            "header = {}\n",
            quote(&format!("X-KVMD-Passwd: {}", self.controller.pass))
        ));
        out.push_str("header = \"Accept: application/json\"\n");

        if let Some(tag) = etag {
            out.push_str(&format!(
                "header = {}\n",
                quote(&format!("If-Match: {tag}"))
            ));
        }
        if let Some(body) = body {
            // Set by hand: `--data` alone sends a form content type, and a Redfish service
            // answers that with 415. This is the single most common "works in the vendor's
            // example, fails from curl" failure.
            out.push_str("header = \"Content-Type: application/json\"\n");
            out.push_str(&format!("data = {}\n", quote(body)));
        }

        match &self.controller.tls {
            Tls::Insecure => out.push_str("insecure\n"),
            Tls::CaCert(p) => {
                out.push_str(&format!("cacert = {}\n", quote(&p.display().to_string())));
            }
            Tls::PinnedPubKey(k) => out.push_str(&format!("pinnedpubkey = {}\n", quote(k))),
        }

        out.push_str(&format!("max-time = {}\n", self.timeout.as_secs()));
        out.push_str("silent\n");
        out.push_str("show-error\n");
        // Deliberately no `fail`: it would discard the vendor's error body, which is the
        // most useful thing a failed call produces. The status comes from `write-out`
        // instead, after a newline so the body can be split back off from the right.
        out.push_str("dump-header = \"-\"\n");
        out.push_str("write-out = \"\\n%{http_code}\"\n");
        out
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        etag: Option<&str>,
    ) -> Result<Reply, Failed> {
        let config = self.config(method, path, body, etag);

        let mut child = match Command::new("curl")
            .args(&argv()[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Failed::NoCurl),
            Err(e) => return Err(Failed::Unknown(format!("cannot run curl: {e}"))),
        };

        if let Some(mut stdin) = child.stdin.take()
            && let Err(e) = stdin.write_all(config.as_bytes())
        {
            return Err(Failed::Unknown(format!("cannot write curl's options: {e}")));
        }

        let out = child
            .wait_with_output()
            .map_err(|e| Failed::Unknown(format!("curl did not finish: {e}")))?;

        if !out.status.success() {
            let code = out.status.code().unwrap_or(-1);
            let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
            // 28 is curl's timeout, and it is the one whose meaning matters: the request
            // may well have taken effect. Say so rather than implying nothing happened.
            let why = match code {
                28 => format!(
                    "{} did not answer within {}s — **the outcome is unknown**; \
                     read the state back rather than sending it again",
                    self.controller.url,
                    self.timeout.as_secs()
                ),
                6 => format!("cannot resolve {}: {said}", self.controller.url),
                7 => format!("cannot connect to {}: {said}", self.controller.url),
                60 => format!(
                    "{} presented a certificate this configuration does not trust: {said}",
                    self.controller.url
                ),
                _ => format!("curl exited {code}: {said}"),
            };
            return Err(Failed::Unknown(why));
        }

        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let (head_and_body, status) = text
            .rsplit_once('\n')
            .ok_or_else(|| Failed::Unknown("curl produced no status".to_string()))?;
        let status: u16 = status
            .trim()
            .parse()
            .map_err(|_| Failed::Unknown(format!("curl produced no status: {status:?}")))?;

        // `dump-header -` puts the headers on stdout ahead of the body, which is how the
        // etag is read without a second request.
        let (headers, body) = split_headers(head_and_body);
        Ok(Reply {
            status,
            body: body.to_string(),
            etag: header(headers, "etag"),
        })
    }

    /// `GET`, which is the only verb here that may be retried freely.
    pub fn get(&self, path: &str) -> Result<Reply, Failed> {
        self.send("GET", path, None, None)
    }

    /// The system this controller drives.
    ///
    /// **Never follow a URL out of the response body.** `@odata.id` is service-root
    /// relative by the specification, and PiKVM breaks that: the handbook serves Redfish
    /// at `/api/redfish/v1` while kvmd emits `"@odata.id": "/redfish/v1/Systems/0"`, so
    /// the value is inconsistent with the path it came from. Taking the **last segment**
    /// and composing `<base>/Systems/<id>` is the only form that works on both.
    ///
    /// **`Members[0]` is a guess.** A blade enclosure, a Dell FX2, and a PiKVM with a
    /// switch all expose several systems — and on a PiKVM with ATX disabled, the first
    /// member is a switch port, a different machine entirely. With more than one, refuse
    /// and name them; the entry can say `system = "…"`.
    pub fn system_id(&self) -> Result<String, Failed> {
        if let Some(explicit) = &self.controller.system {
            return Ok(explicit.clone());
        }

        let path = format!("{}/Systems", self.controller.base);
        let reply = self.get(&path)?;
        if !reply.ok() {
            return Err(Failed::Refused(reply));
        }

        let members = members(&reply.body).map_err(|e| Failed::Unknown(format!("{path}: {e}")))?;
        match members.len() {
            0 => Err(Failed::Unknown(format!(
                "{} has no systems to drive",
                self.controller.url
            ))),
            1 => Ok(members[0].clone()),
            _ => Err(Failed::Unknown(format!(
                "{} exposes {} systems ({}) — say which with `system = \"…\"` in the \
                 controllers file. Picking the first would power somebody else's machine",
                self.controller.url,
                members.len(),
                members.join(", ")
            ))),
        }
    }

    /// Ask this system to change power state.
    ///
    /// The reset is checked against `ResetType@Redfish.AllowableValues` first: sending
    /// `GracefulShutdown` to a service that only offers `ForceOff` earns a 400, and
    /// naming the ones that *would* work beats reporting the number.
    ///
    /// **Never retried.** A `Reset` that timed out may have powered the rack on.
    pub fn reset(&self, id: &str, reset: &str) -> Result<(), Failed> {
        let system = self.system(id)?;
        let allowed = allowable_resets(&system.body);
        if !allowed.is_empty() && !allowed.iter().any(|a| a == reset) {
            return Err(Failed::Unknown(format!(
                "this system does not accept {reset:?} — it offers {}",
                allowed.join(", ")
            )));
        }

        let body = format!("{{\"ResetType\":\"{reset}\"}}");
        let path = format!(
            "{}/Systems/{id}/Actions/ComputerSystem.Reset",
            self.controller.base
        );
        let reply = self.send("POST", &path, Some(&body), None)?;
        if reply.ok() {
            Ok(())
        } else {
            Err(Failed::Refused(reply))
        }
    }

    /// Arm a **one-time** network boot, and read it back.
    ///
    /// `Once`, never `Continuous`, and that is a safety property rather than tidiness: an
    /// override consumed at the next boot means a machine that fails to install and
    /// reboots comes up on its own disk instead of installing again — the same protection
    /// `RESCRIPTUM_BOOT_UNCLAIMED` gives from the other end.
    ///
    /// **`BootSourceOverrideMode` is deliberately not set.** It selects UEFI or Legacy;
    /// setting it wrong makes a UEFI machine PXE-boot in legacy mode and fail in a way
    /// that looks like the TFTP server, and on several iDRAC generations changing it needs
    /// a reboot before it takes effect. Leave the BMC's own setting alone.
    ///
    /// **The read-back is not optional.** PiKVM's handler returns `204 No Content` and
    /// does nothing at all — verified in kvmd's source — while reporting
    /// `BootSourceOverrideEnabled: "Disabled"`. A client that trusts the status code
    /// believes it armed a boot that will not happen, and the machine then installs
    /// nothing while looking correct.
    pub fn set_pxe_once(&self, id: &str) -> Result<bool, Failed> {
        let before = self.system(id)?;
        let path = format!("{}/Systems/{id}", self.controller.base);
        let body =
            r#"{"Boot":{"BootSourceOverrideTarget":"Pxe","BootSourceOverrideEnabled":"Once"}}"#;

        let reply = self.send("PATCH", &path, Some(body), before.etag.as_deref())?;
        if !reply.ok() {
            return Err(Failed::Refused(reply));
        }

        let after = self.system(id)?;
        let (enabled, target) = boot_override(&after.body);
        Ok(enabled.as_deref() == Some("Once") && target.as_deref() == Some("Pxe"))
    }

    /// The system resource: its power state, what resets it accepts, and its etag.
    pub fn system(&self, id: &str) -> Result<Reply, Failed> {
        let reply = self.get(&format!("{}/Systems/{id}", self.controller.base))?;
        if reply.ok() {
            Ok(reply)
        } else {
            Err(Failed::Refused(reply))
        }
    }
}

/// Member ids from a Redfish collection — the last segment of each `@odata.id`.
pub fn members(body: &str) -> Result<Vec<String>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    let array = v
        .get("Members")
        .and_then(Value::as_array)
        .ok_or_else(|| "no `Members` array".to_string())?;
    Ok(array
        .iter()
        .filter_map(|m| m.get("@odata.id")?.as_str())
        .filter_map(|id| id.trim_end_matches('/').rsplit('/').next())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect())
}

/// What resets this system says it accepts.
///
/// Sending `GracefulShutdown` to a BMC that only offers `ForceOff` earns a 400. Reading
/// the list first means naming the ones that would work instead.
pub fn allowable_resets(system_body: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(system_body) else {
        return Vec::new();
    };
    v.get("Actions")
        .and_then(|a| a.get("#ComputerSystem.Reset"))
        .and_then(|r| r.get("ResetType@Redfish.AllowableValues"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `On`, `Off`, or whatever else the service calls it.
pub fn power_state(system_body: &str) -> Option<String> {
    serde_json::from_str::<Value>(system_body)
        .ok()?
        .get("PowerState")?
        .as_str()
        .map(str::to_string)
}

/// Whether a one-time network boot is currently armed, as the service reports it.
///
/// Read back rather than assumed, because a PATCH can succeed and do nothing: PiKVM's
/// handler returns **204 No Content** and ignores the body, so a client that trusts the
/// status believes it armed a boot that will not happen.
pub fn boot_override(system_body: &str) -> (Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<Value>(system_body) else {
        return (None, None);
    };
    let Some(boot) = v.get("Boot") else {
        return (None, None);
    };
    let s = |k: &str| boot.get(k).and_then(Value::as_str).map(str::to_string);
    (
        s("BootSourceOverrideEnabled"),
        s("BootSourceOverrideTarget"),
    )
}

fn split_headers(text: &str) -> (&str, &str) {
    // curl writes each response's headers followed by a blank line. A proxy or a 100-
    // continue can produce more than one block, so take the last separator rather than
    // the first.
    match text.rfind("\r\n\r\n") {
        Some(i) => (&text[..i], &text[i + 4..]),
        None => match text.rfind("\n\n") {
            Some(i) => (&text[..i], &text[i + 2..]),
            None => ("", text),
        },
    }
}

fn header(headers: &str, name: &str) -> Option<String> {
    headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controllers::DEFAULT_BASE;
    use std::path::PathBuf;

    fn controller(tls: Tls) -> Redfish {
        Redfish {
            url: "https://10.0.0.51".to_string(),
            base: DEFAULT_BASE.to_string(),
            user: "root".to_string(),
            pass: "calvin".to_string(),
            system: None,
            tls,
        }
    }

    #[test]
    fn a_password_with_a_quote_and_a_backslash_is_escaped_for_curls_own_syntax() {
        // Written through unescaped it authenticates as something else, and the BMC's
        // answer to that is a 401 — which reads as a wrong password rather than a bug.
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(quote("tab\there"), r#""tab\there""#);
        assert_eq!(quote("line\nbreak"), r#""line\nbreak""#);
    }

    /// The whole reason for `--config -`: `ps` shows the argument vector, and a password
    /// in one is readable by every user on the box through `/proc/<pid>/cmdline`.
    #[test]
    fn the_credential_is_in_the_configuration_and_never_in_the_argument_vector() {
        let c = controller(Tls::Insecure);
        let client = Client::new(&c);
        let joined = argv().join(" ");
        assert_eq!(joined, "curl --config -");
        assert!(!joined.contains("calvin"));

        let config = client.config("GET", "/redfish/v1/Systems", None, None);
        assert!(config.contains("calvin"), "{config}");
    }

    /// `--data` alone sends a form content type, and Redfish answers that with 415.
    #[test]
    fn a_body_carries_an_explicit_json_content_type() {
        let c = controller(Tls::Insecure);
        let config = Client::new(&c).config("POST", "/x", Some(r#"{"ResetType":"On"}"#), None);
        assert!(
            config.contains("Content-Type: application/json"),
            "{config}"
        );
        // And the body is quoted by the same escaper the password is, because it lands in
        // the same file — `--config -` has already taken stdin.
        assert!(
            config.contains(r#"data = "{\"ResetType\":\"On\"}""#),
            "{config}"
        );
    }

    /// It would discard `error.@Message.ExtendedInfo`, which is the most useful thing a
    /// failed call produces.
    #[test]
    fn the_configuration_never_asks_curl_to_fail_quietly() {
        let c = controller(Tls::Insecure);
        let config = Client::new(&c).config("GET", "/x", None, None);
        assert!(!config.lines().any(|l| l.trim() == "fail"), "{config}");
        // And a deadline is not optional: a BMC that accepts a connection and never
        // answers is ordinary.
        assert!(config.contains("max-time = "), "{config}");
    }

    #[test]
    fn each_way_of_trusting_a_certificate_reaches_curl() {
        let insecure = controller(Tls::Insecure);
        assert!(
            Client::new(&insecure)
                .config("GET", "/x", None, None)
                .contains("insecure")
        );

        let ca = controller(Tls::CaCert(PathBuf::from("/etc/bmc-ca.pem")));
        assert!(
            Client::new(&ca)
                .config("GET", "/x", None, None)
                .contains(r#"cacert = "/etc/bmc-ca.pem""#)
        );

        let pinned = controller(Tls::PinnedPubKey("sha256//abc".to_string()));
        assert!(
            Client::new(&pinned)
                .config("GET", "/x", None, None)
                .contains(r#"pinnedpubkey = "sha256//abc""#)
        );
    }

    #[test]
    fn an_etag_becomes_an_if_match_header() {
        // iLO and several iDRAC builds answer a PATCH without one with 412, and the
        // message says "precondition", which sends people looking at the payload.
        let c = controller(Tls::Insecure);
        let with = Client::new(&c).config("PATCH", "/x", Some("{}"), Some("W/\"abc\""));
        assert!(with.contains(r#"If-Match: W/\"abc\""#), "{with}");

        // And where the resource carried none, no header: `If-Match: *` is not universally
        // accepted.
        let without = Client::new(&c).config("PATCH", "/x", Some("{}"), None);
        assert!(!without.contains("If-Match"), "{without}");
    }

    /// PiKVM's own shape, taken from kvmd's source: the body says `/redfish/v1/...` even
    /// when the service is mounted at `/api/redfish/v1`, so only the last segment is safe
    /// to use.
    #[test]
    fn member_ids_are_the_last_segment_and_never_a_path() {
        let body = r#"{"Members":[
            {"@odata.id":"/redfish/v1/Systems/System.Embedded.1"},
            {"@odata.id":"/redfish/v1/Systems/0"},
            {"@odata.id":"/redfish/v1/Systems/SwitchPort0/"}
        ]}"#;
        assert_eq!(
            members(body).expect("members"),
            ["System.Embedded.1", "0", "SwitchPort0"]
        );
    }

    #[test]
    fn a_collection_that_is_not_json_says_so_rather_than_panicking() {
        assert!(members("<html>nope</html>").is_err());
        assert!(members("{}").is_err());
    }

    #[test]
    fn the_reset_list_and_the_power_state_are_read_from_the_system() {
        // Exactly what kvmd emits, so the client is written against a real shape.
        let body = r##"{
            "PowerState": "On",
            "Actions": {"#ComputerSystem.Reset": {
                "ResetType@Redfish.AllowableValues":
                    ["On","ForceOff","GracefulShutdown","ForceRestart","ForceOn","PushPowerButton"]
            }},
            "Boot": {"BootSourceOverrideEnabled":"Disabled","BootSourceOverrideTarget":null}
        }"##;
        assert_eq!(power_state(body).as_deref(), Some("On"));
        assert!(allowable_resets(body).contains(&"ForceRestart".to_string()));
        // PiKVM reports the override as Disabled and its PATCH does nothing — which is
        // why the caller reads this back instead of trusting a status code.
        assert_eq!(boot_override(body).0.as_deref(), Some("Disabled"));
        assert_eq!(boot_override(body).1, None);
    }

    #[test]
    fn a_vendors_error_sentence_survives_to_be_printed() {
        let reply = Reply {
            status: 400,
            body: r#"{"error":{"@Message.ExtendedInfo":[
                {"Message":"The value 'Pxe' for the property BootSourceOverrideTarget is not in the list of acceptable values."}
            ]}}"#
                .to_string(),
            etag: None,
        };
        assert!(
            reply.describe().contains("not in the list"),
            "{}",
            reply.describe()
        );
        assert!(reply.describe().starts_with("HTTP 400"));
    }

    #[test]
    fn an_error_with_no_sentence_still_reports_its_status() {
        let reply = Reply {
            status: 500,
            body: "<html>".to_string(),
            etag: None,
        };
        assert_eq!(reply.describe(), "HTTP 500");
    }

    #[test]
    fn headers_are_split_from_the_body_at_the_last_blank_line() {
        let (h, b) = split_headers("HTTP/1.1 200 OK\r\nETag: \"x\"\r\n\r\n{\"a\":1}");
        assert!(h.contains("ETag"));
        assert_eq!(b, "{\"a\":1}");
        assert_eq!(header(h, "etag"), Some("\"x\"".to_string()));
        // A body containing a blank line must not be truncated by taking the first one.
        let (_, b) = split_headers("H: 1\r\n\r\nline\r\n\r\nmore");
        assert_eq!(b, "more");
    }
}
