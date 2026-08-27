//! The media listener, end to end: the real binary, a real socket, real images.
//!
//! The unit tests stop at the module boundary, and the failures that matter here do
//! not. A range that comes back with the wrong `Content-Range`, a `HEAD` that carries a
//! body, an image download that starves the answer endpoint — none of those is visible
//! from inside a function.
//!
//! **Every abuse case ends by proving the server still answers.** That last assertion is
//! the one that matters: a listener that survives one bad request and then serves
//! nothing has failed the only test a provisioning server has to pass.

// There is nothing here to test in a binary built without the feature, and compiling to
// nothing is a clearer answer than a wall of unresolved imports.
#![cfg(feature = "boot")]

use rescriptum::boot::iso::build;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A server with both listeners up, plus the two directories it serves.
struct Server {
    child: Child,
    answer_addr: String,
    media_addr: String,
    media_dir: PathBuf,
    answers_dir: PathBuf,
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(images: &[(&str, Vec<u8>)]) -> Server {
        Server::start_env(images, &[])
    }

    fn start_env(images: &[(&str, Vec<u8>)], env: &[(&str, &str)]) -> Server {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("rescriptum-media-{}-{n}", std::process::id()));
        let media_dir = base.join("media");
        let answers_dir = base.join("answers");
        fs::create_dir_all(&media_dir).expect("media dir");
        fs::create_dir_all(&answers_dir).expect("answers dir");
        for (name, bytes) in images {
            fs::write(media_dir.join(name), bytes).expect("write image");
        }

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
        cmd.env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
            .env("RESCRIPTUM_ANSWERS_DIR", &answers_dir)
            .env("RESCRIPTUM_MEDIA_DIR", &media_dir)
            .env("RESCRIPTUM_MEDIA_ADDR", "127.0.0.1:0")
            .env("RESCRIPTUM_PUBLIC_HOST", "127.0.0.1")
            .env("RESCRIPTUM_TIMEOUT_SECS", "5")
            .stderr(Stdio::piped())
            .stdout(Stdio::null());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().expect("spawn server");

        // Both listeners announce the address they actually bound, so there is no port
        // race — and both have to be seen before a test can talk to either.
        let stderr = child.stderr.take().expect("piped stderr");
        let mut lines = BufReader::new(stderr).lines();
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (mut answer_addr, mut media_addr) = (None, None);
        for _ in 0..16 {
            let Some(Ok(line)) = lines.next() else { break };
            if let Some(rest) = line.split("listening on ").nth(1) {
                let bound = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if line.contains("media listening on") {
                    media_addr = Some(bound);
                } else {
                    answer_addr = Some(bound);
                }
            }
            log.lock().unwrap().push(line);
            if answer_addr.is_some() && media_addr.is_some() {
                break;
            }
        }

        let collected = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in lines.map_while(Result::ok) {
                collected.lock().unwrap().push(line);
            }
        });

        let seen = || format!("{:#?}", log.lock().unwrap());
        Server {
            answer_addr: answer_addr.unwrap_or_else(|| panic!("no answer address; saw {}", seen())),
            media_addr: media_addr.unwrap_or_else(|| panic!("no media address; saw {}", seen())),
            child,
            media_dir,
            answers_dir,
            log: Arc::clone(&log),
        }
    }

    fn media_dir(&self) -> &Path {
        &self.media_dir
    }

    fn startup_log(&self) -> String {
        self.log.lock().unwrap().join("\n")
    }

    /// A media request, returning the raw bytes: an image is not UTF-8.
    fn get(&self, path: &str) -> Vec<u8> {
        self.get_with(path, &[])
    }

    fn get_with(&self, path: &str, headers: &[(&str, &str)]) -> Vec<u8> {
        self.request("GET", path, headers)
    }

    fn request(&self, method: &str, path: &str, headers: &[(&str, &str)]) -> Vec<u8> {
        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: boot\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("Connection: close\r\n\r\n");

        let mut sock = TcpStream::connect(&self.media_addr).expect("connect to media");
        sock.set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        sock.write_all(request.as_bytes()).expect("write");
        sock.flush().unwrap();
        let mut out = Vec::new();
        sock.read_to_end(&mut out).expect("read");
        out
    }

    /// Ask the *answer* endpoint something, to prove it is unaffected.
    fn answer(&self, body: &str) -> String {
        let mut sock = TcpStream::connect(&self.answer_addr).expect("connect to answers");
        sock.set_read_timeout(Some(Duration::from_secs(10)))
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
        let _ = sock.read_to_string(&mut out);
        out
    }

    /// Run a subcommand of the same binary against the same configuration.
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_rescriptum"))
            .args(args)
            .env("RESCRIPTUM_MEDIA_DIR", &self.media_dir)
            .env("RESCRIPTUM_ANSWERS_DIR", &self.answers_dir)
            .env("RESCRIPTUM_PUBLIC_HOST", "192.0.2.10")
            .env("RESCRIPTUM_MEDIA_ADDR", "0.0.0.0:8001")
            .output()
            .expect("run subcommand")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(self.media_dir.parent().unwrap_or(&self.media_dir));
    }
}

// ---- helpers --------------------------------------------------------------

fn head_of(response: &[u8]) -> String {
    let split = find(response, b"\r\n\r\n").unwrap_or(response.len());
    String::from_utf8_lossy(&response[..split]).to_ascii_lowercase()
}

fn body_of(response: &[u8]) -> &[u8] {
    match find(response, b"\r\n\r\n") {
        Some(at) => &response[at + 4..],
        None => &[],
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn status(response: &[u8]) -> String {
    String::from_utf8_lossy(&response[..response.len().min(32)])
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// A header's value, from a lowercased copy: hyper emits header names lowercased, which
/// is correct, so the assertions test the contract rather than the casing.
fn header(response: &[u8], name: &str) -> Option<String> {
    head_of(response)
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}: ")).map(str::to_string))
}

fn bzimage() -> Vec<u8> {
    let mut kernel = vec![0u8; 0x400];
    kernel[0x202..0x206].copy_from_slice(b"HdrS");
    // Something recognisable at the front, so a range test can say where it landed.
    kernel[..6].copy_from_slice(b"KERNEL");
    kernel
}

/// A Proxmox image, complete enough to probe and to boot.
fn pve_image() -> Vec<u8> {
    build::Builder::new()
        .volume("PVE")
        .file("/boot/linux26", &bzimage())
        .file("/boot/initrd.img", b"\x1f\x8bINITRD-BYTES")
        .file(
            "/.disk/info",
            b"PRODUCTLONG='Proxmox Virtual Environment'\nRELEASE='8.4'\nISORELEASE='1'\nARCH='amd64'\n",
        )
        .build()
}

fn image_for(family: &str) -> Vec<u8> {
    let builder = build::Builder::new().volume("TEST");
    match family {
        "proxmox" => return pve_image(),
        "ubuntu" => builder
            .file("/casper/vmlinuz", &bzimage())
            .file("/casper/initrd", b"initrd"),
        "debian" => builder
            .file("/install.amd/vmlinuz", &bzimage())
            .file("/install.amd/initrd.gz", b"initrd"),
        "rhel" => builder
            .file("/images/pxeboot/vmlinuz", &bzimage())
            .file("/images/pxeboot/initrd.img", b"initrd"),
        "suse" => builder
            .file("/boot/x86_64/loader/linux", &bzimage())
            .file("/boot/x86_64/loader/initrd", b"initrd"),
        "coreos" => builder
            .file("/images/pxeboot/vmlinuz", &bzimage())
            .file("/images/pxeboot/initrd.img", b"initrd")
            .file("/images/pxeboot/rootfs.img", b"live root"),
        other => panic!("no fixture for {other}"),
    }
    .build()
}

// ---- the catalogue --------------------------------------------------------

#[test]
fn the_catalogue_lists_what_the_directory_holds() {
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let r = s.get("/");
    assert!(status(&r).starts_with("HTTP/1.1 200"), "{}", head_of(&r));

    let body = String::from_utf8_lossy(body_of(&r)).to_string();
    assert!(body.contains("pve-8.4"), "{body}");
    assert!(body.contains("proxmox"), "{body}");
    assert!(body.contains("bootable"), "{body}");
}

#[test]
fn the_catalogue_answers_json_when_asked_for_it() {
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let r = s.get_with("/", &[("Accept", "application/json")]);
    let body = String::from_utf8_lossy(body_of(&r)).to_string();
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["media"][0]["id"], "pve-8.4");
    assert_eq!(parsed["media"][0]["family"], "proxmox");
    assert_eq!(parsed["media"][0]["arch"], "x86_64");
    assert_eq!(parsed["media"][0]["bootable"], true);
}

#[test]
fn an_image_dropped_in_later_appears_without_a_restart() {
    // Discovered, not declared — the same guarantee the answers directory gives.
    let s = Server::start(&[]);
    assert!(
        String::from_utf8_lossy(body_of(&s.get("/"))).contains("no images"),
        "starts empty"
    );

    fs::write(s.media_dir().join("late.iso"), pve_image()).expect("write");
    std::thread::sleep(Duration::from_millis(1200));
    let body = String::from_utf8_lossy(body_of(&s.get("/"))).to_string();
    assert!(body.contains("late"), "{body}");
}

// ---- serving bytes --------------------------------------------------------

#[test]
fn an_image_is_served_whole_with_a_validator_and_a_length() {
    let image = pve_image();
    let s = Server::start(&[("pve-8.4.iso", image.clone())]);
    let r = s.get("/pve-8.4/iso");

    assert!(status(&r).starts_with("HTTP/1.1 200"), "{}", head_of(&r));
    assert_eq!(header(&r, "accept-ranges").as_deref(), Some("bytes"));
    assert_eq!(
        header(&r, "content-length").as_deref(),
        Some(image.len().to_string().as_str())
    );
    assert!(header(&r, "etag").is_some(), "{}", head_of(&r));
    assert_eq!(body_of(&r), image.as_slice(), "the bytes must be the image");
}

#[test]
fn a_kernel_is_streamed_from_inside_the_image_without_extraction() {
    // The property the whole listener rests on: a file in an ISO9660 image is one
    // contiguous extent, so this is a seek — nothing is unpacked and nothing is copied.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);

    let kernel = s.get("/pve-8.4/kernel");
    assert!(
        status(&kernel).starts_with("HTTP/1.1 200"),
        "{}",
        head_of(&kernel)
    );
    assert_eq!(body_of(&kernel), bzimage().as_slice());

    let initrd = s.get("/pve-8.4/initrd");
    assert_eq!(body_of(&initrd), b"\x1f\x8bINITRD-BYTES");
}

#[test]
fn a_file_inside_the_image_is_reachable_by_path() {
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let r = s.get("/pve-8.4/file/.disk/info");
    assert!(status(&r).starts_with("HTTP/1.1 200"), "{}", head_of(&r));
    assert!(
        String::from_utf8_lossy(body_of(&r)).contains("Proxmox"),
        "{:?}",
        body_of(&r)
    );
}

#[test]
fn traversal_out_of_the_image_is_refused_and_the_server_still_answers() {
    // The guard is structural — an ISO9660 image is its own root, so no such record
    // exists — and `..` is refused outright on top of that.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    for path in [
        "/pve-8.4/file/../../../etc/passwd",
        "/pve-8.4/file/..%2f..%2fetc/passwd",
        "/../etc/passwd",
    ] {
        let r = s.get(path);
        assert!(
            !String::from_utf8_lossy(body_of(&r)).contains("root:"),
            "{path} leaked something"
        );
    }
    assert!(status(&s.get("/pve-8.4/kernel")).starts_with("HTTP/1.1 200"));
}

#[test]
fn initrd_plus_iso_is_synthesised_rather_than_stored() {
    // The initrd, then a cpio header naming proxmox.iso, then the image. Old loaders
    // cannot do `initrd <uri> <name>` themselves, and building a 1.5 GB file on disk to
    // work around that is what this avoids.
    let image = pve_image();
    let s = Server::start(&[("pve-8.4.iso", image.clone())]);
    let r = s.get("/pve-8.4/initrd+iso");

    assert!(status(&r).starts_with("HTTP/1.1 200"), "{}", head_of(&r));
    let body = body_of(&r);
    assert!(
        body.starts_with(b"\x1f\x8bINITRD-BYTES"),
        "the initrd comes first"
    );
    assert!(find(body, b"070701").is_some(), "a cpio header follows");
    assert!(find(body, b"proxmox.iso\0").is_some(), "naming the image");
    assert!(find(body, b"TRAILER!!!").is_some(), "and the archive ends");
    // The image itself is in there, whole.
    assert!(
        find(body, &image[32768..32800]).is_some(),
        "the image is appended"
    );

    // Nothing resumes an initrd, and the header says so rather than letting a client
    // discover it by having a range quietly ignored.
    assert_eq!(header(&r, "accept-ranges").as_deref(), Some("none"));
    // And the declared length is exact arithmetic, not a guess.
    let declared: usize = header(&r, "content-length").unwrap().parse().unwrap();
    assert_eq!(declared, body.len());
}

// ---- ranges ---------------------------------------------------------------

#[test]
fn the_three_range_forms_come_back_as_partial_content() {
    // Five of the seven installers range-fetch — casper and anaconda both do — so this
    // is not a nicety.
    let image = pve_image();
    let s = Server::start(&[("pve-8.4.iso", image.clone())]);
    let total = image.len();

    for (spec, expected_range, expected_bytes) in [
        ("bytes=0-99", format!("bytes 0-99/{total}"), &image[0..100]),
        (
            "bytes=32768-32867",
            format!("bytes 32768-32867/{total}"),
            &image[32768..32868],
        ),
        (
            "bytes=-100",
            format!("bytes {}-{}/{total}", total - 100, total - 1),
            &image[total - 100..],
        ),
    ] {
        let r = s.get_with("/pve-8.4/iso", &[("Range", spec)]);
        assert!(
            status(&r).starts_with("HTTP/1.1 206"),
            "{spec}: {}",
            head_of(&r)
        );
        assert_eq!(
            header(&r, "content-range").as_deref(),
            Some(expected_range.as_str())
        );
        assert_eq!(body_of(&r), expected_bytes, "{spec}");
    }

    // `a-` to the end, checked separately because the body is large.
    let r = s.get_with("/pve-8.4/iso", &[("Range", "bytes=32768-")]);
    assert!(status(&r).starts_with("HTTP/1.1 206"), "{}", head_of(&r));
    assert_eq!(body_of(&r), &image[32768..]);
}

#[test]
fn a_range_inside_a_file_inside_the_image_lands_in_the_right_place() {
    // The offset has to move *within the extent*, which is what makes a resumed kernel
    // fetch work at all — and getting it wrong serves plausible bytes from the wrong
    // part of the image.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let r = s.get_with("/pve-8.4/kernel", &[("Range", "bytes=0-5")]);
    assert!(status(&r).starts_with("HTTP/1.1 206"), "{}", head_of(&r));
    assert_eq!(body_of(&r), b"KERNEL");

    let r = s.get_with("/pve-8.4/kernel", &[("Range", "bytes=514-517")]);
    assert_eq!(body_of(&r), b"HdrS", "0x202 is where the magic lives");
}

#[test]
fn a_range_past_the_end_is_416_carrying_the_real_length() {
    let image = pve_image();
    let s = Server::start(&[("pve-8.4.iso", image.clone())]);
    let r = s.get_with(
        "/pve-8.4/iso",
        &[("Range", format!("bytes={}-", image.len()).as_str())],
    );

    assert!(status(&r).starts_with("HTTP/1.1 416"), "{}", head_of(&r));
    assert_eq!(
        header(&r, "content-range").as_deref(),
        Some(format!("bytes */{}", image.len()).as_str()),
        "a 416 has to say how long the entity actually is"
    );
}

#[test]
fn a_multi_range_request_is_answered_whole() {
    // Permitted, and far better than half-implementing multipart/byteranges for a
    // client that does not exist.
    let image = pve_image();
    let s = Server::start(&[("pve-8.4.iso", image.clone())]);
    let r = s.get_with("/pve-8.4/iso", &[("Range", "bytes=0-99,200-299")]);

    assert!(status(&r).starts_with("HTTP/1.1 200"), "{}", head_of(&r));
    assert_eq!(body_of(&r).len(), image.len());
}

#[test]
fn a_resumed_transfer_that_raced_a_replacement_restarts() {
    // Splicing two images together produces a file that is neither, and the failure
    // surfaces as an install that goes wrong much later.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let etag = header(&s.get("/pve-8.4/iso"), "etag").expect("a validator");

    // Same validator: the range is honoured.
    let r = s.get_with(
        "/pve-8.4/iso",
        &[("Range", "bytes=0-99"), ("If-Range", &etag)],
    );
    assert!(status(&r).starts_with("HTTP/1.1 206"), "{}", head_of(&r));

    // A stale one: the whole entity comes back instead.
    let r = s.get_with(
        "/pve-8.4/iso",
        &[("Range", "bytes=0-99"), ("If-Range", "\"something-else\"")],
    );
    assert!(status(&r).starts_with("HTTP/1.1 200"), "{}", head_of(&r));
    assert!(body_of(&r).len() > 100, "the whole entity, not the range");
}

#[test]
fn head_is_answered_with_the_headers_and_no_body() {
    // UEFI HTTP Boot asks before it fetches, and a `HEAD` that carries a body is a
    // protocol violation the firmware reads as a broken server.
    let image = pve_image();
    let s = Server::start(&[("pve-8.4.iso", image.clone())]);
    let r = s.request("HEAD", "/pve-8.4/iso", &[]);

    assert!(status(&r).starts_with("HTTP/1.1 200"), "{}", head_of(&r));
    assert_eq!(
        header(&r, "content-length").as_deref(),
        Some(image.len().to_string().as_str()),
        "the length is the entity's, not the body's"
    );
    assert!(body_of(&r).is_empty(), "a HEAD carries no body");
}

// ---- refusals, and surviving them -----------------------------------------

#[test]
fn an_unknown_id_is_404_and_the_server_still_answers() {
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    for path in ["/nope/iso", "/pve-8.4/nonsense", "/pve-8.4"] {
        let r = s.get(path);
        assert!(
            status(&r).starts_with("HTTP/1.1 404"),
            "{path}: {}",
            head_of(&r)
        );
    }
    assert!(status(&s.get("/pve-8.4/iso")).starts_with("HTTP/1.1 200"));
}

#[test]
fn an_image_with_no_kernel_says_so_rather_than_serving_nothing() {
    let s = Server::start(&[("mystery.iso", vec![0u8; 128 * 1024])]);
    let r = s.get("/mystery/kernel");
    assert!(status(&r).starts_with("HTTP/1.1 404"), "{}", head_of(&r));
    assert!(
        String::from_utf8_lossy(body_of(&r)).contains("no kernel"),
        "{:?}",
        String::from_utf8_lossy(body_of(&r))
    );
    // But the image itself is still servable: not describable is not the same as not
    // usable — `sanboot` takes one, and so does a USB stick.
    assert!(status(&s.get("/mystery/iso")).starts_with("HTTP/1.1 200"));
}

#[test]
fn writing_is_refused_because_the_listener_is_read_only() {
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    for method in ["PUT", "DELETE", "POST"] {
        let r = s.request(method, "/pve-8.4/iso", &[]);
        assert!(
            status(&r).starts_with("HTTP/1.1 405"),
            "{method}: {}",
            head_of(&r)
        );
        assert!(head_of(&r).contains("allow: get, head"), "{}", head_of(&r));
    }
    assert!(status(&s.get("/pve-8.4/iso")).starts_with("HTTP/1.1 200"));
}

#[test]
fn a_truncated_download_does_not_stop_the_server() {
    // The client hangs up mid-transfer, which is what a rebooting machine does. The
    // blocking reader has to notice and stop rather than finishing an image nobody is
    // receiving.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    {
        let mut sock = TcpStream::connect(&s.media_addr).expect("connect");
        sock.write_all(b"GET /pve-8.4/iso HTTP/1.1\r\nHost: boot\r\nConnection: close\r\n\r\n")
            .expect("write");
        sock.flush().unwrap();
        let mut first = [0u8; 64];
        let _ = sock.read(&mut first);
        // Dropped here, mid-transfer.
    }

    assert!(
        status(&s.get("/pve-8.4/iso")).starts_with("HTTP/1.1 200"),
        "the server still answers"
    );
}

#[test]
fn an_allowlist_refuses_a_peer_outside_it() {
    // Boot traffic is unauthenticated by necessity — a PXE ROM has no credentials — so
    // this is the only control that can say "not you".
    let s = Server::start_env(
        &[("pve-8.4.iso", pve_image())],
        &[("RESCRIPTUM_BOOT_ALLOW", "10.99.0.0/16")],
    );
    let r = s.get("/pve-8.4/iso");
    assert!(status(&r).starts_with("HTTP/1.1 403"), "{}", head_of(&r));

    let s = Server::start_env(
        &[("pve-8.4.iso", pve_image())],
        &[("RESCRIPTUM_BOOT_ALLOW", "127.0.0.0/8, 10.0.0.0/8")],
    );
    assert!(status(&s.get("/pve-8.4/iso")).starts_with("HTTP/1.1 200"));
}

// ---- the assertion that matters -------------------------------------------

#[test]
fn image_downloads_never_starve_the_answer_endpoint() {
    // **This is the one that could veto the design.** A download holds its permit for
    // minutes; if answers shared that budget, a rollout would starve its own installs.
    // Two listeners, two budgets — and this is what proves it rather than hoping.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    fs::write(
        s.answers_dir.join("default.toml"),
        "[global]\nkeyboard = \"fr\"\n",
    )
    .expect("write answer");
    std::thread::sleep(Duration::from_millis(1200));

    // Four transfers in flight, each deliberately left unread so it stays open.
    let mut holding = Vec::new();
    for _ in 0..4 {
        let mut sock = TcpStream::connect(&s.media_addr).expect("connect");
        sock.write_all(b"GET /pve-8.4/iso HTTP/1.1\r\nHost: boot\r\nConnection: close\r\n\r\n")
            .expect("write");
        sock.flush().unwrap();
        holding.push(sock);
    }

    let answer = s.answer(r#"{"mac":"98:fa:9b:50:d8:10"}"#);
    assert!(
        answer.starts_with("HTTP/1.1 200"),
        "answers must keep working while media transfers run: {answer}"
    );
    assert!(answer.contains("\"fr\""), "{answer}");

    drop(holding);
    assert!(status(&s.get("/pve-8.4/iso")).starts_with("HTTP/1.1 200"));
}

#[test]
fn the_media_listener_has_its_own_health_and_its_own_socket() {
    let s = Server::start(&[]);
    assert!(status(&s.get("/health")).starts_with("HTTP/1.1 200"));
    assert_ne!(s.media_addr, s.answer_addr, "never the same socket");
}

// ---- the stanzas, per family ----------------------------------------------

#[test]
fn every_family_gets_its_own_boot_stanza() {
    // **A stanza proven for one family claims six**, the way `tests/stores.rs` asserts
    // per store. Proxmox is the founding case and the odd one out — a parameter that
    // selects the automated path, and the image carried as a second initrd, where every
    // other family takes a URL on the command line.
    let cases: &[(&str, &str)] = &[
        ("proxmox", "proxmox-start-auto-installer"),
        ("debian", "preseed/url=http://192.0.2.10:8000/debian"),
        ("ubuntu", "ds=nocloud-net;s=http://192.0.2.10:8000/ubuntu/"),
        ("rhel", "inst.ks=http://192.0.2.10:8000/rhel"),
        ("suse", "autoyast=http://192.0.2.10:8000/suse"),
        (
            "coreos",
            "ignition.config.url=http://192.0.2.10:8000/coreos",
        ),
    ];

    let images: Vec<(&str, Vec<u8>)> = cases
        .iter()
        .map(|(family, _)| (*family, image_for(family)))
        .collect();
    let named: Vec<(&str, Vec<u8>)> = images
        .iter()
        .map(|(family, bytes)| {
            (
                match *family {
                    "proxmox" => "proxmox.iso",
                    "debian" => "debian.iso",
                    "ubuntu" => "ubuntu.iso",
                    "rhel" => "rhel.iso",
                    "suse" => "suse.iso",
                    _ => "coreos.iso",
                },
                bytes.clone(),
            )
        })
        .collect();
    let s = Server::start(&named);

    for (family, needle) in cases {
        let out = s.run(&["media", "ipxe", family]);
        assert!(
            out.status.success(),
            "{family}: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        let script = String::from_utf8_lossy(&out.stdout);
        assert!(script.starts_with("#!ipxe\n"), "{family}: {script}");
        assert!(script.contains(needle), "{family}: {script}");
        // stdout is the script and stderr is everything else, so `> answer.ipxe` works.
        assert!(!script.contains("# warning"), "{family}: {script}");
        // Every stanza names the media listener's own port, never the answer one's.
        assert!(
            script.contains("http://192.0.2.10:8001/"),
            "{family}: {script}"
        );
    }
}

#[test]
fn a_generated_stanza_is_an_ordinary_answer_document() {
    // The altitude that keeps the model intact: `media ipxe` prints a script, it does
    // not install one. Saved into the answers directory it is selected, layered and
    // templated like anything else — which is what this proves by serving it.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let out = s.run(&["media", "ipxe", "pve-8.4"]);
    assert!(out.status.success());

    fs::write(
        s.answers_dir.join("98-fa-9b-50-d8-10.ipxe"),
        String::from_utf8_lossy(&out.stdout).as_ref(),
    )
    .expect("save the generated answer");
    std::thread::sleep(Duration::from_millis(1200));

    let served = s.answer(r#"{"mac":"98:fa:9b:50:d8:10"}"#);
    assert!(served.starts_with("HTTP/1.1 200"), "{served}");
    assert!(served.contains("proxmox-start-auto-installer"), "{served}");
    assert!(
        served.contains("initrd http://192.0.2.10:8001/pve-8.4/iso proxmox.iso"),
        "{served}"
    );
}

// ---- the CLI half ---------------------------------------------------------

#[test]
fn media_add_records_a_digest_and_media_check_re_verifies_it() {
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let image = s.media_dir().join("pve-8.4.iso");

    let out = s.run(&["media", "add", image.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(printed.contains("pve-8.4"), "{printed}");
    assert!(printed.contains("proxmox"), "{printed}");
    assert!(
        s.media_dir().join("pve-8.4.media").is_file(),
        "a sidecar is written"
    );

    let out = s.run(&["media", "check"]);
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("1 verified"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn media_check_fails_when_an_image_changed_under_its_digest() {
    // The one failure that silently installs something nobody reviewed. Its exit code
    // is a contract, like `check`'s — `deploy.sh` keys on it.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let image = s.media_dir().join("pve-8.4.iso");
    assert!(
        s.run(&["media", "add", image.to_str().unwrap()])
            .status
            .success()
    );

    let mut changed = pve_image();
    let last = changed.len() - 1;
    changed[last] ^= 0xff;
    fs::write(&image, changed).expect("replace the image");

    let out = s.run(&["media", "check"]);
    assert!(!out.status.success(), "a drifted image must fail the check");
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(printed.contains("no longer matches"), "{printed}");
}

#[test]
fn media_add_refuses_a_digest_that_does_not_match() {
    // A mismatch is a truncated download or the wrong file, and both install the wrong
    // thing on every machine that asks.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let image = s.media_dir().join("pve-8.4.iso");

    let out = s.run(&[
        "media",
        "add",
        image.to_str().unwrap(),
        "--sha256",
        &"a".repeat(64),
    ]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("digest mismatch"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !s.media_dir().join("pve-8.4.media").exists(),
        "nothing may be recorded when the digest is wrong"
    );
}

#[test]
fn media_list_shows_what_the_catalogue_holds() {
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let out = s.run(&["media", "list"]);
    assert!(out.status.success());
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(printed.contains("pve-8.4"), "{printed}");
    assert!(printed.contains("proxmox"), "{printed}");
    assert!(printed.contains("x86_64"), "{printed}");
}

#[test]
fn a_derived_public_host_is_announced_loudly() {
    // A wrong guess here produces a machine that boots, chains, and hangs on an address
    // that does not exist — and this log line is the only place the answer appears.
    let s = Server::start_env(&[], &[("RESCRIPTUM_PUBLIC_HOST", "")]);
    let log = s.startup_log();
    assert!(log.contains("RESCRIPTUM_PUBLIC_HOST is not set"), "{log}");
    assert!(log.contains("derived"), "{log}");
}

// ---- the generated scripts -------------------------------------------------

#[test]
fn the_bootstrap_is_served_when_the_answer_set_is_empty() {
    // **This is the whole reason it lives on the media listener.** It has to work
    // before anybody has written a single answer, which is the state every new install
    // starts in — and it is what puts a MAC in the query string, without which the
    // selection engine matches nothing but the default.
    let s = Server::start(&[]);
    let r = s.get("/ipxe/bootstrap");

    assert!(status(&r).starts_with("HTTP/1.1 200"), "{}", head_of(&r));
    let script = String::from_utf8_lossy(body_of(&r)).to_string();
    assert!(script.starts_with("#!ipxe\n"), "{script}");
    assert!(script.contains("mac=${netX/mac}"), "{script}");
    assert!(script.contains("/ipxe/boot?"), "{script}");
    // And an unclaimed machine falls through to the menu, which is `default.toml`'s
    // job description applied to a different format.
    assert!(script.contains("|| chain"), "{script}");
    assert!(script.contains("/ipxe/menu"), "{script}");
}

#[test]
fn the_menu_is_rendered_from_the_catalogue_at_request_time() {
    // Not a file kept in sync: drop an ISO in the directory and it is in the menu on
    // the next fetch. That is the same instinct as answers being discovered.
    let s = Server::start(&[("pve-8.4.iso", pve_image())]);
    let script = String::from_utf8_lossy(body_of(&s.get("/ipxe/menu"))).to_string();
    assert!(script.contains("item pve-8.4"), "{script}");
    assert!(script.contains(":pve-8.4"), "a label to goto: {script}");

    fs::write(s.media_dir().join("late.iso"), pve_image()).expect("write");
    std::thread::sleep(Duration::from_millis(1200));
    let script = String::from_utf8_lossy(body_of(&s.get("/ipxe/menu"))).to_string();
    assert!(script.contains("item late"), "{script}");
}

#[test]
fn the_menu_is_never_cached() {
    // A cached menu shows yesterday's images, and the operator who just dropped an ISO
    // in has no way to tell that from a broken catalogue.
    let s = Server::start(&[]);
    let r = s.get("/ipxe/menu");
    assert_eq!(header(&r, "cache-control").as_deref(), Some("no-store"));
}

#[test]
fn the_menu_falls_through_to_the_local_disk() {
    // **The safety behaviour that must not be lost.** A machine that PXE-boots by
    // accident, and that nothing claims, ends up on its own disk after a few seconds —
    // it never sits at a menu waiting for a human who is not coming, and it never
    // installs anything.
    let s = Server::start(&[]);
    let script = String::from_utf8_lossy(body_of(&s.get("/ipxe/menu"))).to_string();
    assert!(
        script.contains("--default local target || goto local"),
        "{script}"
    );
    assert!(script.contains(":local\n"), "{script}");
}

#[test]
fn a_boot_asset_is_served_over_http_for_uefi_http_boot() {
    // Firmware that HTTP-boots fetches its loader here rather than over TFTP — the
    // shortest chain there is, and it skips TFTP entirely.
    let s = Server::start_env(&[], &[]);
    let boot_dir = s.media_dir().parent().expect("base").join("boot");
    fs::create_dir_all(&boot_dir).expect("boot dir");
    fs::write(boot_dir.join("ipxe-x86_64.efi"), b"a loader, pretend").expect("write");

    // A fresh server, now with the boot directory named.
    let s = Server::start_env(&[], &[("RESCRIPTUM_BOOT_DIR", boot_dir.to_str().unwrap())]);
    let r = s.get("/boot/ipxe-x86_64.efi");
    assert!(status(&r).starts_with("HTTP/1.1 200"), "{}", head_of(&r));
    assert_eq!(body_of(&r), b"a loader, pretend");

    // And nothing outside that directory.
    for path in ["/boot/../../etc/passwd", "/boot/nope.efi"] {
        let r = s.get(path);
        assert!(
            status(&r).starts_with("HTTP/1.1 404"),
            "{path}: {}",
            head_of(&r)
        );
    }
    let _ = fs::remove_dir_all(&boot_dir);
}

#[test]
fn a_boot_asset_route_with_no_boot_directory_says_which_setting_is_missing() {
    let s = Server::start(&[]);
    let r = s.get("/boot/ipxe-x86_64.efi");
    assert!(status(&r).starts_with("HTTP/1.1 404"), "{}", head_of(&r));
    assert!(
        String::from_utf8_lossy(body_of(&r)).contains("RESCRIPTUM_BOOT_DIR"),
        "a 404 that names the setting beats one that does not: {:?}",
        String::from_utf8_lossy(body_of(&r))
    );
}
