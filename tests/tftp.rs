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
