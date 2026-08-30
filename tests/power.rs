//! The Redfish client, against a Redfish service that actually answers.
//!
//! There is no BMC in CI and no PiKVM either, so the service here is a few hundred lines
//! of `TcpListener` shaped like the real ones — and crucially the **real `curl`** is what
//! talks to it. That is the half worth testing without hardware: the option file, the
//! quoting, the status/body split, the etag round trip, and what a timeout is reported as.
//! Everything a vendor's own firmware decides still has to be confirmed on a board.
//!
//! Plain HTTP, because TLS is exactly the part curl is trusted with rather than tested
//! here.

use rescriptum::controllers::{DEFAULT_BASE, Redfish, Tls};
use rescriptum::redfish::{Client, Failed};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How the fake service should behave for one test.
#[derive(Clone)]
struct Behaviour {
    /// Members of `/Systems`, as `@odata.id` values.
    members: Vec<String>,
    /// Sent on the system resource, when set.
    etag: Option<String>,
    /// What `Boot` reports. PiKVM answers a PATCH 204 and leaves this at Disabled.
    boot: Arc<Mutex<(String, Option<String>)>>,
    /// Whether a PATCH actually changes anything — false is PiKVM's behaviour.
    patch_takes_effect: bool,
    /// Answer nothing at all, so curl hits its deadline.
    stall: bool,
    /// Fail the reset with this vendor sentence.
    reset_error: Option<String>,
}

impl Default for Behaviour {
    fn default() -> Behaviour {
        Behaviour {
            members: vec!["/redfish/v1/Systems/System.Embedded.1".to_string()],
            etag: None,
            boot: Arc::new(Mutex::new(("Disabled".to_string(), None))),
            patch_takes_effect: true,
            stall: false,
            reset_error: None,
        }
    }
}

struct Service {
    url: String,
    behaviour: Behaviour,
    /// Every `If-Match` the service was sent, so a test can assert one arrived.
    if_match: Arc<Mutex<Vec<String>>>,
    resets: Arc<Mutex<Vec<String>>>,
}

impl Service {
    fn start(behaviour: Behaviour) -> Service {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let if_match = Arc::new(Mutex::new(Vec::new()));
        let resets = Arc::new(Mutex::new(Vec::new()));

        let b = behaviour.clone();
        let seen = Arc::clone(&if_match);
        let got = Arc::clone(&resets);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let b = b.clone();
                let seen = Arc::clone(&seen);
                let got = Arc::clone(&got);
                thread::spawn(move || serve(stream, &b, &seen, &got));
            }
        });

        Service {
            url: format!("http://127.0.0.1:{port}"),
            behaviour,
            if_match,
            resets,
        }
    }

    fn controller(&self) -> Redfish {
        Redfish {
            url: self.url.clone(),
            base: DEFAULT_BASE.to_string(),
            user: "root".to_string(),
            // A password carrying both characters curl's config syntax cares about, in
            // every test rather than only in one — if the escaping breaks, everything
            // here starts answering 401 and says so.
            pass: r#"a"b\c"#.to_string(),
            system: None,
            tls: Tls::Insecure,
        }
    }
}

fn serve(
    mut stream: TcpStream,
    b: &Behaviour,
    if_match: &Mutex<Vec<String>>,
    resets: &Mutex<Vec<String>>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut length = 0usize;
    let mut authorized = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            match k.as_str() {
                "content-length" => length = v.parse().unwrap_or(0),
                "if-match" => if_match.lock().expect("lock").push(v),
                // `root` with the password above, base64. Checked rather than ignored, so
                // that a quoting bug in the option file shows up as a 401 here instead of
                // passing silently.
                "authorization" => authorized = v == format!("Basic {}", basic()),
                _ => {}
            }
        }
    }

    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();

    if b.stall {
        // Accept, say nothing, and let curl's deadline decide. This is a BMC that is
        // reachable and wedged, which is an ordinary state.
        thread::sleep(Duration::from_secs(30));
        return;
    }

    if !authorized {
        let _ = reply(
            &mut stream,
            401,
            None,
            r#"{"error":{"message":"bad credential"}}"#,
        );
        return;
    }

    let systems = format!("{DEFAULT_BASE}/Systems");
    if method == "GET" && path == systems {
        let members: Vec<String> = b
            .members
            .iter()
            .map(|m| format!(r#"{{"@odata.id":"{m}"}}"#))
            .collect();
        let payload = format!(
            r#"{{"Members":[{}],"Members@odata.count":{}}}"#,
            members.join(","),
            members.len()
        );
        let _ = reply(&mut stream, 200, None, &payload);
        return;
    }

    let reset_suffix = "/Actions/ComputerSystem.Reset";
    if method == "POST" && path.ends_with(reset_suffix) {
        let kind = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("ResetType")?.as_str().map(str::to_string))
            .unwrap_or_default();
        resets.lock().expect("lock").push(kind);
        match &b.reset_error {
            Some(message) => {
                let payload = format!(
                    r#"{{"error":{{"@Message.ExtendedInfo":[{{"Message":"{message}"}}]}}}}"#
                );
                let _ = reply(&mut stream, 400, None, &payload);
            }
            None => {
                let _ = reply(&mut stream, 204, None, "");
            }
        }
        return;
    }

    if path.starts_with(&systems) {
        if method == "PATCH" {
            if b.patch_takes_effect
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&body)
                && let Some(boot) = v.get("Boot")
            {
                let mut held = b.boot.lock().expect("lock");
                if let Some(e) = boot
                    .get("BootSourceOverrideEnabled")
                    .and_then(|s| s.as_str())
                {
                    held.0 = e.to_string();
                }
                held.1 = boot
                    .get("BootSourceOverrideTarget")
                    .and_then(|s| s.as_str())
                    .map(str::to_string);
            }
            // 204 whether or not anything changed — which is exactly PiKVM's behaviour,
            // and the reason the client reads the state back.
            let _ = reply(&mut stream, 204, None, "");
            return;
        }

        let (enabled, target) = b.boot.lock().expect("lock").clone();
        let target = match target {
            Some(t) => format!("\"{t}\""),
            None => "null".to_string(),
        };
        let payload = format!(
            r##"{{"PowerState":"Off",
                 "Actions":{{"#ComputerSystem.Reset":{{
                    "ResetType@Redfish.AllowableValues":["On","ForceOff","ForceRestart"]}}}},
                 "Boot":{{"BootSourceOverrideEnabled":"{enabled}","BootSourceOverrideTarget":{target}}}}}"##
        );
        let _ = reply(&mut stream, 200, b.etag.as_deref(), &payload);
        return;
    }

    let _ = reply(
        &mut stream,
        404,
        None,
        r#"{"error":{"message":"no such resource"}}"#,
    );
}

fn basic() -> String {
    // Base64 of `root:a"b\c`, hand-rolled rather than pulling a crate in for six bytes.
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = br#"root:a"b\c"#;
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn reply(
    stream: &mut TcpStream,
    status: u16,
    etag: Option<&str>,
    body: &str,
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(tag) = etag {
        head.push_str(&format!("ETag: {tag}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn scratch() -> usize {
    static N: AtomicUsize = AtomicUsize::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

// ---- discovery ------------------------------------------------------------

#[test]
fn one_system_is_discovered_and_named_by_its_last_segment() {
    let s = Service::start(Behaviour::default());
    let c = s.controller();
    let id = Client::new(&c).system_id().expect("discovery");
    // Not the path out of the body: the id, which is what `<base>/Systems/<id>` is built
    // from. PiKVM emits `/redfish/v1/...` even when mounted at `/api/redfish/v1`.
    assert_eq!(id, "System.Embedded.1");
    let _ = scratch();
}

/// A blade chassis, a Dell FX2, and a PiKVM with a switch all do this. Picking the first
/// would power somebody else's machine.
#[test]
fn several_systems_are_refused_by_name_rather_than_guessed_between() {
    let s = Service::start(Behaviour {
        members: vec![
            "/redfish/v1/Systems/0".to_string(),
            "/redfish/v1/Systems/SwitchPort0".to_string(),
        ],
        ..Default::default()
    });
    let c = s.controller();
    let e = Client::new(&c).system_id().expect_err("must refuse");
    let said = e.to_string();
    assert!(said.contains('0'), "{said}");
    assert!(said.contains("SwitchPort0"), "{said}");
    assert!(
        said.contains("system ="),
        "it must say how to resolve this: {said}"
    );
}

#[test]
fn an_explicit_system_skips_discovery_entirely() {
    let s = Service::start(Behaviour {
        members: vec!["/redfish/v1/Systems/0".to_string(), "/x/1".to_string()],
        ..Default::default()
    });
    let mut c = s.controller();
    c.system = Some("SwitchPort3".to_string());
    assert_eq!(
        Client::new(&c).system_id().expect("explicit"),
        "SwitchPort3"
    );
}

// ---- credentials ----------------------------------------------------------

/// The service checks Basic auth against a password carrying `"` and `\`. If the option
/// file's quoting were wrong, every test in this file would 401.
#[test]
fn a_password_with_a_quote_and_a_backslash_authenticates() {
    let s = Service::start(Behaviour::default());
    let c = s.controller();
    assert!(Client::new(&c).system_id().is_ok());

    let mut wrong = s.controller();
    wrong.pass = "not-it".to_string();
    let e = Client::new(&wrong).system_id().expect_err("must fail");
    assert!(e.to_string().contains("401"), "{e}");
}

// ---- errors ---------------------------------------------------------------

/// `--fail` would have thrown this away, and the operator would be reading a packet
/// capture instead of a sentence.
#[test]
fn a_vendors_error_sentence_reaches_the_caller() {
    let s = Service::start(Behaviour {
        reset_error: Some("The value 'Nope' is not in the list of acceptable values.".to_string()),
        ..Default::default()
    });
    let c = s.controller();
    let e = Client::new(&c)
        .reset("System.Embedded.1", "ForceOff")
        .expect_err("must fail");
    let said = e.to_string();
    assert!(said.contains("HTTP 400"), "{said}");
    assert!(said.contains("acceptable values"), "{said}");
}

/// Naming the ones that would work beats reporting the 400 that a wrong one earns.
#[test]
fn a_reset_the_system_does_not_offer_is_refused_before_it_is_sent() {
    let s = Service::start(Behaviour::default());
    let c = s.controller();
    let e = Client::new(&c)
        .reset("System.Embedded.1", "GracefulShutdown")
        .expect_err("must refuse");
    let said = e.to_string();
    assert!(
        said.contains("ForceOff"),
        "it must name what is offered: {said}"
    );
    assert!(
        s.resets.lock().expect("lock").is_empty(),
        "nothing should have been sent"
    );
}

#[test]
fn a_reset_the_system_offers_is_sent_as_written() {
    let s = Service::start(Behaviour::default());
    let c = s.controller();
    Client::new(&c)
        .reset("System.Embedded.1", "On")
        .expect("reset");
    assert_eq!(*s.resets.lock().expect("lock"), ["On"]);
}

/// A deadline says when to stop waiting, not what happened. This is the one message that
/// must not imply the request did nothing.
#[test]
fn a_stalled_service_is_reported_as_an_unknown_outcome() {
    let s = Service::start(Behaviour {
        stall: true,
        ..Default::default()
    });
    let c = s.controller();
    let e = Client::new(&c)
        .with_timeout(Duration::from_secs(1))
        .system_id()
        .expect_err("must time out");
    assert!(matches!(e, Failed::Unknown(_)), "{e:?}");
    let said = e.to_string();
    assert!(said.contains("outcome is unknown"), "{said}");
    assert!(said.contains("read the state back"), "{said}");
}

// ---- the boot override ----------------------------------------------------

#[test]
fn an_etag_is_read_from_the_system_and_sent_back_as_if_match() {
    // iLO and several iDRAC builds answer a PATCH without one with 412.
    let s = Service::start(Behaviour {
        etag: Some("W/\"abc123\"".to_string()),
        ..Default::default()
    });
    let c = s.controller();
    assert!(
        Client::new(&c)
            .set_pxe_once("System.Embedded.1")
            .expect("patch"),
        "the override should have taken"
    );
    let seen = s.if_match.lock().expect("lock").clone();
    assert_eq!(
        seen,
        ["W/\"abc123\""],
        "the etag must come back as If-Match"
    );
}

#[test]
fn no_etag_means_no_if_match_header_rather_than_a_wildcard() {
    // `If-Match: *` is not universally accepted.
    let s = Service::start(Behaviour::default());
    let c = s.controller();
    Client::new(&c)
        .set_pxe_once("System.Embedded.1")
        .expect("patch");
    assert!(s.if_match.lock().expect("lock").is_empty());
}

/// **The find that makes the read-back mandatory.** PiKVM's PATCH handler returns 204 and
/// does nothing at all, while reporting the override as Disabled. A client that trusts the
/// status code believes it armed a boot that will never happen — and the machine then
/// installs nothing while looking correct.
#[test]
fn a_patch_that_answers_204_and_does_nothing_is_caught_by_reading_it_back() {
    let s = Service::start(Behaviour {
        patch_takes_effect: false,
        ..Default::default()
    });
    let c = s.controller();
    let armed = Client::new(&c)
        .set_pxe_once("System.Embedded.1")
        .expect("the PATCH itself succeeds");
    assert!(
        !armed,
        "204 must not be taken as proof: the service still reports the override disabled"
    );
}

#[test]
fn the_override_is_once_and_never_continuous() {
    // An override consumed at the next boot means a machine that fails to install and
    // reboots comes up on its own disk rather than installing again.
    let s = Service::start(Behaviour::default());
    let c = s.controller();
    assert!(
        Client::new(&c)
            .set_pxe_once("System.Embedded.1")
            .expect("patch")
    );
    let (enabled, target) = s.behaviour.boot.lock().expect("lock").clone();
    assert_eq!(enabled, "Once");
    assert_eq!(target.as_deref(), Some("Pxe"));
}
