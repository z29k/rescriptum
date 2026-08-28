//! End-to-end tests: start the real binary on an ephemeral port and talk HTTP to it.
//!
//! These exist because the unit tests all stop at the module boundary. The failure
//! this project actually has to avoid — an unattended install that hangs at 3am —
//! lives in the wiring, not in the pure functions.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A running server plus the temp directory it serves, both cleaned up on drop.
struct Server {
    child: Child,
    addr: String,
    dir: PathBuf,
    /// Everything the server has said on stderr, so a test can assert on a startup
    /// warning rather than only on what comes back over the socket.
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(files: &[(&str, &str)]) -> Server {
        Server::start_with(files, "5")
    }

    fn start_with(files: &[(&str, &str)], timeout_secs: &str) -> Server {
        Server::start_env(files, timeout_secs, &[])
    }

    fn start_env(files: &[(&str, &str)], timeout_secs: &str, env: &[(&str, &str)]) -> Server {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rescriptum-it-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create answers dir");
        for (name, contents) in files {
            let path = dir.join(name);
            // Names may be nested, e.g. "groups/rack-a.toml".
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create answer subdirectory");
            }
            fs::write(&path, contents).expect("write answer file");
        }

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
        cmd.env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
            .env("RESCRIPTUM_ANSWERS_DIR", &dir)
            .env("RESCRIPTUM_TIMEOUT_SECS", timeout_secs)
            .stderr(Stdio::piped())
            .stdout(Stdio::null());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().expect("spawn server");

        // The server prints the address it actually bound; parse it rather than
        // guessing a free port and racing for it. Scan rather than take the first line:
        // an env file, a missing answers directory or a load-time problem all announce
        // themselves before the banner.
        let stderr = child.stderr.take().expect("piped stderr");
        let mut lines = BufReader::new(stderr).lines();
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut addr = None;
        for _ in 0..8 {
            let Some(Ok(line)) = lines.next() else { break };
            let announced = line.split("listening on ").nth(1).map(|rest| {
                rest.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string()
            });
            log.lock().unwrap().push(line);
            if let Some(bound) = announced {
                addr = Some(bound);
                break;
            }
        }
        let addr =
            addr.unwrap_or_else(|| panic!("no address announced; saw {:#?}", log.lock().unwrap()));

        // Keep draining stderr so the server never blocks on a full pipe — and keep what
        // it said, because some of it is the thing under test.
        let collected = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in lines.map_while(Result::ok) {
                collected.lock().unwrap().push(line);
            }
        });

        Server {
            child,
            addr,
            dir,
            log,
        }
    }

    /// What the server has printed to stderr so far, joined.
    fn startup_log(&self) -> String {
        self.log.lock().unwrap().join("\n")
    }

    fn dir(&self) -> &Path {
        &self.dir
    }

    /// Send raw bytes, read the whole response back.
    fn raw(&self, request: &[u8]) -> String {
        let mut sock = TcpStream::connect(&self.addr).expect("connect");
        sock.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        sock.write_all(request).expect("write request");
        sock.flush().unwrap();
        let mut out = String::new();
        sock.read_to_string(&mut out).expect("read response");
        out
    }

    /// A POST to a specific path, which is what the install-finished webhook needs: the
    /// answer endpoint takes any path, so only a test that names one can tell a reserved
    /// route from an ordinary answer request.
    fn post_to(&self, path: &str, body: &str) -> String {
        self.raw(
            format!(
                "POST {path} HTTP/1.1\r\nHost: nas\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
    }

    fn post(&self, body: &str) -> String {
        self.raw(
            format!(
                "POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Close to what the installer actually POSTs: several NICs, one of which is ours.
fn installer_body(mac: &str) -> String {
    format!(
        r#"{{"product":"PowerEdge R620","serial":"7ABC123",
  "network_interfaces":[
    {{"name":"eno1","mac":"{mac}","index":0,"link":true}},
    {{"name":"eno2","mac":"aa:bb:cc:00:11:22","index":1,"link":false}}],
  "disks":[{{"path":"/dev/sda","size":500107862016,"model":"INTEL SSDSC2BB48"}}],
  "dmi":{{"system":{{"manufacturer":"Dell Inc.","product":"PowerEdge R620"}}}}}}"#
    )
}

/// Header names are case-insensitive, and hyper emits them lowercased. Compare on a
/// lowercased copy so the assertions test the contract, not the casing.
fn has_header(response: &str, needle: &str) -> bool {
    response
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn status_line(response: &str) -> &str {
    response.lines().next().unwrap_or("")
}

fn body_of(response: &str) -> &str {
    response.split("\r\n\r\n").nth(1).unwrap_or("")
}

#[test]
fn serves_the_matching_answer_file() {
    let s = Server::start(&[
        ("98-fa-9b-50-d8-10.toml", "[global]\nkeyboard = \"fr\"\n"),
        ("default.toml", "[global]\nkeyboard = \"us\"\n"),
    ]);
    let r = s.post(&installer_body("98:fa:9b:50:d8:10"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200 OK"), "{r}");
    assert!(has_header(&r, "content-type: text/plain"), "{r}");
    assert!(has_header(&r, "connection: close"), "{r}");
    assert!(body_of(&r).contains("\"fr\""), "{r}");
}

#[test]
fn falls_back_to_default_for_an_unknown_machine() {
    let s = Server::start(&[
        ("98-fa-9b-50-d8-10.toml", "marker = \"specific\"\n"),
        ("default.toml", "marker = \"fallback\"\n"),
    ]);
    let r = s.post(&installer_body("de:ad:be:ef:00:01"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200 OK"), "{r}");
    assert!(body_of(&r).contains("fallback"), "{r}");
}

#[test]
fn answers_404_when_nothing_applies() {
    let s = Server::start(&[("98-fa-9b-50-d8-10.toml", "marker = \"specific\"\n")]);
    let r = s.post(&installer_body("de:ad:be:ef:00:01"));
    assert!(status_line(&r).starts_with("HTTP/1.1 404"), "{r}");
}

#[test]
fn a_file_dropped_in_later_is_picked_up_without_a_restart() {
    let s = Server::start(&[]);
    let body = installer_body("98:fa:9b:50:d8:10");
    assert!(status_line(&s.post(&body)).starts_with("HTTP/1.1 404"));

    fs::write(
        s.dir().join("98fa9b50d810.toml"),
        "marker = \"added-at-runtime\"\n",
    )
    .unwrap();
    let r = s.post(&body);
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
    assert!(body_of(&r).contains("added-at-runtime"), "{r}");
}

#[test]
fn health_check_responds() {
    let s = Server::start(&[]);
    let r = s.raw(b"GET /health HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(status_line(&r).starts_with("HTTP/1.1 200 OK"), "{r}");
    assert_eq!(body_of(&r), "OK\n");
}

#[test]
fn methods_that_cannot_be_an_installer_are_rejected() {
    let s = Server::start(&[("default.toml", "marker = \"x\"\n")]);
    for verb in ["PUT", "DELETE", "PATCH"] {
        let r = s.raw(format!("{verb} /answer HTTP/1.1\r\nHost: nas\r\n\r\n").as_bytes());
        assert!(status_line(&r).starts_with("HTTP/1.1 405"), "{verb}: {r}");
    }
}

#[test]
fn a_get_is_answered_too() {
    // Proxmox posts its hardware inventory; a Debian preseed or a RHEL kickstart is
    // fetched, so GET has to work as well.
    let s = Server::start(&[(
        "default.preseed",
        "d-i debian-installer/locale string en_US\n",
    )]);
    let r = s.raw(b"GET /preseed HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
    assert!(body_of(&r).contains("locale"), "{r}");
}

#[test]
fn a_machine_is_identified_by_query_parameters_on_a_get() {
    // This is how iPXE hands over an identity when there is no body to inspect.
    let s = Server::start(&[
        ("98fa9b50d810.ks", "# kickstart\nlang fr_FR\n"),
        ("default.ks", "# kickstart\nlang en_US\n"),
    ]);

    let r = s.raw(b"GET /ks?mac=98:fa:9b:50:d8:10 HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
    assert!(
        body_of(&r).contains("fr_FR"),
        "the named machine's answer: {r}"
    );

    let r = s.raw(b"GET /ks?mac=00:00:00:00:00:01 HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(body_of(&r).contains("en_US"), "the default: {r}");
}

#[test]
fn a_selector_matches_on_something_that_is_not_the_filename() {
    // The whole point of selectors: name the file readably, match on the hardware.
    let s = Server::start(&[(
        "groups/dell-r620.toml",
        "[match]\nproduct = \"PowerEdge R620\"\n[global]\nkeyboard = \"fr\"\n",
    )]);

    let body = installer_body("11:22:33:44:55:66");
    let r = s.raw(
        format!(
            "POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
    assert!(body_of(&r).contains("keyboard"), "{r}");
}

#[test]
fn the_content_type_follows_the_format() {
    for (file, body, expected) in [
        ("default.toml", "marker = \"x\"\n", "text/plain"),
        ("default.yaml", "marker: x\n", "text/yaml"),
        ("default.json", "{\"marker\":\"x\"}", "application/json"),
        (
            "default.xml",
            "<r><marker>x</marker></r>",
            "application/xml",
        ),
        // The text family: opaque to us, and served as plain text like the TOML is.
        (
            "default.ks",
            "# kickstart\nlang fr_FR.UTF-8\n",
            "text/plain",
        ),
        ("default.preseed", "d-i marker string x\n", "text/plain"),
        (
            "default.ipxe",
            "#!ipxe\nchain http://example/boot\n",
            "text/plain",
        ),
    ] {
        let s = Server::start(&[(file, body)]);
        let r = s.post(&installer_body("11:22:33:44:55:66"));
        assert!(has_header(&r, expected), "{file}: expected {expected}\n{r}");
    }
}

#[test]
fn any_path_is_accepted_for_post() {
    // The URL is fixed inside the ISO, so the server must not care what it is.
    let s = Server::start(&[("default.toml", "marker = \"answer\"\n")]);
    for path in ["/", "/answer", "/pve/answer.toml", "/deep/nested/path?q=1"] {
        let body = installer_body("de:ad:be:ef:00:01");
        let r = s.raw(
            format!(
                "POST {path} HTTP/1.1\r\nHost: nas\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
        assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{path}: {r}");
    }
}

#[test]
fn a_truncated_request_does_not_take_the_server_down() {
    let s = Server::start(&[("default.toml", "marker = \"answer\"\n")]);

    // Promise 500 bytes, send 5, then hang up.
    {
        let mut sock = TcpStream::connect(&s.addr).unwrap();
        sock.write_all(b"POST /answer HTTP/1.1\r\nContent-Length: 500\r\n\r\nshort")
            .unwrap();
        sock.flush().unwrap();
    } // dropped: the server sees EOF mid-body

    // Garbage that is not HTTP at all.
    let _ = s.raw(b"\x00\x01\x02\x03 hello?\r\n\r\n");

    // No Content-Length at all.
    let _ = s.raw(b"POST /answer HTTP/1.1\r\nHost: nas\r\n\r\n");

    // The server must still be serving.
    let r = s.post(&installer_body("de:ad:be:ef:00:01"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
}

#[test]
fn an_absurd_content_length_is_refused_and_the_server_survives() {
    let s = Server::start(&[("default.toml", "marker = \"answer\"\n")]);
    let r = s.raw(b"POST /answer HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n");
    assert!(status_line(&r).starts_with("HTTP/1.1 413"), "{r}");

    let r = s.post(&installer_body("de:ad:be:ef:00:01"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
}

#[test]
fn a_burst_of_concurrent_clients_all_get_answered() {
    // The reason for the worker pool: a rack of machines PXE-booting at once.
    let s = Server::start(&[
        ("98-fa-9b-50-d8-10.toml", "marker = \"specific\"\n"),
        ("default.toml", "marker = \"fallback\"\n"),
    ]);
    let addr = s.addr.clone();

    let mut handles = Vec::new();
    for i in 0..64 {
        let addr = addr.clone();
        handles.push(std::thread::spawn(move || {
            // Half ask for a known machine, half fall back.
            let mac = if i % 2 == 0 {
                "98:fa:9b:50:d8:10".to_string()
            } else {
                format!("de:ad:be:ef:{:02x}:{:02x}", i / 256, i % 256)
            };
            let body = installer_body(&mac);
            let mut sock = TcpStream::connect(&addr).expect("connect");
            sock.set_read_timeout(Some(Duration::from_secs(15)))
                .unwrap();
            sock.write_all(
                format!(
                    "POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .expect("write");
            sock.flush().unwrap();
            let mut out = String::new();
            sock.read_to_string(&mut out).expect("read");
            let expected = if i % 2 == 0 { "specific" } else { "fallback" };
            assert!(status_line(&out).starts_with("HTTP/1.1 200"), "{out}");
            assert!(body_of(&out).contains(expected), "{out}");
        }));
    }
    for (i, h) in handles.into_iter().enumerate() {
        h.join().unwrap_or_else(|_| panic!("client {i} failed"));
    }
}

#[test]
fn a_chunked_body_is_accepted() {
    // The sync implementation answered 501 here. hyper decodes chunked properly, so
    // this is a capability we gained by moving to it — worth pinning down.
    let s = Server::start(&[("98-fa-9b-50-d8-10.toml", "marker = \"chunked-worked\"\n")]);
    let body = installer_body("98:fa:9b:50:d8:10");
    let (first, rest) = body.split_at(40);
    let raw = format!(
        "POST /answer HTTP/1.1\r\nHost: nas\r\nTransfer-Encoding: chunked\r\n\r\n\
         {:x}\r\n{first}\r\n{:x}\r\n{rest}\r\n0\r\n\r\n",
        first.len(),
        rest.len()
    );
    let r = s.raw(raw.as_bytes());
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
    assert!(body_of(&r).contains("chunked-worked"), "{r}");
}

#[test]
fn conflicting_content_lengths_are_rejected() {
    // Request smuggling: two lengths that disagree must not be resolved by guessing.
    let s = Server::start(&[("default.toml", "marker = \"answer\"\n")]);
    let r = s.raw(b"POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: 5\r\nContent-Length: 9\r\n\r\nhello");
    assert!(status_line(&r).starts_with("HTTP/1.1 400"), "{r}");

    let r = s.post(&installer_body("de:ad:be:ef:00:01"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
}

#[test]
fn a_client_that_stalls_mid_headers_is_dropped_and_others_keep_working() {
    // The reason for going async: a stalled client must cost a task, not a worker.
    let s = Server::start_with(&[("default.toml", "marker = \"answer\"\n")], "1");

    // Open several connections that send a partial header block and then just sit.
    let stalled: Vec<TcpStream> = (0..8)
        .map(|_| {
            let mut sock = TcpStream::connect(&s.addr).expect("connect");
            sock.write_all(b"POST /answer HTTP/1.1\r\nHost: nas\r\nX-Partial: ")
                .expect("write partial");
            sock.flush().unwrap();
            sock
        })
        .collect();

    // A normal client must be served immediately, while those are still hanging.
    let r = s.post(&installer_body("de:ad:be:ef:00:01"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");

    // After the header timeout the stalled ones are dropped, and the server is fine.
    std::thread::sleep(Duration::from_millis(2500));
    drop(stalled);
    let r = s.post(&installer_body("de:ad:be:ef:00:01"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
}

#[test]
fn a_body_that_never_arrives_does_not_park_the_connection_forever() {
    // `header_read_timeout` stops at the headers; this is what the whole-connection
    // deadline is for.
    let s = Server::start_with(&[("default.toml", "marker = \"answer\"\n")], "1");
    let mut sock = TcpStream::connect(&s.addr).expect("connect");
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    // Promise a body, send none, keep the socket open.
    sock.write_all(b"POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: 4096\r\n\r\n")
        .unwrap();
    sock.flush().unwrap();

    // The server must give up rather than hold this forever.
    let mut out = String::new();
    sock.read_to_string(&mut out)
        .expect("the server should close the connection");

    let r = s.post(&installer_body("de:ad:be:ef:00:01"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
}

#[test]
fn a_group_answers_for_every_one_of_its_members() {
    let s = Server::start(&[(
        "groups/rack-a.toml",
        "members = [\"98:fa:9b:50:d8:10\", \"aa:bb:cc:dd:ee:ff\"]\n\
         [global]\ncountry = \"fr\"\n[disk-setup]\nfilesystem = \"zfs\"\n",
    )]);

    for mac in ["98:fa:9b:50:d8:10", "aa:bb:cc:dd:ee:ff"] {
        let r = s.post(&installer_body(mac));
        assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{mac}: {r}");
        assert!(body_of(&r).contains("country"), "{mac}: {r}");
        // Our bookkeeping keys must never reach the installer.
        assert!(!body_of(&r).contains("members"), "{mac}: {r}");
    }

    // A machine outside the group gets nothing.
    let r = s.post(&installer_body("11:22:33:44:55:66"));
    assert!(status_line(&r).starts_with("HTTP/1.1 404"), "{r}");
}

#[test]
fn a_machine_file_overrides_its_group_over_http() {
    let s = Server::start(&[
        (
            "groups/rack-a.toml",
            "members = [\"98:fa:9b:50:d8:10\"]\n\
             [global]\ncountry = \"fr\"\nkeyboard = \"fr\"\n",
        ),
        ("98-fa-9b-50-d8-10.toml", "[global]\nkeyboard = \"us\"\n"),
    ]);

    let r = s.post(&installer_body("98:fa:9b:50:d8:10"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
    let body = body_of(&r);
    assert!(body.contains("\"us\""), "the machine must win: {body}");
    assert!(!body.contains("keyboard = \"fr\""), "{body}");
    assert!(body.contains("country"), "the group must survive: {body}");
}

#[test]
fn a_group_edited_in_place_is_picked_up_without_a_restart() {
    // Editing a group file's contents changes no directory mtime, so this exercises
    // the reload backstop rather than mtime invalidation.
    let s = Server::start(&[(
        "groups/rack-a.toml",
        "members = [\"98:fa:9b:50:d8:10\"]\n[global]\nkeyboard = \"fr\"\n",
    )]);
    let r = s.post(&installer_body("98:fa:9b:50:d8:10"));
    assert!(body_of(&r).contains("\"fr\""), "{r}");

    fs::write(
        s.dir().join("groups/rack-a.toml"),
        "members = [\"98:fa:9b:50:d8:10\"]\n[global]\nkeyboard = \"us\"\n",
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(1200));

    let r = s.post(&installer_body("98:fa:9b:50:d8:10"));
    assert!(body_of(&r).contains("\"us\""), "edit not picked up: {r}");
}

#[test]
fn a_broken_answer_file_is_a_500_not_a_wrong_install() {
    let s = Server::start(&[("98-fa-9b-50-d8-10.toml", "this is = = not toml\n")]);
    let r = s.post(&installer_body("98:fa:9b:50:d8:10"));
    assert!(status_line(&r).starts_with("HTTP/1.1 500"), "{r}");

    // And the server keeps serving everyone else.
    fs::write(s.dir().join("default.toml"), "marker = \"ok\"\n").unwrap();
    let r = s.post(&installer_body("11:22:33:44:55:66"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
}

#[test]
fn the_two_nocloud_files_are_answered_differently() {
    // cloud-init fetches both from one URL and skips the datasource if either is
    // missing. Answering them identically is how an Ubuntu install silently never
    // starts, so this is worth pinning down.
    let s = Server::start(&[
        (
            "groups/ubuntu.yaml",
            "match:\n  file: user-data\nversion: 1\nlocale: fr_FR.UTF-8\n",
        ),
        (
            "groups/ubuntu-meta.yaml",
            "match:\n  file: meta-data\ninstance-id: iid-local01\n",
        ),
    ]);

    let user = s.raw(b"GET /autoinstall/user-data HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(status_line(&user).starts_with("HTTP/1.1 200"), "{user}");
    assert!(body_of(&user).contains("locale"), "{user}");
    assert!(!body_of(&user).contains("instance-id"), "{user}");

    let meta = s.raw(b"GET /autoinstall/meta-data HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(status_line(&meta).starts_with("HTTP/1.1 200"), "{meta}");
    assert!(body_of(&meta).contains("instance-id"), "{meta}");
    assert!(!body_of(&meta).contains("locale"), "{meta}");
}

#[test]
fn an_identity_expanded_into_the_path_is_matched() {
    // NoCloud can expand `__dmi.chassis-serial-number__` into the seed URL, which puts
    // the machine's identity in the path rather than the query string.
    let s = Server::start(&[
        ("7ABC123.yaml", "marker: the-right-machine\n"),
        ("default.yaml", "marker: fallback\n"),
    ]);

    let r = s.raw(b"GET /seed/7ABC123/user-data HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(body_of(&r).contains("the-right-machine"), "{r}");
}

#[test]
fn an_endpoint_only_ever_serves_the_format_its_installer_expects() {
    // The mixed-fleet failure this closes: a group matching by MAC answering a URL
    // meant for another installer, and a kickstart client receiving TOML.
    let s = Server::start(&[
        (
            "groups/pve.toml",
            "members = [\"98:fa:9b:50:d8:10\"]\n[global]\nkeyboard = \"fr\"\n",
        ),
        (
            "groups/rhel.ks",
            "# answer: member 98:fa:9b:50:d8:10\nlang fr_FR.UTF-8\n",
        ),
    ]);

    let r = s.raw(b"GET /proxmox/answer?mac=98:fa:9b:50:d8:10 HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(body_of(&r).contains("keyboard"), "{r}");
    assert!(has_header(&r, "text/plain"), "{r}");

    let r = s.raw(b"GET /rhel/ks?mac=98:fa:9b:50:d8:10 HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(body_of(&r).contains("lang fr_FR"), "{r}");
    assert!(
        !body_of(&r).contains("keyboard = "),
        "TOML leaked into kickstart: {r}"
    );

    // A format nobody serves for this machine is a 404, not somebody else's answer.
    let r = s.raw(b"GET /ubuntu/user-data?mac=98:fa:9b:50:d8:10 HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(status_line(&r).starts_with("HTTP/1.1 404"), "{r}");
}

#[test]
fn one_machine_answered_as_two_operating_systems_over_http() {
    let s = Server::start(&[
        ("98fa9b50d810.toml", "marker = \"as-proxmox\"\n"),
        ("98fa9b50d810.preseed", "d-i marker string as-debian\n"),
    ]);

    let p = s.raw(b"GET /proxmox/answer?mac=98:fa:9b:50:d8:10 HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(body_of(&p).contains("as-proxmox"), "{p}");

    let d = s.raw(b"GET /debian/preseed?mac=98:fa:9b:50:d8:10 HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(body_of(&d).contains("as-debian"), "{d}");
}

#[test]
fn one_group_serves_a_whole_rack_through_templating() {
    // Without this, a per-machine hostname means a file per machine.
    let s = Server::start(&[(
        "groups/rack.toml",
        "members = [\"98:fa:9b:50:d8:10\", \"98:fa:9b:50:d8:11\"]\n\
         [global]\nfqdn = \"node-{{ serial }}.example.com\"\n",
    )]);

    for (mac, serial) in [
        ("98:fa:9b:50:d8:10", "7ABC123"),
        ("98:fa:9b:50:d8:11", "9XYZ789"),
    ] {
        let r = s.raw(
            format!("GET /proxmox/answer?mac={mac}&serial={serial} HTTP/1.1\r\nHost: nas\r\n\r\n")
                .as_bytes(),
        );
        assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
        assert!(
            body_of(&r).contains(&format!("node-{serial}.example.com")),
            "{r}"
        );
    }
}

#[test]
fn a_template_the_request_cannot_fill_is_refused_loudly() {
    // Serving `node-.example.com` would install the machine with a broken hostname.
    let s = Server::start(&[(
        "groups/rack.toml",
        "members = [\"98:fa:9b:50:d8:10\"]\n[global]\nfqdn = \"node-{{ serial }}\"\n",
    )]);
    let r = s.raw(b"GET /proxmox/answer?mac=98:fa:9b:50:d8:10 HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(status_line(&r).starts_with("HTTP/1.1 500"), "{r}");
}

#[test]
fn the_matched_machine_and_group_are_available_to_a_template() {
    let s = Server::start(&[
        (
            "groups/rack.toml",
            "members = [\"98:fa:9b:50:d8:10\"]\n[global]\ntag = \"{{ group }}\"\n",
        ),
        (
            "98fa9b50d810.toml",
            "[global]\nfqdn = \"{{ machine }}.lan\"\n",
        ),
    ]);
    let r = s.raw(b"GET /proxmox/answer?mac=98:fa:9b:50:d8:10 HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(body_of(&r).contains("98fa9b50d810.lan"), "{r}");
    assert!(body_of(&r).contains("\"rack\""), "{r}");
}

#[test]
fn over_the_connection_cap_the_server_sheds_promptly_instead_of_queueing() {
    // Async connections are cheap, not free: unbounded accept turns a provisioning
    // burst into an out-of-memory. Over the cap the client is told to go away, which is
    // strictly better for it than being parked in a queue that will not drain.
    let s = Server::start_env(
        &[("default.toml", "marker = \"served\"\n")],
        "5",
        &[("RESCRIPTUM_MAX_CONNECTIONS", "1")],
    );

    // One connection that opens and then says nothing: it holds the only permit.
    let mut holder = TcpStream::connect(&s.addr).expect("connect holder");
    holder
        .write_all(b"POST /answer HTTP/1.1\r\nHost: nas\r\n")
        .expect("partial headers");
    holder.flush().unwrap();
    std::thread::sleep(Duration::from_millis(200));

    let shed = s.raw(b"POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: 2\r\n\r\n{}");
    assert!(
        status_line(&shed).starts_with("HTTP/1.1 503"),
        "expected a shed connection\n{shed}"
    );

    // …and the permit comes back when that connection ends.
    drop(holder);
    let mut recovered = None;
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        let r = s.post(&installer_body("de:ad:be:ef:00:01"));
        if status_line(&r).starts_with("HTTP/1.1 200") {
            recovered = Some(r);
            break;
        }
    }
    assert!(recovered.is_some(), "the permit was never released");
}

#[test]
fn a_body_that_outgrows_the_cap_while_streaming_is_refused_too() {
    // The declared-length case is refused from the header. This is the other route in:
    // chunked, so there is no length to inspect and the limit has to trip mid-read.
    let s = Server::start(&[("default.toml", "marker = \"served\"\n")]);

    let mut sock = TcpStream::connect(&s.addr).expect("connect");
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    sock.write_all(b"POST /answer HTTP/1.1\r\nHost: nas\r\nTransfer-Encoding: chunked\r\n\r\n")
        .expect("headers");

    // 2 MB in 64 KB chunks. The server answers and closes partway through, so writes
    // are expected to fail: that is the point, not a problem.
    let chunk = vec![b'a'; 64 * 1024];
    let header = format!("{:x}\r\n", chunk.len());
    for _ in 0..32 {
        if sock.write_all(header.as_bytes()).is_err()
            || sock.write_all(&chunk).is_err()
            || sock.write_all(b"\r\n").is_err()
        {
            break;
        }
    }
    let _ = sock.flush();

    let mut out = String::new();
    let _ = sock.read_to_string(&mut out);
    assert!(status_line(&out).starts_with("HTTP/1.1 413"), "{out}");

    // The abuse is only interesting if the server survives it.
    let r = s.post(&installer_body("de:ad:be:ef:00:01"));
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
}

#[test]
fn a_body_that_is_not_utf8_is_matched_on_rather_than_rejected() {
    // A request body is arbitrary bytes and need not be valid UTF-8 — which is why
    // normalization works on bytes. Over a socket that has to hold too.
    let s = Server::start(&[
        ("98fa9b50d810.toml", "marker = \"matched\"\n"),
        ("default.toml", "marker = \"fallback\"\n"),
    ]);

    let mut body: Vec<u8> = vec![0xff, 0xfe, 0x00, 0x80];
    body.extend_from_slice(br#"{"mac":"98:fa:9b:50:d8:10"}"#);
    body.extend_from_slice(&[0x80, 0xff]);

    let mut request = format!(
        "POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(&body);

    let r = s.raw(&request);
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
    assert!(body_of(&r).contains("matched"), "{r}");

    // Pure noise resolves to the fallback rather than to a 500.
    let noise: Vec<u8> = (0u8..=255).collect();
    let mut request = format!(
        "POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: {}\r\n\r\n",
        noise.len()
    )
    .into_bytes();
    request.extend_from_slice(&noise);
    let r = s.raw(&request);
    assert!(status_line(&r).starts_with("HTTP/1.1 200"), "{r}");
    assert!(body_of(&r).contains("fallback"), "{r}");
}

#[test]
fn a_running_server_takes_its_configuration_from_the_env_file() {
    // The DSM case: no systemd, so there is nowhere to put RESCRIPTUM_ANSWER_TOKEN except
    // a file — and sourcing it from the Task Scheduler fails silently when it fails at
    // all. This proves the file reaches the running server, not just the CLI.
    static N: AtomicUsize = AtomicUsize::new(0);
    let env_path = std::env::temp_dir().join(format!(
        "rescriptum-envfile-{}-{}.env",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(
        &env_path,
        "# guarded by a token nothing else knows about\nexport RESCRIPTUM_ANSWER_TOKEN=from-the-env-file-0123\n",
    )
    .expect("write env file");

    let s = Server::start_env(
        &[("default.toml", "marker = \"served\"\n")],
        "5",
        &[("RESCRIPTUM_ENV_FILE", env_path.to_str().unwrap())],
    );

    // The token came from the file, so the endpoint is now guarded.
    let body = installer_body("98:fa:9b:50:d8:10");
    let unauthenticated = s.post(&body);
    assert!(
        status_line(&unauthenticated).starts_with("HTTP/1.1 401"),
        "the file's token must be in force\n{unauthenticated}"
    );

    let authenticated = s.raw(
        format!(
            "POST /answer HTTP/1.1\r\nHost: nas\r\nAuthorization: Bearer from-the-env-file-0123\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
    assert!(
        status_line(&authenticated).starts_with("HTTP/1.1 200"),
        "{authenticated}"
    );
    assert!(
        body_of(&authenticated).contains("served"),
        "{authenticated}"
    );

    let _ = fs::remove_file(&env_path);
}

#[test]
fn a_server_told_to_read_a_file_that_is_not_there_does_not_come_up() {
    // Starting on defaults instead — wrong answers directory, no admin token, no word in
    // the log — is the failure the env file exists to remove. It must be fatal.
    let out = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
        .env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
        .env("RESCRIPTUM_ENV_FILE", "/nonexistent/rescriptum.env")
        .output()
        .expect("run server");
    assert!(!out.status.success(), "it must refuse to start");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("RESCRIPTUM_ENV_FILE"), "{stderr}");
    assert!(stderr.contains("cannot be read"), "{stderr}");
    assert!(
        !stderr.contains("listening on"),
        "it must not bind before giving up\n{stderr}"
    );
}

#[test]
fn an_answers_directory_that_cannot_be_read_says_so_at_startup() {
    // The failure a packaged install meets first: DSM runs a package as a non-root user,
    // so a directory created by hand as root is readable to nobody who matters. Without
    // this the symptom is a 404 storm and a completely silent log.
    let s = Server::start(&[("default.toml", "marker = \"served\"\n")]);
    std::fs::set_permissions(s.dir(), std::fs::Permissions::from_mode(0o000))
        .expect("chmod the answers directory");

    // Running as root defeats the whole scenario — nothing is unreadable. Say so and
    // stop rather than asserting something that cannot happen.
    let unreadable = std::fs::read_dir(s.dir()).is_err();
    let restore = || {
        let _ = std::fs::set_permissions(s.dir(), std::fs::Permissions::from_mode(0o755));
    };
    if !unreadable {
        restore();
        eprintln!("skipped: this process can read a 0000 directory, so it is root");
        return;
    }

    let warned = Server::start_env(
        &[],
        "5",
        &[("RESCRIPTUM_ANSWERS_DIR", s.dir().to_str().unwrap())],
    );
    std::thread::sleep(Duration::from_millis(300));
    let log = warned.startup_log();
    restore();

    assert!(log.contains("cannot be read"), "{log}");
    assert!(
        log.contains("404"),
        "the warning has to say what the symptom will be\n{log}"
    );
}

#[test]
fn an_answers_directory_that_is_merely_missing_says_something_different() {
    let missing = std::env::temp_dir().join(format!(
        "rescriptum-absent-{}-{}",
        std::process::id(),
        MISSING.fetch_add(1, Ordering::Relaxed)
    ));
    let s = Server::start_env(
        &[],
        "5",
        &[("RESCRIPTUM_ANSWERS_DIR", missing.to_str().unwrap())],
    );
    std::thread::sleep(Duration::from_millis(300));

    let log = s.startup_log();
    assert!(log.contains("does not exist yet"), "{log}");

    // And it is a warning, not a refusal: the directory may appear later, and it is
    // re-read as it changes.
    let r = s.post(&installer_body("98:fa:9b:50:d8:10"));
    assert!(status_line(&r).starts_with("HTTP/1.1 404"), "{r}");
}

#[test]
fn a_path_that_exists_but_is_not_a_directory_says_which() {
    // "does not exist yet" would send you looking for something that is right there.
    let s = Server::start(&[]);
    let path = s.dir().join("not-a-directory");
    fs::write(&path, "this is a file\n").unwrap();

    let bad = Server::start_env(
        &[],
        "5",
        &[("RESCRIPTUM_ANSWERS_DIR", path.to_str().unwrap())],
    );
    std::thread::sleep(Duration::from_millis(300));

    let log = bad.startup_log();
    assert!(log.contains("is not a directory"), "{log}");
    assert!(!log.contains("does not exist"), "{log}");
}

#[test]
fn a_problem_in_the_answer_set_is_reported_once_at_startup() {
    // It used to be printed twice, once by the listing and once by the caller. A log that
    // is the whole diagnostic story should not say everything twice.
    let s = Server::start(&[(
        "98fa9b50d810.toml",
        "extends = \"nope\"\n\n[global]\nx = 1\n",
    )]);
    std::thread::sleep(Duration::from_millis(300));

    let log = s.startup_log();
    assert_eq!(
        log.matches("extends unknown group").count(),
        1,
        "reported {} times\n{log}",
        log.matches("extends unknown group").count()
    );
    assert!(log.contains("warning: "), "{log}");
}

static MISSING: AtomicUsize = AtomicUsize::new(0);

#[test]
fn the_log_can_be_quietened_to_the_requests_that_failed() {
    // One line per answer is the only thing here with any volume. At thirteen thousand
    // requests a second you want the failures and nothing else.
    let s = Server::start_env(
        &[("default.toml", "marker = \"served\"\n")],
        "5",
        &[("RESCRIPTUM_LOG", "problems")],
    );

    assert!(status_line(&s.post(&installer_body("98:fa:9b:50:d8:10"))).starts_with("HTTP/1.1 200"));
    let rejected = s.raw(b"PATCH /answer HTTP/1.1\r\nHost: nas\r\n\r\n");
    assert!(
        status_line(&rejected).starts_with("HTTP/1.1 405"),
        "{rejected}"
    );
    std::thread::sleep(Duration::from_millis(200));

    let log = s.startup_log();
    assert!(
        log.contains("log=problems"),
        "the banner must say why\n{log}"
    );
    assert!(log.contains("405"), "a failure must survive\n{log}");
    assert!(!log.contains(" 200 "), "a success must not\n{log}");
}

/// Spawn a server without reading its banner off stderr, for the cases where stderr is
/// deliberately not where the log goes.
fn spawn_quiet(dir: &Path, env: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
    cmd.env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
        .env("RESCRIPTUM_ANSWERS_DIR", dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.spawn().expect("spawn server")
}

#[test]
fn the_log_can_be_turned_off_entirely() {
    let s = Server::start(&[("default.toml", "marker = \"served\"\n")]);
    let mut child = spawn_quiet(s.dir(), &[("RESCRIPTUM_LOG", "off")]);
    std::thread::sleep(Duration::from_millis(400));
    let _ = child.kill();
    let _ = child.wait();

    let mut said = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut said);
    }
    assert!(said.is_empty(), "off has to mean off\n{said}");
}

#[test]
fn the_log_can_be_sent_to_a_file_that_does_not_exist_yet() {
    let s = Server::start(&[("default.toml", "marker = \"served\"\n")]);
    // Into a subdirectory, because a package installs its log somewhere of its own.
    let path = s.dir().join("var").join("rescriptum.log");
    let mut child = spawn_quiet(s.dir(), &[("RESCRIPTUM_LOG_FILE", path.to_str().unwrap())]);
    std::thread::sleep(Duration::from_millis(400));
    let _ = child.kill();
    let _ = child.wait();

    let mut said = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut said);
    }
    assert!(said.is_empty(), "nothing should be left on stderr\n{said}");

    let written = fs::read_to_string(&path).expect("the log file");
    assert!(written.contains("listening on"), "{written}");
}

#[test]
fn a_log_file_that_cannot_be_opened_stops_the_server() {
    // Carrying on and writing somewhere else would be a silent surprise, discovered later.
    let out = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
        .env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
        .env("RESCRIPTUM_LOG_FILE", "/nonexistent-root/rescriptum.log")
        .output()
        .expect("run server");
    assert!(!out.status.success(), "it must refuse to start");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("RESCRIPTUM_LOG_FILE"), "{stderr}");
    assert!(
        !stderr.contains("listening on"),
        "it must not bind before giving up\n{stderr}"
    );
}

/// Run a server briefly and collect what it said on each stream.
fn quiet_run(dir: &Path, env: &[(&str, &str)]) -> (String, String) {
    let mut child = spawn_quiet(dir, env);
    std::thread::sleep(Duration::from_millis(400));
    let _ = child.kill();
    let _ = child.wait();

    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut out);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut err);
    }
    (out, err)
}

#[test]
fn a_log_file_is_appended_to_rather_than_truncated() {
    // A restart that wiped the log would destroy the diagnostic exactly when someone is
    // restarting the server to fix whatever the log was about to explain.
    let s = Server::start(&[("default.toml", "marker = \"served\"\n")]);
    let path = s.dir().join("rescriptum.log");
    let env = [("RESCRIPTUM_LOG_FILE", path.to_str().unwrap())];

    quiet_run(s.dir(), &env);
    quiet_run(s.dir(), &env);

    let written = fs::read_to_string(&path).expect("the log file");
    assert_eq!(
        written.matches("listening on").count(),
        2,
        "both runs have to be in there\n{written}"
    );
}

#[test]
fn stdout_and_stderr_can_be_named_outright() {
    let s = Server::start(&[("default.toml", "marker = \"served\"\n")]);

    let (out, err) = quiet_run(s.dir(), &[("RESCRIPTUM_LOG_FILE", "stdout")]);
    assert!(out.contains("listening on"), "stdout: {out}");
    assert!(err.is_empty(), "stderr should be untouched: {err}");

    let (out, err) = quiet_run(s.dir(), &[("RESCRIPTUM_LOG_FILE", "stderr")]);
    assert!(err.contains("listening on"), "stderr: {err}");
    assert!(out.is_empty(), "stdout should be untouched: {out}");
}

#[test]
fn a_request_that_never_reached_a_status_counts_as_a_problem() {
    // `problems` filters on the status. A connection that timed out mid-body never got
    // one, and dropping it would hide exactly the client worth knowing about.
    let s = Server::start_env(
        &[("default.toml", "marker = \"answer\"\n")],
        "1",
        &[("RESCRIPTUM_LOG", "problems")],
    );

    let mut sock = TcpStream::connect(&s.addr).expect("connect");
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    // Promise a body, send none, keep the socket open.
    sock.write_all(b"POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: 4096\r\n\r\n")
        .unwrap();
    sock.flush().unwrap();
    let mut discard = String::new();
    let _ = sock.read_to_string(&mut discard);
    std::thread::sleep(Duration::from_millis(300));

    let log = s.startup_log();
    assert!(log.contains("connection timed out"), "{log}");
}

/// **A machine reporting that it installed, and its claim being dropped.**
///
/// The loop this closes: a machine claimed by an `.ipxe` answer installs, reboots, is
/// claimed again, and installs again — wiping its disk every time. Proxmox's
/// `[post-installation-webhook]` fires after a successful install and before that reboot,
/// with the interfaces in its body, so the machine is the one that knows.
///
/// Everything below is against the real binary over a real socket, because the parts that
/// can be wrong here are wiring: which guard runs first, whether the body is read before
/// answering, and whether the endpoint exists at all.
#[test]
fn a_machine_can_report_that_it_is_installed_and_stop_being_claimed() {
    let s = Server::start_env(
        &[
            ("98-fa-9b-50-d8-10.ipxe", "#!ipxe\nchain installer\n"),
            ("98-fa-9b-50-d8-10.toml", "[global]\nkeyboard = \"fr\"\n"),
        ],
        "5",
        &[
            ("RESCRIPTUM_INSTALLED_TOKEN", "nas:s3cr3t"),
            // Set deliberately: the webhook's credential is in the body, so the bearer
            // guard must not be what answers it. Without the route running first, every
            // webhook would 401 the moment somebody protected their answers.
            ("RESCRIPTUM_ANSWER_TOKEN", "nas:answer-token-long-enough"),
        ],
    );

    let body = r#"{"token":"nas:s3cr3t","fqdn":"node01.z29k.fr",
        "network_interfaces":[{"name":"eno1","mac":"98:fa:9b:50:d8:10"}]}"#;

    // A wrong token is refused, and refused *without* disarming anything.
    let bad = body.replace("s3cr3t", "s3cr3x");
    let response = s.post_to("/installed", &bad);
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");

    let response = s.post_to("/installed", body);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.contains("installed-98-fa-9b-50-d8-10"),
        "{response}"
    );

    // The claim is gone…
    assert!(!s.dir().join("98-fa-9b-50-d8-10.ipxe").exists());
    // …the document is not, so re-arming is a rename…
    assert!(s.dir().join("installed-98-fa-9b-50-d8-10.ipxe").exists());
    // …and the machine's own answer, which the installer reads, is untouched.
    assert!(s.dir().join("98-fa-9b-50-d8-10.toml").exists());

    // Twice is not an error: the webhook may be retried, and a machine installed from the
    // menu was never claimed at all.
    let response = s.post_to("/installed", body);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("nothing was claiming it"), "{response}");

    // And the server still answers, which is the assertion that matters in this file.
    // With its own credential — the bearer the *installer* presents, which is a different
    // one from the webhook's and is the whole reason these two guards are separate.
    let payload = r#"{"mac":"98:fa:9b:50:d8:10"}"#;
    let answer = s.raw(
        format!(
            "POST /proxmox/answer HTTP/1.1\r\nHost: nas\r\n\
             Authorization: Bearer nas:answer-token-long-enough\r\n\
             Content-Length: {}\r\n\r\n{payload}",
            payload.len()
        )
        .as_bytes(),
    );
    assert!(
        answer.contains("keyboard"),
        "the answer endpoint stopped working: {answer}"
    );
}

/// Without the token there is no endpoint — not an open one, **absent**. So `/installed`
/// is an ordinary answer request like any other path, which is what keeps "POST on any
/// path is an answer request" true for everybody who does not use this.
#[test]
fn without_a_token_installed_is_just_another_answer_path() {
    let s = Server::start(&[("default.toml", "[global]\nkeyboard = \"fr\"\n")]);
    let response = s.post_to("/installed", r#"{"token":"anything"}"#);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.contains("keyboard"),
        "it answered as an endpoint rather than serving the default: {response}"
    );
}

/// **Every other family reports back from a shell script, not from a webhook.** A
/// kickstart `%post`, a preseed `late_command`, an autoinstall `late-commands`, an
/// AutoYaST chroot script — all of them can run one `curl`, and none of them will compose
/// Proxmox's JSON body. So the endpoint takes the machine's identity from the query string
/// and its credential from an ordinary bearer header, which is what that one line can send.
#[test]
fn a_kickstart_or_a_preseed_can_report_installed_with_one_curl() {
    let s = Server::start_env(
        &[
            ("98-fa-9b-50-d8-10.ipxe", "#!ipxe\nchain installer\n"),
            ("98-fa-9b-50-d8-10.ks", "%post\n"),
        ],
        "5",
        &[("RESCRIPTUM_INSTALLED_TOKEN", "nas:s3cr3t")],
    );

    // Exactly what a `%post` writes:
    //   curl -X POST -H "Authorization: Bearer nas:s3cr3t" \
    //        "http://server:8000/installed?mac=$(cat /sys/class/net/eth0/address)"
    // No body at all — there is nothing it needs to say beyond who it is.
    let response = s.raw(
        concat!(
            "POST /installed?mac=98:fa:9b:50:d8:10 HTTP/1.1\r\nHost: nas\r\n",
            "Authorization: Bearer nas:s3cr3t\r\n",
            "Content-Length: 0\r\n\r\n",
        )
        .as_bytes(),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.contains("installed-98-fa-9b-50-d8-10"),
        "{response}"
    );
    assert!(!s.dir().join("98-fa-9b-50-d8-10.ipxe").exists());
    // The kickstart itself is not an `.ipxe` and is left exactly where it was.
    assert!(s.dir().join("98-fa-9b-50-d8-10.ks").exists());

    // And a wrong bearer is refused, with nothing left to disarm anyway.
    let response = s.raw(
        concat!(
            "POST /installed?mac=aa:bb:cc:dd:ee:ff HTTP/1.1\r\nHost: nas\r\n",
            "Authorization: Bearer nas:wrong\r\n",
            "Content-Length: 0\r\n\r\n",
        )
        .as_bytes(),
    );
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
}
