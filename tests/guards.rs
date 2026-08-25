//! The answer endpoint's own token, and the capture of what machines really send.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const TOKEN: &str = "rescriptum-token-0123456789";

struct Server {
    child: Child,
    addr: String,
    dir: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Server {
    fn start(with_token: bool, with_capture: bool) -> Server {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pve-guard-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch");
        fs::write(dir.join("default.toml"), "marker = \"served\"\n").expect("fixture");

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
        cmd.env("RESCRIPTUM_ANSWERS_DIR", &dir)
            .env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
            .env("RESCRIPTUM_TIMEOUT_SECS", "5")
            .stderr(Stdio::piped())
            .stdout(Stdio::null());
        if with_token {
            cmd.env("RESCRIPTUM_ANSWER_TOKEN", TOKEN);
        }
        if with_capture {
            cmd.env("RESCRIPTUM_CAPTURE_DIR", dir.join("captured"));
        }
        let mut child = cmd.spawn().expect("spawn");

        let stderr = child.stderr.take().expect("piped");
        let mut lines = BufReader::new(stderr).lines();
        let mut addr = None;
        for _ in 0..6 {
            let Some(Ok(line)) = lines.next() else { break };
            if let Some(rest) = line.split("listening on ").nth(1) {
                addr = rest.split_whitespace().next().map(str::to_string);
                break;
            }
        }
        std::thread::spawn(move || lines.for_each(drop));

        Server {
            child,
            addr: addr.expect("never announced an address"),
            dir,
        }
    }

    fn post(&self, auth: Option<&str>, body: &str) -> String {
        let auth_line = match auth {
            Some(a) => format!("Authorization: {a}\r\n"),
            None => String::new(),
        };
        let request = format!(
            "POST /proxmox/answer HTTP/1.1\r\nHost: nas\r\n{auth_line}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut sock = TcpStream::connect(&self.addr).expect("connect");
        sock.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        sock.write_all(request.as_bytes()).expect("write");
        sock.flush().unwrap();
        let mut out = String::new();
        sock.read_to_string(&mut out).expect("read");
        out
    }
}

impl Server {
    /// A bare GET, for the endpoints that take no body.
    fn get(&self, path: &str) -> String {
        let request = format!("GET {path} HTTP/1.1\r\nHost: nas\r\n\r\n");
        let mut sock = TcpStream::connect(&self.addr).expect("connect");
        sock.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        sock.write_all(request.as_bytes()).expect("write");
        sock.flush().unwrap();
        let mut out = String::new();
        sock.read_to_string(&mut out).expect("read");
        out
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

#[test]
fn the_answer_endpoint_is_open_when_no_token_is_configured() {
    // Most installers cannot authenticate, so this has to stay the default.
    let s = Server::start(false, false);
    assert_eq!(status(&s.post(None, "{}")), 200);
}

#[test]
fn a_configured_token_is_required() {
    // What it guards is the root password hash and SSH keys of every machine installed
    // afterwards.
    let s = Server::start(true, false);
    assert_eq!(status(&s.post(None, "{}")), 401, "no header");
    assert_eq!(
        status(&s.post(Some("Bearer wrong-token-entirely"), "{}")),
        401
    );
    assert_eq!(
        status(&s.post(Some("Basic abc"), "{}")),
        401,
        "wrong scheme"
    );

    // Right length, one byte off — the case a timing attack would work towards.
    let near = format!("Bearer {}x", &TOKEN[..TOKEN.len() - 1]);
    assert_eq!(status(&s.post(Some(&near), "{}")), 401);

    assert_eq!(status(&s.post(Some(&format!("Bearer {TOKEN}")), "{}")), 200);
}

#[test]
fn health_stays_open_so_monitoring_does_not_go_dark() {
    let s = Server::start(true, false);
    let mut sock = TcpStream::connect(&s.addr).expect("connect");
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    sock.write_all(b"GET /health HTTP/1.1\r\nHost: nas\r\n\r\n")
        .unwrap();
    sock.flush().unwrap();
    let mut out = String::new();
    sock.read_to_string(&mut out).unwrap();
    assert_eq!(status(&out), 200, "{out}");
}

#[test]
fn what_a_machine_sends_can_be_captured_and_replayed() {
    // The gap this closes: everything here was built against documentation, never
    // against a request a real installer made.
    let s = Server::start(false, true);
    let body = r#"{"dmi":{"system":{"serial":"7ABC123"}},"network_interfaces":[{"mac":"aa:bb:cc:dd:ee:ff"}]}"#;
    assert_eq!(status(&s.post(None, body)), 200);

    let captured = s.dir.join("captured");
    let files: Vec<PathBuf> = fs::read_dir(&captured)
        .expect("capture directory")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(files.len(), 2, "one body and one meta: {files:?}");

    // Verbatim, so `render --body` can replay it.
    let body_file = files
        .iter()
        .find(|p| p.extension().unwrap() == "body")
        .unwrap();
    assert_eq!(fs::read_to_string(body_file).unwrap(), body);

    let meta_file = files
        .iter()
        .find(|p| p.extension().unwrap() == "meta")
        .unwrap();
    let meta = fs::read_to_string(meta_file).unwrap();
    assert!(meta.contains("POST /proxmox/answer"), "{meta}");
    assert!(meta.contains("outcome: 200"), "{meta}");

    // And it really replays.
    let out = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
        .env("RESCRIPTUM_ANSWERS_DIR", &s.dir)
        .arg("render")
        .arg("--body")
        .arg(body_file)
        .output()
        .expect("render");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("served"));
}

#[test]
fn capturing_is_off_unless_asked_for() {
    let s = Server::start(false, false);
    assert_eq!(status(&s.post(None, "{}")), 200);
    assert!(!s.dir.join("captured").exists());
}

#[test]
fn the_answer_endpoint_never_shuts_an_address_out_however_hard_it_gets_it_wrong() {
    // The deliberate asymmetry, and the one worth pinning: the admin API locks out a
    // guesser, this endpoint must not. A whole rack can sit behind one address, so
    // blocking it turns one machine's bad token into a failed rollout — and no
    // installer can be told to back off and try again later.
    let s = Server::start(true, false);

    for attempt in 1..=30 {
        let r = s.post(Some("Bearer wrong-token-here-0000"), "{}");
        assert_eq!(
            status(&r),
            401,
            "attempt {attempt} must be refused, not throttled\n{r}"
        );
        // And monitoring must not go dark while it happens.
        assert_eq!(
            status(&s.get("/health")),
            200,
            "health at attempt {attempt}"
        );
    }

    // The correct token still works afterwards: nothing was held against the address.
    assert_eq!(
        status(&s.post(Some(&format!("Bearer {TOKEN}")), "{}")),
        200,
        "a good token must not be punished for its neighbours"
    );
}

#[test]
fn the_token_guards_every_path_not_just_the_one_the_iso_names() {
    // The URL is baked into the ISO and this server does not choose it, so the check
    // cannot hang off a route.
    let s = Server::start(true, false);
    for path in [
        "/answer",
        "/proxmox/answer",
        "/",
        "/rhel/ks",
        "/anything/at/all",
    ] {
        let request = format!("GET {path} HTTP/1.1\r\nHost: nas\r\n\r\n");
        let mut sock = TcpStream::connect(&s.addr).expect("connect");
        sock.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        sock.write_all(request.as_bytes()).unwrap();
        sock.flush().unwrap();
        let mut out = String::new();
        sock.read_to_string(&mut out).unwrap();
        assert_eq!(status(&out), 401, "{path}: {out}");
    }

    // …except the one a monitor needs.
    assert_eq!(status(&s.get("/health")), 200);
}
