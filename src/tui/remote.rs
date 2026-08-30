//! Reading a deployment's fleet over the admin API.
//!
//! **One request, not one per machine.** `GET /machines` returns bare identifiers, so a
//! remote view built on the endpoints that existed would need a `GET /resolve/{id}` for
//! every machine — two thousand round trips on the fleet this project measures itself
//! against, which is not "the same screens over the wire" but a different and much worse
//! program. `GET /fleet` exists for this, and it returns **byte for byte** what
//! `machines --json` prints, from the same producer, so the two cannot drift.
//!
//! **Three screens, honestly labelled.** The admin API has machines, groups and check —
//! and nothing for media, boot or the log. Rather than growing a read API with its own
//! auth-exposed surface, the other panes say so.
//!
//! **Nothing powers anything.** That is refused in [`super::App::on_key`] rather than
//! here, so no screen can forget it.
//!
//! Through `curl`, like `redfish` and `boot::fetch`: there is no TLS in this binary, and
//! the token must stay out of the process table where `ps` would show it.

use crate::redfish::quote;
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// A screen redraw must never wait longer than a person will.
const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Remote {
    base: String,
    token: String,
}

/// Written by hand rather than derived, because the derived one would print the token.
///
/// This credential sets the root password of every machine installed afterwards, and a
/// `{:?}` in a log line or a panic message is exactly how it would escape. The same reason
/// `config::Setting` never carries a secret's value.
impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remote")
            .field("base", &self.base)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl Remote {
    /// `url` is the admin listener — `http://nas:9000`, with no path.
    pub fn new(url: &str, token: &str) -> Result<Remote, String> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(format!(
                "{url:?} needs a scheme, and it is the admin listener's"
            ));
        }
        if token.trim().is_empty() {
            return Err(
                "no admin token: RESCRIPTUM_ADMIN_TOKEN is what this API authenticates with"
                    .to_string(),
            );
        }
        Ok(Remote {
            base: url.trim_end_matches('/').to_string(),
            token: token.to_string(),
        })
    }

    /// The token goes in the option file on stdin, never in `argv` — `ps` would otherwise
    /// show a credential that sets the root password of every machine installed afterwards.
    fn get(&self, path: &str) -> Result<String, String> {
        let mut config = String::new();
        config.push_str(&format!(
            "url = {}\n",
            quote(&format!("{}{path}", self.base))
        ));
        config.push_str(&format!(
            "header = {}\n",
            quote(&format!("Authorization: Bearer {}", self.token))
        ));
        config.push_str(&format!("max-time = {}\n", TIMEOUT.as_secs()));
        config.push_str("silent\nshow-error\n");
        config.push_str("write-out = \"\\n%{http_code}\"\n");

        let mut child = Command::new("curl")
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    "curl is not installed, and there is no TLS in this binary".to_string()
                }
                _ => format!("cannot run curl: {e}"),
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(config.as_bytes())
                .map_err(|e| format!("cannot write curl's options: {e}"))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("curl did not finish: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "{}{path}: {}",
                self.base,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }

        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let (body, status) = text
            .rsplit_once('\n')
            .ok_or_else(|| "curl produced no status".to_string())?;
        match status.trim() {
            "200" => Ok(body.to_string()),
            "401" => Err("401 — the admin token was refused".to_string()),
            "404" => Err(format!(
                "404 — {} has no /fleet, so it is older than this build",
                self.base
            )),
            other => Err(format!("HTTP {other} from {}{path}", self.base)),
        }
    }

    /// The same shape `crate::cli::fleet::machines` produces locally, parsed back.
    pub fn machines(&self) -> Result<Vec<crate::cli::fleet::Machine>, String> {
        let body = self.get("/fleet")?;
        let v: Value = serde_json::from_str(&body).map_err(|e| format!("/fleet: {e}"))?;
        let rows = v
            .get("machines")
            .and_then(Value::as_array)
            .ok_or_else(|| "/fleet: no `machines`".to_string())?;

        Ok(rows
            .iter()
            .map(|m| crate::cli::fleet::Machine {
                id: string(m, "id"),
                formats: m
                    .get("formats")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                group: m.get("group").and_then(Value::as_str).map(str::to_string),
                armed: flag(m, "armed"),
                armed_by_group: flag(m, "armed_by_group"),
                disarmed: flag(m, "disarmed"),
            })
            .collect())
    }

    /// Group names only: the admin API lists identifiers, and inventing the rest would be
    /// the drift `GET /fleet` exists to prevent.
    pub fn groups(&self) -> Result<Vec<crate::cli::fleet::Group>, String> {
        let body = self.get("/groups")?;
        let v: Value = serde_json::from_str(&body).map_err(|e| format!("/groups: {e}"))?;
        let rows = v
            .get("group")
            .and_then(Value::as_array)
            .ok_or_else(|| "/groups: unexpected shape".to_string())?;
        Ok(rows
            .iter()
            .filter_map(|g| g.as_str())
            .map(|name| crate::cli::fleet::Group {
                name: name.to_string(),
                format: String::new(),
                origin: "over the admin API".to_string(),
                members: Vec::new(),
                matchers: Vec::new(),
                extends: Vec::new(),
            })
            .collect())
    }

    pub fn problems(&self) -> Result<Vec<String>, String> {
        let body = self.get("/check")?;
        let v: Value = serde_json::from_str(&body).map_err(|e| format!("/check: {e}"))?;
        Ok(v.get("problems")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn describe(&self) -> String {
        format!("{} (remote, read-only)", self.base)
    }
}

fn string(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_must_be_the_admin_listener_and_the_token_must_exist() {
        assert!(Remote::new("nas:9000", "x".repeat(16).as_str()).is_err());
        let e = Remote::new("http://nas:9000", "  ").expect_err("no token");
        assert!(e.contains("RESCRIPTUM_ADMIN_TOKEN"), "{e}");
        assert!(Remote::new("http://nas:9000/", "token").is_ok());
    }

    /// The remote model must read back what the local one prints, or the two views are
    /// two programs. `tests/admin.rs` asserts the endpoint is byte-identical to
    /// `machines --json`; this asserts this end of it.
    #[test]
    fn the_fleet_payload_parses_into_the_same_shape_as_the_local_model() {
        let body = r#"{"machines":[
            {"id":"98fa9b50d810","formats":["ipxe","toml"],"group":"rack-a",
             "armed":true,"armed_by_group":false,"disarmed":false},
            {"id":"aabbccddeeff","formats":[],"group":null,
             "armed":true,"armed_by_group":true,"disarmed":false}
        ]}"#;
        let v: serde_json::Value = serde_json::from_str(body).expect("json");
        let rows = v["machines"].as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(string(&rows[0], "id"), "98fa9b50d810");
        assert!(flag(&rows[0], "armed"));
        assert!(!flag(&rows[0], "armed_by_group"));
        // The one that cannot disarm itself has to survive the wire, because that is the
        // state an operator most needs to see.
        assert!(flag(&rows[1], "armed_by_group"));
        assert!(rows[1].get("group").expect("key").is_null());
    }

    /// A `{:?}` in a log line or a panic message is exactly how a credential escapes.
    #[test]
    fn debug_never_prints_the_token() {
        let r = Remote::new("http://nas:9000", "s3cr3t-admin-token").expect("valid");
        let shown = format!("{r:?}");
        assert!(!shown.contains("s3cr3t"), "{shown}");
        assert!(shown.contains("nas:9000"), "{shown}");
    }

    #[test]
    fn a_missing_field_is_a_default_rather_than_a_panic() {
        let v: serde_json::Value = serde_json::from_str(r#"{"id":"x"}"#).expect("json");
        assert_eq!(string(&v, "nothing"), "");
        assert!(!flag(&v, "nothing"));
    }
}
