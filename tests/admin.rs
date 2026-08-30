//! The admin API, against the real binary: authentication, writes, and the promise that
//! a write can never leave the answer set broken.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOKEN: &str = "0123456789abcdef-test-token";

struct Server {
    child: Child,
    answer_addr: String,
    admin_addr: String,
    dir: PathBuf,
    /// Everything the server said on stderr, so a startup warning can be asserted on.
    log: Arc<Mutex<Vec<String>>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn scratch(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("pve-admin-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("scratch dir");
    p
}

impl Server {
    fn start() -> Server {
        Server::boot(scratch("run"), &[])
    }

    /// A server over a database seeded from a directory of answers — the only way to
    /// reach a state the API itself refuses to create.
    fn start_seeded(files: &[(&str, &str)]) -> Server {
        let dir = scratch("seeded");
        let answers = dir.join("answers");
        fs::create_dir_all(&answers).expect("answers dir");
        for (name, contents) in files {
            common::seed(&answers, name, contents);
        }
        let imported = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
            .env("RESCRIPTUM_STORE", "sqlite")
            .env("RESCRIPTUM_DB_PATH", dir.join("answers.db"))
            .args(["import", answers.to_str().expect("utf-8 path")])
            .output()
            .expect("import");
        assert!(imported.status.success(), "seeding the database failed");
        Server::boot(dir, &[])
    }

    fn boot(dir: PathBuf, extra: &[(&str, &str)]) -> Server {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
        cmd.env("RESCRIPTUM_STORE", "sqlite")
            .env("RESCRIPTUM_DB_PATH", dir.join("answers.db"))
            .env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
            .env("RESCRIPTUM_ADMIN_ADDR", "127.0.0.1:0")
            .env("RESCRIPTUM_ADMIN_TOKEN", TOKEN)
            .env("RESCRIPTUM_TIMEOUT_SECS", "5")
            .stderr(Stdio::piped())
            .stdout(Stdio::null());
        for (key, value) in extra {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().expect("spawn server");

        let stderr = child.stderr.take().expect("piped stderr");
        let mut lines = BufReader::new(stderr).lines();
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut answer_addr = None;
        let mut admin_addr = None;
        // Both listeners announce themselves; take whichever order they arrive in.
        for _ in 0..8 {
            let Some(Ok(line)) = lines.next() else { break };
            if let Some(rest) = line.split("admin API listening on ").nth(1) {
                admin_addr = rest.split_whitespace().next().map(str::to_string);
            } else if let Some(rest) = line.split("listening on ").nth(1) {
                answer_addr = rest.split_whitespace().next().map(str::to_string);
            }
            log.lock().unwrap().push(line);
            if answer_addr.is_some() && admin_addr.is_some() {
                break;
            }
        }
        let collected = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in lines.map_while(Result::ok) {
                collected.lock().unwrap().push(line);
            }
        });

        Server {
            child,
            answer_addr: answer_addr.expect("answer listener never announced"),
            admin_addr: admin_addr.expect("admin listener never announced"),
            dir,
            log,
        }
    }

    fn startup_log(&self) -> String {
        self.log.lock().unwrap().join("\n")
    }

    fn raw(addr: &str, request: &str, body: &[u8]) -> String {
        let mut sock = TcpStream::connect(addr).expect("connect");
        sock.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        sock.write_all(request.as_bytes()).expect("write head");
        sock.write_all(body).expect("write body");
        sock.flush().unwrap();
        let mut out = String::new();
        sock.read_to_string(&mut out).expect("read response");
        out
    }

    /// An authenticated admin request.
    fn admin(&self, method: &str, path: &str, body: &str) -> String {
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: admin\r\nAuthorization: Bearer {TOKEN}\r\n\
             Content-Length: {}\r\n\r\n",
            body.len()
        );
        Server::raw(&self.admin_addr, &head, body.as_bytes())
    }

    /// An admin request with whatever authorization header you like.
    fn admin_with_auth(&self, path: &str, auth: Option<&str>) -> String {
        let auth_line = match auth {
            Some(a) => format!("Authorization: {a}\r\n"),
            None => String::new(),
        };
        let head =
            format!("GET {path} HTTP/1.1\r\nHost: admin\r\n{auth_line}Content-Length: 0\r\n\r\n");
        Server::raw(&self.admin_addr, &head, b"")
    }

    /// What the installer would get.
    fn ask(&self, mac: &str) -> String {
        let body = format!(r#"{{"network_interfaces":[{{"mac":"{mac}"}}]}}"#);
        let head = format!(
            "POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        Server::raw(&self.answer_addr, &head, body.as_bytes())
    }
}

fn status(response: &str) -> u16 {
    response
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0)
}

fn body_of(response: &str) -> &str {
    response.split("\r\n\r\n").nth(1).unwrap_or("")
}

// ---- authentication -------------------------------------------------------

#[test]
fn the_api_refuses_anyone_without_the_token() {
    // Four failures — deliberately under the lockout threshold. Adding a fifth here
    // would trip the guard and make this test about something else.
    let s = Server::start();
    assert_eq!(status(&s.admin_with_auth("/machines", None)), 401);
    assert_eq!(
        status(&s.admin_with_auth("/machines", Some("Bearer wrong-token-entirely"))),
        401
    );
    // A token of the right length but wrong content must fare no better.
    let near = format!("Bearer {}x", &TOKEN[..TOKEN.len() - 1]);
    assert_eq!(status(&s.admin_with_auth("/machines", Some(&near))), 401);
    assert_eq!(
        status(&s.admin_with_auth("/machines", Some("Basic abc"))),
        401
    );
}

#[test]
fn guessing_at_the_token_gets_you_shut_out() {
    // The token is long enough that brute force is hopeless anyway; this makes trying
    // expensive rather than merely futile, and puts the attempt in the log.
    let s = Server::start();

    let mut blocked_after = None;
    for attempt in 1..=8 {
        let r = s.admin_with_auth("/machines", Some("Bearer wrong-token-here-0000"));
        match status(&r) {
            401 => {}
            429 => {
                blocked_after = Some(attempt);
                break;
            }
            other => panic!("attempt {attempt}: unexpected {other}\n{r}"),
        }
    }
    let blocked_after = blocked_after.expect("repeated bad tokens should earn a block");
    assert!(blocked_after <= 6, "took {blocked_after} attempts to block");

    // And the block must hold even for the *correct* token — otherwise guessing until
    // you get it right would cost nothing at all.
    let r = s.admin("GET", "/machines", "");
    assert_eq!(status(&r), 429, "a block must survive a correct token: {r}");
    assert!(
        r.to_ascii_lowercase().contains("retry-after"),
        "should say how long: {r}"
    );

    // Liveness stays available, so a monitor does not go dark during an attack.
    assert_eq!(status(&s.admin_with_auth("/health", None)), 200);
}

#[test]
fn health_needs_no_token_so_a_monitor_can_use_it() {
    let s = Server::start();
    assert_eq!(status(&s.admin_with_auth("/health", None)), 200);
}

// ---- the basic round trip -------------------------------------------------

#[test]
fn a_machine_can_be_written_read_and_removed() {
    let s = Server::start();

    let r = s.admin(
        "PUT",
        "/machines/98-fa-9b-50-d8-10",
        "[global]\nkeyboard = \"fr\"\n",
    );
    assert_eq!(status(&r), 200, "{r}");

    let r = s.admin("GET", "/machines/98-fa-9b-50-d8-10", "");
    assert_eq!(status(&r), 200, "{r}");
    assert!(body_of(&r).contains("keyboard"), "{r}");

    let r = s.admin("GET", "/machines", "");
    assert!(body_of(&r).contains("98-fa-9b-50-d8-10"), "{r}");

    assert_eq!(
        status(&s.admin("DELETE", "/machines/98-fa-9b-50-d8-10", "")),
        200
    );
    assert_eq!(
        status(&s.admin("GET", "/machines/98-fa-9b-50-d8-10", "")),
        404
    );
    // Deleting it twice is a 404, not a pretend success.
    assert_eq!(
        status(&s.admin("DELETE", "/machines/98-fa-9b-50-d8-10", "")),
        404
    );
}

#[test]
fn a_change_reaches_the_installer_immediately() {
    let s = Server::start();
    assert_eq!(status(&s.ask("98:fa:9b:50:d8:10")), 404);

    s.admin("PUT", "/machines/98fa9b50d810", "marker = \"live\"\n");
    let r = s.ask("98:FA:9B:50:D8:10");
    assert_eq!(status(&r), 200, "{r}");
    assert!(body_of(&r).contains("live"), "{r}");

    s.admin("DELETE", "/machines/98fa9b50d810", "");
    assert_eq!(status(&s.ask("98:fa:9b:50:d8:10")), 404);
}

#[test]
fn groups_and_overrides_compose_through_the_api() {
    let s = Server::start();
    s.admin(
        "PUT",
        "/groups/rack-a",
        "members = [\"98:fa:9b:50:d8:10\"]\n[global]\ncountry = \"fr\"\nkeyboard = \"fr\"\n",
    );
    s.admin(
        "PUT",
        "/machines/98-fa-9b-50-d8-10",
        "[global]\nkeyboard = \"us\"\n",
    );

    let r = s.admin("GET", "/resolve/98:fa:9b:50:d8:10", "");
    assert_eq!(status(&r), 200, "{r}");
    assert!(body_of(&r).contains("\"us\""), "machine must win: {r}");
    assert!(body_of(&r).contains("country"), "group must survive: {r}");
    // And it is the same thing the installer receives.
    assert_eq!(body_of(&s.ask("98:fa:9b:50:d8:10")), body_of(&r));
}

// ---- input the API must refuse --------------------------------------------

#[test]
fn malformed_toml_is_refused_at_write_time() {
    // Storing it would turn into a 500 the next time a machine asked.
    let s = Server::start();
    let r = s.admin("PUT", "/machines/98fa9b50d810", "this is = = not toml\n");
    assert_eq!(status(&r), 400, "{r}");
    assert!(body_of(&r).contains("invalid TOML"), "{r}");
    assert_eq!(status(&s.admin("GET", "/machines/98fa9b50d810", "")), 404);
}

#[test]
fn identifiers_that_could_escape_the_directory_are_refused() {
    // These are written out as filenames by `export`.
    let s = Server::start();
    for id in ["..", "%2e%2e", "a%2Fb", "with%20space"] {
        let r = s.admin("PUT", &format!("/machines/{id}"), "x = 1\n");
        assert!(
            status(&r) == 400 || status(&r) == 404,
            "{id} should be refused, got {r}"
        );
    }
}

#[test]
fn an_unknown_endpoint_is_a_404() {
    let s = Server::start();
    assert_eq!(status(&s.admin("GET", "/nope", "")), 404);
    assert_eq!(status(&s.admin("POST", "/machines", "")), 404);
}

// ---- the promise: a write never leaves the answer set broken ---------------

#[test]
fn a_write_that_would_create_a_cycle_is_refused_and_rolled_back() {
    let s = Server::start();
    s.admin(
        "PUT",
        "/groups/a",
        "members = [\"98:fa:9b:50:d8:10\"]\n[global]\nx = 1\n",
    );
    s.admin("PUT", "/groups/b", "extends = \"a\"\n[global]\ny = 2\n");
    // Everything is healthy so far.
    assert_eq!(status(&s.ask("98:fa:9b:50:d8:10")), 200);

    // Closing the loop would drop both groups and 404 the whole rack.
    let r = s.admin(
        "PUT",
        "/groups/a",
        "extends = \"b\"\nmembers = [\"98:fa:9b:50:d8:10\"]\n",
    );
    assert_eq!(status(&r), 409, "{r}");
    assert!(body_of(&r).contains("cycle"), "{r}");

    // Rolled back: the rack still installs, and the old document is intact.
    assert_eq!(
        status(&s.ask("98:fa:9b:50:d8:10")),
        200,
        "rack should still install"
    );
    let a = s.admin("GET", "/groups/a", "");
    assert!(
        !body_of(&a).contains("extends"),
        "the bad write should be gone: {a}"
    );
}

#[test]
fn deleting_a_group_something_still_extends_is_refused_and_rolled_back() {
    let s = Server::start();
    s.admin("PUT", "/groups/base", "[global]\ncountry = \"fr\"\n");
    s.admin(
        "PUT",
        "/groups/rack-a",
        "extends = \"base\"\nmembers = [\"98:fa:9b:50:d8:10\"]\n",
    );
    assert_eq!(status(&s.ask("98:fa:9b:50:d8:10")), 200);

    let r = s.admin("DELETE", "/groups/base", "");
    assert_eq!(
        status(&r),
        409,
        "removing a base in use must be refused: {r}"
    );

    // Still there, still serving.
    assert_eq!(status(&s.admin("GET", "/groups/base", "")), 200);
    assert_eq!(status(&s.ask("98:fa:9b:50:d8:10")), 200);
}

#[test]
fn a_machine_extending_a_group_that_does_not_exist_is_refused() {
    let s = Server::start();
    let r = s.admin("PUT", "/machines/98fa9b50d810", "extends = \"ghost\"\n");
    // The document parses, so this is caught by the post-write check, not the parser.
    assert_eq!(status(&r), 409, "{r}");
    assert_eq!(status(&s.admin("GET", "/machines/98fa9b50d810", "")), 404);
}

// ---- refusing to start in an unsafe shape ---------------------------------

fn start_expecting_failure(env: &[(&str, &str)]) -> String {
    let dir = scratch("bad");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
    cmd.env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
        .env("RESCRIPTUM_DB_PATH", dir.join("answers.db"))
        .env("RESCRIPTUM_ANSWERS_DIR", &dir)
        .stderr(Stdio::piped())
        .stdout(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run server");
    let _ = fs::remove_dir_all(&dir);
    assert!(
        !out.status.success(),
        "the server should have refused to start"
    );
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn the_server_refuses_to_start_with_an_unauthenticated_admin_api() {
    let err = start_expecting_failure(&[
        ("RESCRIPTUM_STORE", "sqlite"),
        ("RESCRIPTUM_ADMIN_ADDR", "127.0.0.1:0"),
    ]);
    assert!(err.contains("RESCRIPTUM_ADMIN_TOKEN"), "{err}");
}

#[test]
fn the_server_refuses_a_guessable_admin_token() {
    let err = start_expecting_failure(&[
        ("RESCRIPTUM_STORE", "sqlite"),
        ("RESCRIPTUM_ADMIN_ADDR", "127.0.0.1:0"),
        ("RESCRIPTUM_ADMIN_TOKEN", "hunter2"),
    ]);
    assert!(err.contains("16"), "{err}");
}

#[test]
fn the_server_refuses_the_admin_api_over_the_file_store() {
    // Two ways to edit the same configuration, racing each other, is not a feature.
    let err = start_expecting_failure(&[
        ("RESCRIPTUM_STORE", "files"),
        ("RESCRIPTUM_ADMIN_ADDR", "127.0.0.1:0"),
        ("RESCRIPTUM_ADMIN_TOKEN", "0123456789abcdef0"),
    ]);
    assert!(err.contains("sqlite"), "{err}");
}

// ---- formats over the wire ------------------------------------------------

#[test]
fn a_document_is_stored_under_the_format_it_was_written_in() {
    // A document is keyed by (identifier, format), and `?format=` is the only way the
    // API is told which. Defaulting to TOML is what keeps a pre-format client working.
    let s = Server::start();

    assert_eq!(
        status(&s.admin(
            "PUT",
            "/machines/98fa9b50d810?format=preseed",
            "d-i marker string as-debian\n"
        )),
        200
    );

    let back = s.admin("GET", "/machines/98fa9b50d810?format=preseed", "");
    assert_eq!(status(&back), 200, "{back}");
    assert!(body_of(&back).contains("as-debian"), "{back}");

    // Without `?format=`, the request is about the TOML answer — which does not exist.
    assert_eq!(
        status(&s.admin("GET", "/machines/98fa9b50d810", "")),
        404,
        "the default format must not fall through to another one"
    );
}

#[test]
fn one_machine_is_two_answers_and_deleting_one_leaves_the_other() {
    // The trap this pins: an earlier `put` deleted the other formats of a stem, on the
    // theory that a machine has one answer. It has one answer *per operating system*.
    let s = Server::start();
    assert_eq!(
        status(&s.admin("PUT", "/machines/98fa9b50d810", "marker = \"as-proxmox\"\n")),
        200
    );
    assert_eq!(
        status(&s.admin(
            "PUT",
            "/machines/98fa9b50d810?format=preseed",
            "d-i marker string as-debian\n"
        )),
        200
    );

    // The listing is of (id, format) pairs, so the identifier appears once per format.
    let listing = s.admin("GET", "/machines", "");
    assert_eq!(
        body_of(&listing).matches("98fa9b50d810").count(),
        2,
        "{listing}"
    );

    // Both are really servable, each at its own endpoint.
    let toml = s.admin(
        "GET",
        "/resolve?path=/proxmox/answer&mac=98:fa:9b:50:d8:10",
        "",
    );
    assert!(body_of(&toml).contains("as-proxmox"), "{toml}");
    let preseed = s.admin(
        "GET",
        "/resolve?path=/debian/preseed&mac=98:fa:9b:50:d8:10",
        "",
    );
    assert!(body_of(&preseed).contains("as-debian"), "{preseed}");

    assert_eq!(
        status(&s.admin("DELETE", "/machines/98fa9b50d810?format=preseed", "")),
        200
    );
    assert_eq!(
        status(&s.admin("GET", "/machines/98fa9b50d810?format=preseed", "")),
        404
    );
    assert_eq!(
        status(&s.admin("GET", "/machines/98fa9b50d810", "")),
        200,
        "deleting one operating system must not delete the other"
    );
}

#[test]
fn groups_and_defaults_carry_a_format_too() {
    let s = Server::start();
    assert_eq!(
        status(&s.admin(
            "PUT",
            "/groups/base?format=preseed",
            "d-i base string shared\n"
        )),
        200
    );
    assert_eq!(
        status(&s.admin(
            "PUT",
            "/default?format=preseed",
            "d-i marker string fallback\n"
        )),
        200
    );

    // A TOML default is a different document, and does not answer a preseed fetch.
    assert_eq!(
        status(&s.admin("PUT", "/default", "marker = \"toml-fallback\"\n")),
        200
    );

    let preseed = s.admin(
        "GET",
        "/resolve?path=/debian/preseed&mac=de:ad:be:ef:00:01",
        "",
    );
    assert!(body_of(&preseed).contains("fallback"), "{preseed}");
    assert!(!body_of(&preseed).contains("toml-fallback"), "{preseed}");

    let toml = s.admin(
        "GET",
        "/resolve?path=/proxmox/answer&mac=de:ad:be:ef:00:01",
        "",
    );
    assert!(body_of(&toml).contains("toml-fallback"), "{toml}");
}

#[test]
fn a_format_nothing_can_serve_is_refused_at_the_boundary() {
    // A document in a format no endpoint asks for could be stored and never served.
    let s = Server::start();
    for format in ["txt", "conf", "md", ""] {
        let r = s.admin(
            "PUT",
            &format!("/machines/98fa9b50d810?format={format}"),
            "whatever\n",
        );
        assert_eq!(status(&r), 400, "?format={format}: {r}");
    }
}

#[test]
fn resolving_by_path_ignores_the_identifier_in_the_url() {
    // Documented, and easy to trip over: with a query string present the facts come
    // from the query alone, so `?format=` on /resolve/{id} resolves nothing at all.
    let s = Server::start();
    assert_eq!(
        status(&s.admin("PUT", "/machines/98fa9b50d810", "marker = \"m\"\n")),
        200
    );

    let bare = s.admin("GET", "/resolve/98:fa:9b:50:d8:10", "");
    assert_eq!(status(&bare), 200, "{bare}");
    assert!(body_of(&bare).contains("marker"), "{bare}");

    let with_query = s.admin("GET", "/resolve/98:fa:9b:50:d8:10?format=toml", "");
    assert_eq!(
        status(&with_query),
        404,
        "a query string replaces the identifier rather than refining it\n{with_query}"
    );

    // The identity has to go in the query instead.
    let by_label = s.admin("GET", "/resolve?mac=98:fa:9b:50:d8:10", "");
    assert_eq!(status(&by_label), 200, "{by_label}");
}

#[test]
fn a_successful_write_answers_with_the_source_it_would_serve() {
    let s = Server::start();
    assert_eq!(
        status(&s.admin("PUT", "/machines/98fa9b50d810", "marker = \"m\"\n")),
        200
    );
    let r = s.admin("GET", "/resolve/98:fa:9b:50:d8:10", "");
    assert!(
        r.to_ascii_lowercase()
            .contains("x-answer-source: format=toml"),
        "the provenance header is what makes a resolve reviewable\n{r}"
    );
}

// ---- promises the documentation makes -------------------------------------

#[test]
fn every_admin_response_closes_the_connection() {
    // Without this a client waits out the whole connection timeout on every call: the
    // suite once took 30 s instead of 0.4 s, and the eventual drop sometimes arrived as a
    // reset rather than a clean EOF. Slow and flaky, and nothing said why.
    let s = Server::start();
    for r in [
        s.admin("GET", "/machines", ""),
        s.admin("PUT", "/machines/98fa9b50d810", "marker = \"m\"\n"),
        s.admin("GET", "/nowhere", ""),
        s.admin_with_auth("/machines", None),
        s.admin_with_auth("/health", None),
    ] {
        assert!(r.to_ascii_lowercase().contains("connection: close"), "{r}");
    }
}

#[test]
fn a_document_larger_than_the_cap_is_refused() {
    let s = Server::start();
    let huge = format!("marker = \"{}\"\n", "x".repeat(300 * 1024));
    let r = s.admin("PUT", "/machines/98fa9b50d810", &huge);
    assert_eq!(status(&r), 413, "{}", &r[..r.len().min(200)]);

    // And the server is still there afterwards.
    assert_eq!(status(&s.admin("GET", "/machines", "")), 200);
}

#[test]
fn a_body_that_is_not_utf8_is_refused_rather_than_stored() {
    // A document is text by the time it reaches a parser; bytes that are not text have to
    // stop before that, with a reason rather than a mangled document.
    let s = Server::start();
    let head = format!(
        "PUT /machines/98fa9b50d810 HTTP/1.1\r\nHost: admin\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Length: 6\r\n\r\n"
    );
    let r = Server::raw(&s.admin_addr, &head, &[0xff, 0xfe, 0x00, 0x80, 0xff, 0xfe]);
    assert_eq!(status(&r), 400, "{r}");
    assert!(r.to_ascii_lowercase().contains("utf-8"), "{r}");
}

#[test]
fn a_successful_write_still_reports_what_was_already_broken() {
    // "A clean response never implies the whole set is healthy — only that you did not
    // make it worse." The guard refuses to *create* this state, so it has to be seeded.
    let s = Server::start_seeded(&[(
        "98fa9b50d810.toml",
        "extends = \"gone\"\n\n[global]\nx = 1\n",
    )]);

    let r = s.admin("PUT", "/groups/rack-a", "[global]\nkeyboard = \"fr\"\n");
    assert_eq!(status(&r), 200, "an unrelated write must succeed: {r}");
    assert!(
        body_of(&r).contains("extends unknown group"),
        "the pre-existing problem has to be in the response\n{r}"
    );
}

#[test]
fn binding_the_admin_api_beyond_loopback_is_said_out_loud() {
    // It rewrites what gets installed on every machine. Choosing to expose it is
    // legitimate; doing it without noticing is not.
    let s = Server::boot(
        scratch("exposed"),
        &[("RESCRIPTUM_ADMIN_ADDR", "0.0.0.0:0")],
    );
    std::thread::sleep(Duration::from_millis(200));
    let log = s.startup_log();
    assert!(log.contains("not bound to loopback"), "{log}");
}
