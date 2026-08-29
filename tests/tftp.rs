//! TFTP against the real binary, over real UDP.
//!
//! Nothing here can be proved from inside a function. A transfer is a conversation —
//! blocks, acknowledgements, retransmission, the empty packet that ends it — and every
//! bug this file exists to catch lives in the turn-taking rather than in the parsing.
//!
//! The one that motivated writing it: **a file whose length is an exact multiple of the
//! block size must end with an empty data packet.** Leave it out and the client waits
//! forever for a final block that never comes. The unit tests could not see it; the
//! first run of `a_file_that_is_an_exact_multiple_of_the_block_size_still_ends` did.

#![cfg(feature = "boot")]

mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const OP_RRQ: u16 = 1;
const OP_WRQ: u16 = 2;
const OP_DATA: u16 = 3;
const OP_ACK: u16 = 4;
const OP_ERROR: u16 = 5;
const OP_OACK: u16 = 6;

struct Server {
    child: Child,
    tftp_addr: String,
    boot_dir: PathBuf,
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(files: &[(&str, Vec<u8>)]) -> Server {
        Server::start_env(files, &[])
    }

    fn start_env(files: &[(&str, Vec<u8>)], env: &[(&str, &str)]) -> Server {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("rescriptum-tftp-it-{}-{n}", std::process::id()));
        let boot_dir = base.join("boot");
        let answers_dir = base.join("answers");
        fs::create_dir_all(&boot_dir).expect("boot dir");
        fs::create_dir_all(&answers_dir).expect("answers dir");
        for (name, bytes) in files {
            let path = boot_dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("nested");
            }
            fs::write(&path, bytes).expect("write");
        }

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
        cmd.env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
            .env("RESCRIPTUM_ANSWERS_DIR", &answers_dir)
            .env("RESCRIPTUM_BOOT_DIR", &boot_dir)
            // Port 69 is privileged and the test suite is not root.
            .env("RESCRIPTUM_TFTP_ADDR", "127.0.0.1:0")
            .stderr(Stdio::piped())
            .stdout(Stdio::null());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().expect("spawn server");

        let stderr = child.stderr.take().expect("piped stderr");
        let mut lines = BufReader::new(stderr).lines();
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut tftp_addr = None;
        for _ in 0..16 {
            let Some(Ok(line)) = lines.next() else { break };
            if line.contains("tftp listening on")
                && let Some(rest) = line.split("listening on ").nth(1)
            {
                tftp_addr = Some(
                    rest.split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            let done = tftp_addr.is_some() && line.contains("rescriptum ");
            log.lock().unwrap().push(line);
            if done {
                break;
            }
        }
        let collected = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in lines.map_while(Result::ok) {
                collected.lock().unwrap().push(line);
            }
        });

        let tftp_addr =
            tftp_addr.unwrap_or_else(|| panic!("no tftp address; saw {:#?}", log.lock().unwrap()));
        Server {
            child,
            tftp_addr,
            boot_dir,
            log,
        }
    }

    /// What the server has said. Worth surfacing in an assertion: a TFTP failure that
    /// only shows as silence is indistinguishable from a dead server, and that is as
    /// true for a test as it is at power-on.
    fn log(&self) -> String {
        self.log.lock().unwrap().join("\n")
    }

    fn client(&self) -> Client {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind client");
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        Client {
            socket,
            server: self.tftp_addr.clone(),
            peer: None,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(self.boot_dir.parent().unwrap_or(&self.boot_dir));
    }
}

/// A TFTP client, hand-rolled, because the point is to speak the protocol rather than
/// to trust a library that speaks it the same way the server does.
struct Client {
    socket: UdpSocket,
    server: String,
    /// The ephemeral port the server answered from. RFC 1350 requires the data to come
    /// from a *new* port, and the rest of the conversation goes there.
    peer: Option<std::net::SocketAddr>,
}

impl Client {
    fn request(&mut self, opcode: u16, filename: &str, mode: &str, options: &[(&str, &str)]) {
        let mut packet = Vec::new();
        packet.extend_from_slice(&opcode.to_be_bytes());
        packet.extend_from_slice(filename.as_bytes());
        packet.push(0);
        packet.extend_from_slice(mode.as_bytes());
        packet.push(0);
        for (name, value) in options {
            packet.extend_from_slice(name.as_bytes());
            packet.push(0);
            packet.extend_from_slice(value.as_bytes());
            packet.push(0);
        }
        self.socket
            .send_to(&packet, &self.server)
            .expect("send request");
    }

    fn read(&mut self, filename: &str, options: &[(&str, &str)]) {
        self.request(OP_RRQ, filename, "octet", options);
    }

    /// One packet from the server, and remember which port it came from.
    fn receive(&mut self) -> Option<(u16, Vec<u8>)> {
        let mut buffer = vec![0u8; 2048];
        let (n, from) = self.socket.recv_from(&mut buffer).ok()?;
        self.peer = Some(from);
        if n < 2 {
            return None;
        }
        let opcode = u16::from_be_bytes([buffer[0], buffer[1]]);
        Some((opcode, buffer[2..n].to_vec()))
    }

    fn ack(&mut self, block: u16) {
        let mut packet = Vec::with_capacity(4);
        packet.extend_from_slice(&OP_ACK.to_be_bytes());
        packet.extend_from_slice(&block.to_be_bytes());
        let peer = self.peer.expect("the server has answered");
        self.socket.send_to(&packet, peer).expect("send ack");
    }

    /// Run a whole transfer, acknowledging as a real client would, and return what came
    /// back plus how many data packets it took.
    fn fetch(
        &mut self,
        filename: &str,
        options: &[(&str, &str)],
    ) -> Result<(Vec<u8>, usize), String> {
        self.read(filename, options);
        let mut body = Vec::new();
        let mut packets = 0usize;
        let mut expected: u16 = 1;

        loop {
            let Some((opcode, payload)) = self.receive() else {
                return Err(format!("no reply after {packets} data packet(s)"));
            };
            match opcode {
                OP_OACK => {
                    // Options accepted: acknowledge the set with block zero, then data
                    // starts.
                    self.ack(0);
                    continue;
                }
                OP_ERROR => {
                    let code = u16::from_be_bytes([payload[0], payload[1]]);
                    let message = String::from_utf8_lossy(&payload[2..])
                        .trim_end_matches('\0')
                        .to_string();
                    return Err(format!("error {code}: {message}"));
                }
                OP_DATA => {
                    let block = u16::from_be_bytes([payload[0], payload[1]]);
                    let data = &payload[2..];
                    assert_eq!(block, expected, "blocks must arrive in order");
                    packets += 1;
                    body.extend_from_slice(data);
                    self.ack(block);
                    // A short block ends the transfer, and "short" includes empty.
                    let block_size = options
                        .iter()
                        .find(|(k, _)| *k == "blksize")
                        .and_then(|(_, v)| v.parse::<usize>().ok())
                        .unwrap_or(512);
                    if data.len() < block_size {
                        return Ok((body, packets));
                    }
                    expected = expected.wrapping_add(1);
                }
                other => return Err(format!("unexpected opcode {other}")),
            }
        }
    }
}

fn loader(size: usize) -> Vec<u8> {
    // Not zeros: a truncated or spliced transfer has to be visible in the bytes.
    (0..size).map(|i| (i % 251) as u8).collect()
}

// ---- the transfer itself --------------------------------------------------

#[test]
fn a_loader_comes_back_byte_for_byte() {
    let bytes = loader(3000);
    let s = Server::start(&[("ipxe-undionly.kpxe", bytes.clone())]);
    let (body, packets) = s
        .client()
        .fetch("ipxe-undionly.kpxe", &[])
        .expect("a transfer");

    assert_eq!(body, bytes);
    // 3000 bytes at 512 a block: five full blocks and a short one.
    assert_eq!(packets, 6);
}

#[test]
fn a_file_that_is_an_exact_multiple_of_the_block_size_still_ends() {
    // **The bug this file was written for.** A short block ends a transfer, and a file
    // that divides exactly has no short block — so the transfer has to end with an
    // *empty* one. Leave it out and a real ROM waits forever for a final block that
    // never comes, which looks like a dead server rather than an off-by-one.
    for size in [512usize, 1024, 1468 * 2] {
        let block = if size % 512 == 0 { 512 } else { 1468 };
        let options = [("blksize", block.to_string())];
        let options: Vec<(&str, &str)> = options.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let bytes = loader(size);
        let s = Server::start(&[("loader.kpxe", bytes.clone())]);
        let (body, packets) = s
            .client()
            .fetch("loader.kpxe", &options)
            .unwrap_or_else(|e| panic!("{size} bytes at {block}: {e}"));

        assert_eq!(body, bytes, "{size} bytes");
        assert_eq!(
            packets,
            size / block + 1,
            "{size} bytes at {block} must end with an empty packet"
        );
    }
}

#[test]
fn an_empty_file_is_one_empty_packet() {
    let s = Server::start(&[("empty.kpxe", Vec::new())]);
    let (body, packets) = s.client().fetch("empty.kpxe", &[]).expect("a transfer");
    assert!(body.is_empty());
    assert_eq!(packets, 1);
}

// ---- options --------------------------------------------------------------

#[test]
fn the_options_real_roms_need_are_negotiated() {
    // RFC 1350 alone is not enough. `blksize` because 512-byte blocks make a megabyte
    // 2,000 round-trips, and `tsize` because a number of ROMs will not proceed without
    // being told the size up front.
    let bytes = loader(5000);
    let s = Server::start(&[("ipxe.kpxe", bytes.clone())]);
    let mut client = s.client();
    client.read("ipxe.kpxe", &[("blksize", "1468"), ("tsize", "0")]);

    let (opcode, payload) = client.receive().expect("a reply");
    assert_eq!(opcode, OP_OACK, "options must be acknowledged before data");

    let fields: Vec<String> = payload
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).to_string())
        .collect();
    let pairs: Vec<(&str, &str)> = fields
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| (c[0].as_str(), c[1].as_str()))
        .collect();
    assert!(pairs.contains(&("blksize", "1468")), "{pairs:?}");
    assert!(
        pairs.contains(&("tsize", "5000")),
        "tsize must be the real length: {pairs:?}"
    );
}

#[test]
fn an_oversized_block_is_clamped_to_one_ethernet_frame() {
    // 1500 − 20 (IP) − 8 (UDP) − 4 (TFTP) = 1468. Anything larger invites
    // fragmentation, and a fragmented transfer to a PXE ROM is a coin toss.
    let s = Server::start(&[("ipxe.kpxe", loader(4000))]);
    let mut client = s.client();
    client.read("ipxe.kpxe", &[("blksize", "9000")]);

    let (opcode, payload) = client.receive().expect("a reply");
    assert_eq!(opcode, OP_OACK);
    assert!(
        String::from_utf8_lossy(&payload).contains("1468"),
        "{:?}",
        String::from_utf8_lossy(&payload)
    );
}

#[test]
fn an_option_we_do_not_implement_is_simply_left_out() {
    // RFC 2347's way of declining: name only what you accepted.
    let s = Server::start(&[("ipxe.kpxe", loader(100))]);
    let mut client = s.client();
    client.read("ipxe.kpxe", &[("blksize", "1024"), ("mtftp", "yes")]);

    let (opcode, payload) = client.receive().expect("a reply");
    assert_eq!(opcode, OP_OACK);
    let text = String::from_utf8_lossy(&payload);
    assert!(text.contains("1024"), "{text}");
    assert!(!text.contains("mtftp"), "{text}");
}

// ---- refusals -------------------------------------------------------------

#[test]
fn a_write_request_is_refused_and_the_server_keeps_serving() {
    // Read-only is the posture, and writing a loader over unauthenticated UDP would be
    // a way to change what every machine on the segment boots.
    let s = Server::start(&[("ipxe.kpxe", loader(100))]);
    let mut client = s.client();
    client.request(OP_WRQ, "evil.kpxe", "octet", &[]);

    let (opcode, payload) = client.receive().expect("a reply");
    assert_eq!(opcode, OP_ERROR);
    assert_eq!(
        u16::from_be_bytes([payload[0], payload[1]]),
        2,
        "access violation"
    );

    assert!(s.client().fetch("ipxe.kpxe", &[]).is_ok(), "still serving");
}

#[test]
fn netascii_is_refused_rather_than_corrupting_a_binary() {
    let s = Server::start(&[("ipxe.kpxe", loader(100))]);
    let mut client = s.client();
    client.request(OP_RRQ, "ipxe.kpxe", "netascii", &[]);

    let (opcode, payload) = client.receive().expect("a reply");
    assert_eq!(opcode, OP_ERROR);
    assert!(
        String::from_utf8_lossy(&payload).contains("octet"),
        "the reason has to name the mode we do take"
    );
}

#[test]
fn nothing_outside_the_root_can_be_fetched() {
    let s = Server::start(&[("ipxe.kpxe", loader(100))]);
    for name in [
        "../../../etc/passwd",
        "/etc/passwd",
        "subdir/../../etc/passwd",
    ] {
        let e = s
            .client()
            .fetch(name, &[])
            .expect_err(&format!("{name} must be refused"));
        assert!(e.contains("error 1"), "{name}: {e}");
    }
    assert!(s.client().fetch("ipxe.kpxe", &[]).is_ok(), "still serving");
}

#[test]
fn a_missing_file_is_an_error_rather_than_silence() {
    // Silence would look identical to a dead server, and at power-on nobody can tell.
    let s = Server::start(&[("ipxe.kpxe", loader(100))]);
    let e = s.client().fetch("nope.kpxe", &[]).expect_err("must refuse");
    assert!(e.contains("error 1"), "{e}");
}

#[test]
fn a_nonsense_packet_does_not_stop_the_server() {
    let s = Server::start(&[("ipxe.kpxe", loader(100))]);
    {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind");
        for junk in [
            &b""[..],
            &b"\x00"[..],
            &b"\xff\xff\xff\xff"[..],
            &[0u8; 900][..],
        ] {
            let _ = socket.send_to(junk, &s.tftp_addr);
        }
    }
    assert!(
        s.client().fetch("ipxe.kpxe", &[]).is_ok(),
        "still serving; the server said:\n{}",
        s.log()
    );
}

// ---- the conversation's own hazards ---------------------------------------

#[test]
fn a_duplicate_acknowledgement_is_never_answered() {
    // **The Sorcerer's Apprentice bug.** Answering a duplicate acknowledgement with a
    // duplicate data packet makes both sides echo each other, and the transfer's
    // traffic doubles for as long as it lasts. The fix is to ignore it — which means
    // acknowledging block 1 twice must produce exactly one block 2, not two.
    let s = Server::start(&[("ipxe.kpxe", loader(2000))]);
    let mut client = s.client();
    client.read("ipxe.kpxe", &[]);

    let (opcode, payload) = client.receive().expect("block 1");
    assert_eq!(opcode, OP_DATA);
    assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1);

    client.ack(1);
    client.ack(1); // the duplicate

    let (opcode, payload) = client.receive().expect("block 2");
    assert_eq!(opcode, OP_DATA);
    assert_eq!(
        u16::from_be_bytes([payload[0], payload[1]]),
        2,
        "the next block, not a repeat of the first"
    );

    // And nothing else is in flight: the next thing to arrive is block 3, after we ask.
    client.ack(2);
    let (_, payload) = client.receive().expect("block 3");
    assert_eq!(
        u16::from_be_bytes([payload[0], payload[1]]),
        3,
        "a second block 2 would mean the duplicate was answered"
    );
}

#[test]
fn a_lost_acknowledgement_is_recovered_by_retransmission() {
    // UDP correctness never exercised under loss is a hope, not a property. Here the
    // loss is induced: the client simply does not acknowledge block 1 the first time,
    // and the server has to send it again.
    let s = Server::start(&[("ipxe.kpxe", loader(2000))]);
    let mut client = s.client();
    client.read("ipxe.kpxe", &[]);

    let (_, first) = client.receive().expect("block 1");
    assert_eq!(u16::from_be_bytes([first[0], first[1]]), 1);
    // Say nothing. The server's retry timer must fire and resend the same block.

    let (opcode, again) = client.receive().expect("block 1, again");
    assert_eq!(opcode, OP_DATA);
    assert_eq!(
        u16::from_be_bytes([again[0], again[1]]),
        1,
        "the same block"
    );
    assert_eq!(again, first, "byte for byte, not a fresh read");

    // And acknowledging it now still finishes the transfer.
    client.ack(1);
    let (_, payload) = client.receive().expect("block 2");
    assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 2);
}

#[test]
fn the_data_comes_from_a_fresh_port() {
    // RFC 1350 requires it, and it is what lets the server hold many transfers at once
    // on one well-known port. A server answering from port 69 would work against one
    // client and fail against two.
    let s = Server::start(&[("ipxe.kpxe", loader(100))]);
    let mut client = s.client();
    client.read("ipxe.kpxe", &[]);
    client.receive().expect("a reply");

    let from = client.peer.expect("answered");
    let listener: std::net::SocketAddr = s.tftp_addr.parse().expect("address");
    assert_ne!(from.port(), listener.port(), "the data port must be fresh");
}

#[test]
fn two_transfers_at_once_do_not_cross_over() {
    // The failure this pins is the one that installs the wrong machine: two clients,
    // two files, and each must receive only its own bytes.
    let a = loader(1500);
    let b: Vec<u8> = loader(1500).iter().map(|x| x ^ 0xff).collect();
    let s = Server::start(&[("a.kpxe", a.clone()), ("b.kpxe", b.clone())]);

    let mut first = s.client();
    let mut second = s.client();
    first.read("a.kpxe", &[]);
    second.read("b.kpxe", &[]);

    let mut got_a = Vec::new();
    let mut got_b = Vec::new();
    for _ in 0..4 {
        if let Some((OP_DATA, payload)) = first.receive() {
            got_a.extend_from_slice(&payload[2..]);
            let block = u16::from_be_bytes([payload[0], payload[1]]);
            first.ack(block);
        }
        if let Some((OP_DATA, payload)) = second.receive() {
            got_b.extend_from_slice(&payload[2..]);
            let block = u16::from_be_bytes([payload[0], payload[1]]);
            second.ack(block);
        }
    }
    assert_eq!(got_a, a, "the first client got its own file");
    assert_eq!(got_b, b, "and the second got its own");
}

#[test]
fn a_rom_that_retransmits_its_request_is_not_locked_out() {
    // **This is the "works by hand, never after a reboot" failure.** A PXE ROM that
    // does not hear an answer quickly — a NAS with a sleeping disk is enough — sends
    // its read request again, and again. Each retransmission is a fresh transfer from
    // the same address, so a per-peer cap set to the three or four a suspicious mind
    // suggests would shut the machine out of the very server it was retrying to reach.
    let s = Server::start(&[("ipxe.kpxe", loader(100))]);

    let mut clients: Vec<Client> = Vec::new();
    for _ in 0..6 {
        let mut client = s.client();
        client.read("ipxe.kpxe", &[]);
        clients.push(client);
    }
    for (i, client) in clients.iter_mut().enumerate() {
        let (opcode, _) = client
            .receive()
            .unwrap_or_else(|| panic!("retransmission {i} went unanswered:\n{}", s.log()));
        assert_eq!(opcode, OP_DATA, "retransmission {i}");
    }
}

#[test]
fn malformed_packets_do_not_use_up_a_peers_transfers() {
    // A slot is for a transfer, not for a datagram. Counting junk meant a handful of
    // stray packets locked an address out — and in a lab everything arrives from one.
    let s = Server::start(&[("ipxe.kpxe", loader(100))]);
    {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind");
        for _ in 0..20 {
            let _ = socket.send_to(&[0u8, 9, b'x', 0], &s.tftp_addr);
        }
    }
    assert!(
        s.client().fetch("ipxe.kpxe", &[]).is_ok(),
        "twenty junk packets must not cost a transfer:\n{}",
        s.log()
    );
}

// ---- the allowlist --------------------------------------------------------

#[test]
fn an_allowlist_refuses_a_peer_outside_it() {
    let s = Server::start_env(
        &[("ipxe.kpxe", loader(100))],
        &[("RESCRIPTUM_BOOT_ALLOW", "10.99.0.0/16")],
    );
    // Refused with silence rather than an error: an unauthenticated UDP service that
    // answers strangers at all is an amplifier.
    assert!(s.client().fetch("ipxe.kpxe", &[]).is_err());

    let s = Server::start_env(
        &[("ipxe.kpxe", loader(100))],
        &[("RESCRIPTUM_BOOT_ALLOW", "127.0.0.0/8")],
    );
    assert!(s.client().fetch("ipxe.kpxe", &[]).is_ok());
}

// ---- configuration --------------------------------------------------------

#[test]
fn an_address_with_no_boot_directory_is_refused_at_startup() {
    let out = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
        .arg("check")
        .env("RESCRIPTUM_TFTP_ADDR", "127.0.0.1:6969")
        .env_remove("RESCRIPTUM_BOOT_DIR")
        .output()
        .expect("run");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("RESCRIPTUM_BOOT_DIR"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// **A TFTP port that cannot be bound must not take the answer endpoint down with it.**
///
/// This is the one listener in the server whose failed bind is not fatal, and the reason
/// is a measurement rather than a preference: port 69 is privileged, so it is the only
/// bind that can fail for something nobody configured. On DSM the capability is granted
/// by a `setcap` outside the package and **an upgrade replaces the binary and drops it**;
/// when that was fatal the whole package went to `start_failed`, which failed every
/// install in flight to report that a second port could not be opened.
///
/// The bind is made to fail deterministically by holding the address first — no
/// privileges involved, so this proves the same thing whether or not CI runs as root.
/// What is asserted is all three halves of the decision: the server lives, it says so,
/// and `boot check` still calls it a problem.
#[test]
fn a_tftp_port_that_cannot_be_bound_does_not_take_the_answers_down() {
    let squatter = UdpSocket::bind("127.0.0.1:0").expect("hold the port first");
    let taken = squatter.local_addr().expect("addr").to_string();

    let base = std::env::temp_dir().join(format!("rescriptum-tftp-busy-{}", std::process::id()));
    let boot_dir = base.join("boot");
    let answers_dir = base.join("answers");
    fs::create_dir_all(&boot_dir).expect("boot dir");
    fs::create_dir_all(&answers_dir).expect("answers dir");
    // **Every loader the table names, not just one.** With any of them missing,
    // `boot check` exits non-zero for that instead and the assertion below passes
    // without proving anything — which is what happened the first time this was
    // written, and is the exact shape CLAUDE.md warns about.
    for name in rescriptum::boot::loaders::loaders() {
        fs::write(boot_dir.join(name), loader(100)).expect("loader");
    }
    common::seed(&answers_dir, "default.toml", "keyboard = \"fr\"\n");

    let mut child = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
        .env("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0")
        .env("RESCRIPTUM_ANSWERS_DIR", &answers_dir)
        .env("RESCRIPTUM_BOOT_DIR", &boot_dir)
        .env("RESCRIPTUM_TFTP_ADDR", &taken)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn server");

    let stderr = child.stderr.take().expect("piped stderr");
    let mut lines = BufReader::new(stderr).lines();
    let mut log = Vec::new();
    let mut answer_addr = None;
    for _ in 0..16 {
        let Some(Ok(line)) = lines.next() else { break };
        let done = line.contains("rescriptum ") && line.contains("listening on");
        if done && let Some(rest) = line.split("listening on ").nth(1) {
            answer_addr = Some(
                rest.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        log.push(line);
        if done {
            break;
        }
    }
    let log = log.join("\n");

    // 1. It said so, and said what it costs. A degraded server that says nothing is the
    //    failure mode this whole decision exists to refuse.
    assert!(
        log.contains("warning: cannot bind TFTP"),
        "no warning about the failed bind; saw:\n{log}"
    );
    assert!(
        log.contains("Answers and media are unaffected"),
        "the warning has to say what still works; saw:\n{log}"
    );

    // 2. It is still serving answers — the product.
    let addr = answer_addr.unwrap_or_else(|| panic!("the server never came up; saw:\n{log}"));
    let body = r#"{"dmi":{"system":{"serial":"unclaimed"}}}"#;
    let mut sock = std::net::TcpStream::connect(&addr).expect("connect to answers");
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    std::io::Write::write_all(
        &mut sock,
        format!(
            "POST /answer HTTP/1.1\r\nHost: nas\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .expect("write");
    let mut response = String::new();
    let _ = std::io::Read::read_to_string(&mut sock, &mut response);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("keyboard"), "{response}");

    // 3. And `boot check` still calls it a problem, because a startup warning scrolls
    //    past and a non-zero exit is what a deploy script and a monitor can see.
    let out = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
        .arg("boot")
        .arg("check")
        .env("RESCRIPTUM_BOOT_DIR", &boot_dir)
        .env("RESCRIPTUM_TFTP_ADDR", &taken)
        .output()
        .expect("run boot check");
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(!out.status.success(), "boot check called it fine: {said}");
    assert!(said.contains("BROKEN nothing answers on"), "{said}");
    assert!(
        said.contains("1 problem(s)"),
        "the TFTP port is the only one: {said}"
    );

    // And the control: the same directory with TFTP turned off is clean. Without this
    // the assertions above could be passing on some unrelated complaint.
    let out = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
        .arg("boot")
        .arg("check")
        .env("RESCRIPTUM_BOOT_DIR", &boot_dir)
        .env("RESCRIPTUM_TFTP_ADDR", "off")
        .output()
        .expect("run boot check");
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{said}");

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_dir_all(&base);
    drop(squatter);
}

/// The other half of the same decision: **when TFTP is healthy, `boot check` has to say
/// so.** Without this the command could report every port as a problem and still look
/// correct, because the failing case above is the only one anybody would notice.
///
/// It also pins the probe's happy path, which nothing else reaches: `Served` means a real
/// read request came back with data.
#[test]
fn boot_check_says_so_when_a_loader_really_is_handed_over() {
    let files: Vec<(&str, Vec<u8>)> = rescriptum::boot::loaders::loaders()
        .iter()
        .map(|name| (*name, loader(2048)))
        .collect();
    let s = Server::start(&files);

    let out = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
        .arg("boot")
        .arg("check")
        .env("RESCRIPTUM_BOOT_DIR", &s.boot_dir)
        .env("RESCRIPTUM_TFTP_ADDR", &s.tftp_addr)
        .output()
        .expect("run boot check");
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{said}");
    assert!(said.contains("handed over"), "{said}");

    // And the third outcome, which `boot check` itself never produces because it only
    // ever asks for a loader that is on disk: a server is there and the file is not.
    // Worth telling apart from silence — one is a misconfigured root, the other is
    // nothing running at all.
    use rescriptum::boot::tftp::{ProbeResult, probe};
    assert_eq!(
        probe(&s.tftp_addr, "nothing-of-the-sort", Duration::from_secs(2)),
        ProbeResult::Refused
    );
}

/// **A stalled transfer must not look like a completed one.**
///
/// The line that reports a transfer used to be written *before the first byte went out*,
/// so `ipxe-x86_64.efi 1164800 bytes` meant "about to send". A machine on a real network
/// fetched a loader, did nothing, and the log said success — which is how an afternoon
/// goes. The size is now reported at the end, with what actually happened.
#[test]
fn a_transfer_that_dies_halfway_is_logged_as_a_failure() {
    // Big enough to need many blocks, so abandoning it lands mid-transfer.
    let s = Server::start(&[("ipxe-x86_64.efi", loader(64 * 1024))]);

    let mut client = s.client();
    client.read("ipxe-x86_64.efi", &[]);
    // Take the first data packet and then stop answering, which is exactly what a ROM
    // does when the block size is too large for the path: it never sees block 2.
    let first = client.receive().expect("the first block");
    assert_eq!(first.0, OP_DATA);
    drop(client);

    // The server retries, gives up, and says so. Six retries at 700ms is the bound.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut log = String::new();
    while std::time::Instant::now() < deadline {
        log = s.log();
        if log.contains("FAILED") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        log.contains("FAILED after"),
        "an abandoned transfer left no trace: {log}"
    );
    assert!(
        log.contains("RESCRIPTUM_TFTP_BLKSIZE"),
        "the line has to name the knob that fixes the commonest cause: {log}"
    );
    assert!(
        !log.contains("tftp: sent ipxe-x86_64.efi"),
        "it also claimed to have sent it: {log}"
    );
}

/// The cap exists to be lowered when a path cannot carry a full-MTU block, so the value
/// a client asks for has to actually be bounded by it.
#[test]
fn the_block_size_can_be_capped_below_what_a_client_asks_for() {
    let s = Server::start_env(
        &[("ipxe.kpxe", loader(8000))],
        &[("RESCRIPTUM_TFTP_BLKSIZE", "512")],
    );
    let mut client = s.client();
    client.read("ipxe.kpxe", &[("blksize", "1468")]);
    let (opcode, payload) = client.receive().expect("an answer");
    assert_eq!(opcode, OP_OACK, "the option must still be negotiated");
    let text = String::from_utf8_lossy(&payload);
    assert!(
        text.contains("512"),
        "asked for 1468 with a cap of 512 and got: {text:?}"
    );
    assert!(!text.contains("1468"), "the cap was ignored: {text:?}");
}

/// **`boot check` must not write alarms into the log it exists to help you read.**
///
/// Its probe wants one block and no more, and a client that simply stops answering is
/// indistinguishable from one whose network broke — so the server retried for four
/// seconds and logged a failed transfer. Running the health check produced a scary
/// `FAILED` line about a transfer nobody wanted finished, in the one file an operator
/// reads to find a real one. The probe says goodbye with an ERROR packet now.
#[test]
fn the_health_probe_leaves_no_failure_in_the_log() {
    let files: Vec<(&str, Vec<u8>)> = rescriptum::boot::loaders::loaders()
        .iter()
        .map(|name| (*name, loader(64 * 1024)))
        .collect();
    let s = Server::start(&files);

    let out = Command::new(env!("CARGO_BIN_EXE_rescriptum"))
        .arg("boot")
        .arg("check")
        .env("RESCRIPTUM_BOOT_DIR", &s.boot_dir)
        .env("RESCRIPTUM_TFTP_ADDR", &s.tftp_addr)
        .output()
        .expect("run boot check");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("handed over"),
        "the probe has to still work: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Long enough that a transfer left hanging would have given up and logged by now.
    std::thread::sleep(Duration::from_secs(6));
    let log = s.log();
    assert!(
        !log.contains("FAILED"),
        "the health check logged a failure of its own: {log}"
    );
    // It is still recorded, because a transfer that happened and stopped is worth one
    // line — just not an alarm.
    assert!(
        log.contains("cancelled by the client"),
        "the cancel left no trace at all: {log}"
    );
}

/// **A request that arrived must leave a trace, whatever happens next.**
///
/// Reporting the transfer at its end — right on its own — meant a request that never got
/// going logged nothing at all, which is exactly the case somebody is trying to diagnose.
/// It cost an evening on a real NAS: the machine said it was downloading, the server said
/// nothing, and both were telling the truth.
#[test]
fn a_request_is_logged_when_it_arrives_not_only_when_it_finishes() {
    let s = Server::start(&[("ipxe.kpxe", loader(32 * 1024))]);
    let mut client = s.client();
    client.read("ipxe.kpxe", &[("blksize", "1468")]);
    let _ = client.receive();
    // Deliberately never acknowledge, and look *before* the retries could have expired.
    std::thread::sleep(Duration::from_millis(400));
    let log = s.log();
    assert!(
        log.contains("ipxe.kpxe requested"),
        "a request that has not finished yet is invisible: {log}"
    );
}

/// **A client is allowed to hate the answer, and that must be visible.**
///
/// RFC 2348 lets a server reply with a smaller block size than was asked for, and the
/// client is meant to accept it — but a PXE ROM that wanted 1468 and is offered 512 often
/// just stops. That path used to return without a word, so capping the block size to help
/// one network broke a machine that had been booting fine, invisibly.
#[test]
fn a_refused_option_handshake_says_so_and_names_the_cap() {
    let s = Server::start_env(
        &[("ipxe.kpxe", loader(32 * 1024))],
        &[("RESCRIPTUM_TFTP_BLKSIZE", "512")],
    );
    let mut client = s.client();
    client.read("ipxe.kpxe", &[("blksize", "1468")]);
    let (opcode, _) = client.receive().expect("an option reply");
    assert_eq!(opcode, OP_OACK);
    // A ROM that will not take the smaller size simply stops here.
    drop(client);

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut log = String::new();
    while std::time::Instant::now() < deadline {
        log = s.log();
        if log.contains("option handshake") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        log.contains("FAILED at the option handshake"),
        "the handshake failed in silence: {log}"
    );
    assert!(
        log.contains("blksize=1468") && log.contains("512"),
        "the line has to name both sides of the disagreement: {log}"
    );
    assert!(
        log.contains("RESCRIPTUM_TFTP_BLKSIZE"),
        "and the setting that caused it: {log}"
    );
}

/// **A message must not suggest a cause it has already ruled out.**
///
/// When the server agrees to exactly what the client asked for and the client still walks
/// away, the block-size cap is provably not involved — it did nothing. Naming it anyway
/// printed "asked for 1468 and would not take 1468", which sent two people after the wrong
/// setting for an hour while the real cause (the reply comes from a fresh port, so a
/// firewall between the two eats the acknowledgement) went unexamined.
#[test]
fn a_handshake_failure_blames_the_cap_only_when_the_cap_did_something() {
    // Nothing capped: the server grants what was asked, and the client still stops.
    let s = Server::start(&[("ipxe.kpxe", loader(32 * 1024))]);
    let mut client = s.client();
    client.read("ipxe.kpxe", &[("blksize", "1468")]);
    let _ = client.receive();
    drop(client);

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut log = String::new();
    while std::time::Instant::now() < deadline {
        log = s.log();
        if log.contains("option handshake") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(log.contains("FAILED at the option handshake"), "{log}");
    assert!(
        !log.contains("RESCRIPTUM_TFTP_BLKSIZE"),
        "it blamed a cap that did nothing: {log}"
    );
    assert!(
        log.contains("fresh port"),
        "and it has to point at what this actually looks like: {log}"
    );
    // **Which options were offered is the whole diagnosis.** A ROM that refuses one
    // refuses the whole reply and retries without it; without this line that is only
    // visible in a packet capture, which is where it was actually found.
    assert!(
        log.contains("The reply offered: blksize=1468"),
        "the line has to name what was offered: {log}"
    );
}

/// **A client that answers from a different port must still be served.**
///
/// The data socket used to be `connect`ed to the address the request came from, so the
/// kernel dropped anything from another port *before* this code could see it — and a
/// transfer with a client like that died looking exactly like a firewall eating the
/// acknowledgements. RFC 1350 says a client keeps its TID and most do; the ones that do
/// not are UEFI ROMs, which is the population being served here.
#[test]
fn a_client_that_acknowledges_from_another_port_is_still_served() {
    let s = Server::start(&[("ipxe.kpxe", loader(3000))]);

    // The request comes from one socket…
    let asker = UdpSocket::bind("127.0.0.1:0").expect("bind");
    asker
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut packet = vec![0, 1];
    packet.extend_from_slice(b"ipxe.kpxe\0octet\0");
    asker.send_to(&packet, &s.tftp_addr).expect("send");

    let mut buffer = vec![0u8; 2048];
    let (n, server) = asker.recv_from(&mut buffer).expect("first block");
    assert_eq!(u16::from_be_bytes([buffer[0], buffer[1]]), OP_DATA);
    let mut got = buffer[4..n].to_vec();

    // …and every acknowledgement from a *different* one, which is what the fix is for.
    let other = UdpSocket::bind("127.0.0.1:0").expect("bind");
    other
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut block = 1u16;
    loop {
        let mut ack = vec![0, 4];
        ack.extend_from_slice(&block.to_be_bytes());
        other.send_to(&ack, server).expect("ack");
        if got.len() % 512 != 0 || got.is_empty() {
            break;
        }
        let (n, _) = other.recv_from(&mut buffer).expect("next block");
        block = u16::from_be_bytes([buffer[2], buffer[3]]);
        got.extend_from_slice(&buffer[4..n]);
        if n - 4 < 512 {
            let mut ack = vec![0, 4];
            ack.extend_from_slice(&block.to_be_bytes());
            other.send_to(&ack, server).expect("final ack");
            break;
        }
    }

    assert_eq!(got.len(), 3000, "the whole file has to arrive");
    assert_eq!(got, loader(3000));

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut log = String::new();
    while std::time::Instant::now() < deadline {
        log = s.log();
        if log.contains("sent ipxe.kpxe") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(log.contains("sent ipxe.kpxe 3000 bytes"), "{log}");
    // And it says the client moved, because that is worth knowing about a ROM.
    assert!(log.contains("the client moved to port"), "{log}");
}

/// **Never agree to an option that is not implemented.**
///
/// `windowsize` used to be echoed back — the server saying "yes, four blocks per
/// acknowledgement" — while the transfer loop sent one block and waited. A client told
/// four waits for four, so both sides waited, and only the 700 ms retransmit broke the
/// deadlock. Every block then cost a resend and 700 ms: 1.1 MB takes nine minutes that
/// way, and firmware gives up long before. Found on the wire, from a capture on the NAS,
/// after several wrong guesses about firewalls and block sizes.
///
/// RFC 2347 says an option left out of the OACK is to be treated as never requested, so
/// declining costs a client one acknowledgement per block and nothing else.
#[test]
fn windowsize_is_declined_rather_than_agreed_to_and_ignored() {
    let s = Server::start(&[("ipxe.kpxe", loader(8000))]);
    let mut client = s.client();
    // Exactly what a UEFI ROM sends: tsize, blksize and windowsize together.
    client.read(
        "ipxe.kpxe",
        &[("tsize", "0"), ("blksize", "1468"), ("windowsize", "4")],
    );
    let (opcode, payload) = client.receive().expect("an option reply");
    assert_eq!(opcode, OP_OACK);
    let text = String::from_utf8_lossy(&payload);
    assert!(
        !text.contains("windowsize"),
        "agreed to a window it does not implement: {text:?}"
    );
    // The options that *are* implemented still come back, so declining one is not
    // declining all of them.
    assert!(
        text.contains("blksize") && text.contains("1468"),
        "{text:?}"
    );
    assert!(text.contains("tsize"), "{text:?}");
}

/// And the whole file still arrives, one acknowledgement per block, without a single
/// retransmission — which is what nine minutes versus a second comes down to.
#[test]
fn a_rom_that_asks_for_a_window_still_gets_its_file_promptly() {
    let s = Server::start(&[("ipxe.kpxe", loader(64 * 1024))]);

    // **The client has to behave like the ROM it is standing in for**, or this proves
    // nothing: the ordinary test client acknowledges every block whatever was negotiated,
    // so the deadlock cannot happen and the test passes with the bug reintroduced. Which
    // it did, first time round. This one honours the window it was granted — waiting for
    // that many blocks before acknowledging, exactly as RFC 7440 says a client should.
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind");
    sock.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut request = vec![0, 1];
    // Built field by field rather than as one byte string: `\\00` reads as an
    // octal escape, which in a protocol test is exactly the ambiguity to keep out.
    for field in [
        "ipxe.kpxe",
        "octet",
        "tsize",
        "0",
        "blksize",
        "1468",
        "windowsize",
        "4",
    ] {
        request.extend_from_slice(field.as_bytes());
        request.push(0);
    }
    let started = std::time::Instant::now();
    sock.send_to(&request, &s.tftp_addr).expect("send");

    let mut buffer = vec![0u8; 4096];
    let (n, server) = sock.recv_from(&mut buffer).expect("a reply");
    let mut window = 1usize;
    if u16::from_be_bytes([buffer[0], buffer[1]]) == OP_OACK {
        let text = String::from_utf8_lossy(&buffer[2..n]).to_string();
        let parts: Vec<&str> = text.split('\0').collect();
        for pair in parts.windows(2) {
            if pair[0].eq_ignore_ascii_case("windowsize") {
                window = pair[1].parse().unwrap_or(1);
            }
        }
        sock.send_to(&[0, 4, 0, 0], server)
            .expect("ack the options");
    }

    let mut got = Vec::new();
    let mut since_ack = 0usize;
    loop {
        let Ok((n, _)) = sock.recv_from(&mut buffer) else {
            break;
        };
        if u16::from_be_bytes([buffer[0], buffer[1]]) != OP_DATA {
            break;
        }
        let last = u16::from_be_bytes([buffer[2], buffer[3]]);
        got.extend_from_slice(&buffer[4..n]);
        since_ack += 1;
        let short = n - 4 < 1468;
        if since_ack >= window || short {
            let mut ack = vec![0, 4];
            ack.extend_from_slice(&last.to_be_bytes());
            sock.send_to(&ack, server).expect("ack");
            since_ack = 0;
        }
        if short {
            break;
        }
    }
    assert_eq!(got.len(), 64 * 1024, "the whole file has to arrive");
    assert_eq!(got, loader(64 * 1024));
    // 45 blocks at one retransmit each would be 31 seconds. Anything near that is the
    // deadlock back.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?} — that is the retransmit deadlock, not a transfer",
        started.elapsed()
    );
}
