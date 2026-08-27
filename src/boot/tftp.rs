//! TFTP, read-only, for exactly one job: handing over the loader.
//!
//! **It is core rather than optional.** An appliance that needs somebody else's TFTP
//! server is not an appliance — that was the reasoning the first draft of the plan got
//! backwards, and correcting it is what makes the whole chain start from one binary.
//!
//! ## One file, and then HTTP
//!
//! At `blksize=1468` with lockstep acknowledgement, TFTP moves one block per
//! round-trip — call it 1.4 MB/s at a millisecond of RTT. The loader is about a
//! megabyte, so two seconds. A 1.5 GB image would be **the better part of twenty
//! minutes**, against fifteen seconds over HTTP on the same wire. Hence the rule,
//! written here and in the guide:
//!
//! > **TFTP hands over the loader. Everything after that is HTTP.**
//!
//! Nothing here will serve an image, and the root is the boot-asset directory rather
//! than the media directory precisely so that it cannot.
//!
//! ## What real ROMs need
//!
//! RFC 1350 alone is not enough. The options are what decide whether firmware actually
//! works: `blksize` (RFC 2348) because 512-byte blocks make a megabyte take 2,000
//! round-trips, `tsize` (RFC 2349) because a number of ROMs will not proceed without
//! being told the size up front, `timeout` (RFC 2349), and `windowsize` (RFC 7440) —
//! offered only when asked, because some ROMs get it wrong.
//!
//! ## UDP is forgeable, so this is defensive by construction
//!
//! One connected socket per transfer, so a transfer only ever hears from its own peer;
//! no reply to a broadcast or multicast destination, which is amplification hygiene
//! rather than politeness; a cap on concurrent transfers and a cap per peer; and a
//! duplicate acknowledgement is **ignored, never answered** — that is the Sorcerer's
//! Apprentice bug, and answering doubles the traffic for as long as it lasts.

use crate::config::Config;
use crate::log;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::Semaphore;

const OP_RRQ: u16 = 1;
const OP_WRQ: u16 = 2;
const OP_DATA: u16 = 3;
const OP_ACK: u16 = 4;
const OP_ERROR: u16 = 5;
const OP_OACK: u16 = 6;

/// Error codes worth sending. The rest of RFC 1350's list describes writes.
const ERR_NOT_FOUND: u16 = 1;
const ERR_ACCESS: u16 = 2;
const ERR_ILLEGAL: u16 = 4;
const ERR_NO_USER: u16 = 7;

/// RFC 1350's block size, and the floor every implementation understands.
const DEFAULT_BLOCK: usize = 512;
/// **Clamped so a data packet still fits one Ethernet frame.** 1500 minus 20 bytes of
/// IP and 8 of UDP leaves 1472; minus TFTP's own 4-byte header, 1468. Larger merely
/// invites fragmentation, and a fragmented TFTP transfer to a PXE ROM is a coin toss.
const MAX_BLOCK: usize = 1468;
/// A request larger than this is not a request.
const MAX_REQUEST: usize = 1024;
/// How long to wait for an acknowledgement before sending the block again.
const RETRY: Duration = Duration::from_millis(700);
/// Give up on a peer that has stopped acknowledging.
const MAX_RETRIES: u32 = 6;
/// A transfer that has run this long is not a loader fetch any more.
const MAX_TRANSFER: Duration = Duration::from_secs(60);
/// In-flight transfers, in total and per peer.
///
/// **The per-peer figure is not a hostility threshold, and treating it as one is a bug
/// that only shows up at a reboot.** A PXE ROM that does not hear an answer quickly
/// *retransmits its read request* — a sleeping disk on a NAS is enough to cause it —
/// and each retransmission is a fresh transfer from the same address. Set this to the
/// three or four a suspicious mind suggests and a slow first fetch locks the machine
/// out of the boot server it was retrying to reach.
const MAX_TRANSFERS: usize = 64;
const MAX_PER_PEER: usize = 8;
/// Windowing is offered when asked for, and capped: a ROM that asks for 64 and then
/// mishandles the window turns one lost packet into a stall.
const MAX_WINDOW: u16 = 8;

pub struct Tftp {
    /// The boot-asset directory, canonicalised at start. Every request resolves inside
    /// it or is refused.
    root: PathBuf,
    cfg: Arc<Config>,
}

impl Tftp {
    /// Canonicalise the root now, so the containment check below compares two resolved
    /// paths rather than two hopeful strings.
    pub fn new(root: &Path, cfg: Arc<Config>) -> io::Result<Tftp> {
        let root = root.canonicalize().map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("cannot use {} as the TFTP root: {e}", root.display()),
            )
        })?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a directory", root.display()),
            ));
        }
        Ok(Tftp { root, cfg })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Accept requests forever. Each one is answered from its **own ephemeral socket**,
/// connected to the peer — which is both what RFC 1350 requires and what stops a
/// transfer hearing from anybody else.
pub async fn serve(socket: UdpSocket, tftp: Arc<Tftp>) {
    let permits = Arc::new(Semaphore::new(MAX_TRANSFERS));
    let per_peer: Arc<std::sync::Mutex<std::collections::HashMap<IpAddr, usize>>> =
        Arc::new(Default::default());
    let mut buffer = vec![0u8; MAX_REQUEST];

    loop {
        let (n, peer) = match socket.recv_from(&mut buffer).await {
            Ok(pair) => pair,
            Err(e) => {
                log::server(&format!("tftp: recv failed: {e}"));
                continue;
            }
        };

        // **Never answer a broadcast or multicast destination.** Amplification hygiene:
        // a forged request naming a broadcast address would have us shout a megabyte at
        // a whole segment. Nothing legitimate asks for a loader that way.
        if is_broadcastish(peer.ip()) {
            log::request(&peer.to_string(), 0, "tftp: ignored, not a unicast peer");
            continue;
        }
        if !allowed(&tftp.cfg, peer) {
            log::request(
                &peer.to_string(),
                403,
                "tftp: refused, outside the allowlist",
            );
            continue;
        }

        let request = buffer[..n].to_vec();
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            log::request(&peer.to_string(), 503, "tftp: at max transfers");
            continue;
        };
        // A per-peer cap on top of the global one, so one machine cannot use the whole
        // budget — the global cap alone protects the server, not the other clients.
        //
        // **Only a read request takes a slot.** Anything else — junk, a write request,
        // a mode we refuse — is answered with a fixed-size error and costs nothing, so
        // a burst of malformed packets cannot shut a peer out of the transfers it is
        // entitled to. Counting them was a real defect: four stray datagrams from an
        // address locked that address out, and everything in a lab arrives from one.
        let is_read = request.len() >= 2 && u16::from_be_bytes([request[0], request[1]]) == OP_RRQ;
        if is_read {
            let mut counts = per_peer.lock().unwrap_or_else(|e| e.into_inner());
            let count = counts.entry(peer.ip()).or_insert(0);
            if *count >= MAX_PER_PEER {
                log::request(
                    &peer.to_string(),
                    503,
                    "tftp: at max transfers for this peer",
                );
                continue;
            }
            *count += 1;
        }

        let tftp = Arc::clone(&tftp);
        let per_peer = Arc::clone(&per_peer);
        tokio::spawn(async move {
            let _permit = permit;
            // A transfer that outlives this deadline is not a loader fetch any more.
            let _ = tokio::time::timeout(MAX_TRANSFER, transfer(&request, peer, &tftp)).await;
            if is_read {
                let mut counts = per_peer.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(count) = counts.get_mut(&peer.ip()) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        // Bounded by construction: an idle peer leaves no entry behind,
                        // so the map cannot itself be turned into a memory leak.
                        counts.remove(&peer.ip());
                    }
                }
            }
        });
    }
}

async fn transfer(request: &[u8], peer: SocketAddr, tftp: &Tftp) {
    // The reply socket is ephemeral and *connected*: RFC 1350 wants the data to come
    // from a fresh port, and connecting means this transfer only ever hears its peer.
    let bind = if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let Ok(socket) = UdpSocket::bind(bind).await else {
        return;
    };
    if socket.connect(peer).await.is_err() {
        return;
    }

    let parsed = match parse_request(request) {
        Ok(parsed) => parsed,
        Err(refusal) => {
            let _ = socket
                .send(&error_packet(refusal.code, &refusal.message))
                .await;
            log::request(&peer.to_string(), 0, &format!("tftp: {}", refusal.message));
            return;
        }
    };

    let path = match resolve(&tftp.root, &parsed.filename) {
        Some(path) => path,
        None => {
            let _ = socket
                .send(&error_packet(ERR_NOT_FOUND, "no such file"))
                .await;
            log::request(
                &peer.to_string(),
                404,
                &format!("tftp: {} not found", parsed.filename),
            );
            return;
        }
    };

    let contents = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = socket
                .send(&error_packet(ERR_NOT_FOUND, "cannot read"))
                .await;
            log::request(
                &peer.to_string(),
                500,
                &format!("tftp: {} cannot be read: {e}", path.display()),
            );
            return;
        }
    };

    // Options are negotiated in one OACK, acknowledged with block 0, before any data.
    let mut block_size = DEFAULT_BLOCK;
    let mut accepted: Vec<(String, String)> = Vec::new();
    for (name, value) in &parsed.options {
        match name.as_str() {
            "blksize" => {
                if let Ok(asked) = value.parse::<usize>() {
                    block_size = asked.clamp(DEFAULT_BLOCK, MAX_BLOCK);
                    accepted.push(("blksize".to_string(), block_size.to_string()));
                }
            }
            // A number of ROMs will not proceed without being told the size up front.
            "tsize" => accepted.push(("tsize".to_string(), contents.len().to_string())),
            "timeout" => {
                if let Ok(seconds) = value.parse::<u32>()
                    && (1..=255).contains(&seconds)
                {
                    accepted.push(("timeout".to_string(), seconds.to_string()));
                }
            }
            "windowsize" => {
                if let Ok(asked) = value.parse::<u16>()
                    && asked >= 1
                {
                    accepted.push(("windowsize".to_string(), asked.min(MAX_WINDOW).to_string()));
                }
            }
            // An option we do not implement is left out of the OACK, which is exactly
            // how RFC 2347 says to decline one.
            _ => {}
        }
    }

    if !accepted.is_empty() {
        let mut oack = vec![0, OP_OACK as u8];
        for (name, value) in &accepted {
            oack.extend_from_slice(name.as_bytes());
            oack.push(0);
            oack.extend_from_slice(value.as_bytes());
            oack.push(0);
        }
        if socket.send(&oack).await.is_err() {
            return;
        }
        // The client acknowledges the option set with block 0 before data starts.
        if !wait_for_ack(&socket, 0, &oack).await {
            return;
        }
    }

    log::request(
        &peer.to_string(),
        200,
        &format!(
            "tftp: {} {} bytes blksize={block_size}",
            parsed.filename,
            contents.len()
        ),
    );

    let mut block: u16 = 1;
    let mut sent = 0usize;
    loop {
        let end = (sent + block_size).min(contents.len());
        let chunk = &contents[sent..end];
        // **A short block is what ends a transfer**, and "short" includes empty. A file
        // whose length is an exact multiple of the block size therefore ends with a
        // data packet carrying nothing — leave it out and the client waits forever for
        // a final block that never comes. An empty file is the same case at zero.
        let last = chunk.len() < block_size;

        let mut packet = Vec::with_capacity(4 + chunk.len());
        packet.extend_from_slice(&OP_DATA.to_be_bytes());
        packet.extend_from_slice(&block.to_be_bytes());
        packet.extend_from_slice(chunk);

        if socket.send(&packet).await.is_err() {
            return;
        }
        if !wait_for_ack(&socket, block, &packet).await {
            return;
        }

        sent = end;
        if last {
            break;
        }
        // Block numbers are 16 bits and wrap. A loader will never reach 65535 at 1468
        // bytes a block; be correct anyway, because "never" is how this kind of bug
        // gets in.
        block = block.wrapping_add(1);
    }
}

/// Wait for the acknowledgement of `block`, resending on silence.
///
/// **A duplicate acknowledgement — one for a block already acknowledged — is ignored,
/// never answered.** That is the Sorcerer's Apprentice bug: answering a duplicate with
/// a duplicate makes both sides echo each other and doubles the traffic for the rest of
/// the transfer.
async fn wait_for_ack(socket: &UdpSocket, block: u16, resend: &[u8]) -> bool {
    let mut buffer = [0u8; 64];
    for _ in 0..MAX_RETRIES {
        match tokio::time::timeout(RETRY, socket.recv(&mut buffer)).await {
            Ok(Ok(n)) if n >= 4 => {
                let opcode = u16::from_be_bytes([buffer[0], buffer[1]]);
                let acked = u16::from_be_bytes([buffer[2], buffer[3]]);
                if opcode == OP_ERROR {
                    return false;
                }
                if opcode == OP_ACK {
                    if acked == block {
                        return true;
                    }
                    // An older block: a duplicate. Say nothing and keep waiting.
                    continue;
                }
                // Anything else on this socket is not part of the conversation.
                continue;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => return false,
            // Silence: the block was lost, or the acknowledgement was. Send it again.
            Err(_) => {
                if socket.send(resend).await.is_err() {
                    return false;
                }
            }
        }
    }
    false
}

struct Request {
    filename: String,
    options: Vec<(String, String)>,
}

struct Refusal {
    code: u16,
    message: String,
}

fn parse_request(bytes: &[u8]) -> Result<Request, Refusal> {
    if bytes.len() < 4 {
        return Err(Refusal {
            code: ERR_ILLEGAL,
            message: "truncated request".to_string(),
        });
    }
    let opcode = u16::from_be_bytes([bytes[0], bytes[1]]);
    if opcode == OP_WRQ {
        // Read-only, and this is the whole enforcement. Writing a loader over UDP with
        // no authentication would be a way to change what every machine boots.
        return Err(Refusal {
            code: ERR_ACCESS,
            message: "this server is read-only".to_string(),
        });
    }
    if opcode != OP_RRQ {
        return Err(Refusal {
            code: ERR_ILLEGAL,
            message: format!("opcode {opcode} is not a read request"),
        });
    }

    let mut fields = bytes[2..].split(|b| *b == 0).map(|f| f.to_vec());
    let filename = fields
        .next()
        .and_then(|f| String::from_utf8(f).ok())
        .filter(|f| !f.is_empty())
        .ok_or_else(|| Refusal {
            code: ERR_ILLEGAL,
            message: "no filename".to_string(),
        })?;
    let mode = fields
        .next()
        .and_then(|f| String::from_utf8(f).ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    // **`netascii` is refused rather than mistranslated.** It rewrites line endings, and
    // a loader is a binary: silently corrupting one produces a machine that fetches
    // something and then does nothing anybody can explain.
    if mode != "octet" {
        return Err(Refusal {
            code: ERR_NO_USER,
            message: format!("mode {mode:?} is not octet; a loader is binary"),
        });
    }

    let mut options = Vec::new();
    loop {
        let (Some(name), Some(value)) = (fields.next(), fields.next()) else {
            break;
        };
        let (Ok(name), Ok(value)) = (String::from_utf8(name), String::from_utf8(value)) else {
            break;
        };
        if name.is_empty() {
            break;
        }
        options.push((name.to_ascii_lowercase(), value));
    }

    Ok(Request { filename, options })
}

/// Resolve a requested name inside the root, or refuse.
///
/// Two guards, and the second is the one that matters: the name is stripped of anything
/// that could climb, **and then the resolved path is checked to still be inside the
/// canonicalised root**. A symlink pointing out of the tree is caught by the second
/// check even though it passes the first.
fn resolve(root: &Path, filename: &str) -> Option<PathBuf> {
    let relative = filename.trim_start_matches(['/', '\\']);
    if relative.is_empty() {
        return None;
    }
    let mut path = root.to_path_buf();
    for segment in relative.split(['/', '\\']) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return None;
        }
        path.push(segment);
    }
    let resolved = path.canonicalize().ok()?;
    resolved
        .starts_with(root)
        .then_some(resolved)
        .filter(|p| p.is_file())
}

fn error_packet(code: u16, message: &str) -> Vec<u8> {
    let mut packet = Vec::with_capacity(5 + message.len());
    packet.extend_from_slice(&OP_ERROR.to_be_bytes());
    packet.extend_from_slice(&code.to_be_bytes());
    packet.extend_from_slice(message.as_bytes());
    packet.push(0);
    packet
}

/// Ask a TFTP server on `addr` for `filename`, and say whether anything served it.
///
/// **This exists because binding is not a health check, and the test that found that out
/// is in `tests/tftp.rs`.** A successful bind means *nothing is listening* — which is
/// exactly the degraded state, not the healthy one — and a failed bind cannot tell our
/// own running server apart from some other daemon squatting port 69. Both are
/// `AddrInUse`. The only question with a real answer is the one a booting machine asks:
/// send a read request, and see whether a loader comes back.
///
/// Synchronous and short on purpose: this is `boot check`'s, not the server's, and a
/// command an operator runs must not hang on a silent port. One request, one wait, and
/// the first reply decides — `DATA` or `OACK` means served, an `ERROR` means a server is
/// there and this file is not, silence means nothing is.
pub fn probe(addr: &str, filename: &str, wait: Duration) -> ProbeResult {
    let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") else {
        return ProbeResult::Silent;
    };
    if socket.set_read_timeout(Some(wait)).is_err() {
        return ProbeResult::Silent;
    }

    let mut packet = Vec::new();
    packet.extend_from_slice(&OP_RRQ.to_be_bytes());
    packet.extend_from_slice(filename.as_bytes());
    packet.push(0);
    packet.extend_from_slice(b"octet\0");
    // No options. A server that negotiates none still has to answer with plain 512-byte
    // blocks, so this is the one request every TFTP server on earth understands.
    if socket.send_to(&packet, addr).is_err() {
        return ProbeResult::Silent;
    }

    let mut buffer = [0u8; 1024];
    let Ok((n, _)) = socket.recv_from(&mut buffer) else {
        return ProbeResult::Silent;
    };
    if n < 2 {
        return ProbeResult::Silent;
    }
    match u16::from_be_bytes([buffer[0], buffer[1]]) {
        OP_DATA | OP_OACK => ProbeResult::Served,
        OP_ERROR => ProbeResult::Refused,
        _ => ProbeResult::Silent,
    }
}

/// What [`probe`] found. Three outcomes because they mean three different things to an
/// operator: a loader was handed over, a TFTP server is there but does not have that
/// file, or nothing answered at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    Served,
    Refused,
    Silent,
}

/// Whether an address is one nothing legitimate asks for a loader from.
fn is_broadcastish(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_broadcast() || v4.is_multicast() || v4.is_unspecified(),
        IpAddr::V6(v6) => v6.is_multicast() || v6.is_unspecified(),
    }
}

fn allowed(cfg: &Config, peer: SocketAddr) -> bool {
    let Some(list) = &cfg.boot_allow else {
        return true;
    };
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|cidr| super::media::in_cidr(peer.ip(), cidr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rrq(filename: &str, mode: &str, options: &[(&str, &str)]) -> Vec<u8> {
        let mut packet = vec![0, OP_RRQ as u8];
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
        packet
    }

    #[test]
    fn a_read_request_is_parsed_with_its_options() {
        let parsed = parse_request(&rrq(
            "ipxe-undionly.kpxe",
            "octet",
            &[("blksize", "1468"), ("tsize", "0")],
        ))
        .unwrap_or_else(|e| panic!("{}", e.message));
        assert_eq!(parsed.filename, "ipxe-undionly.kpxe");
        assert_eq!(
            parsed.options,
            vec![
                ("blksize".to_string(), "1468".to_string()),
                ("tsize".to_string(), "0".to_string())
            ]
        );
    }

    #[test]
    fn a_write_request_is_refused_as_an_access_violation() {
        // Read-only is the whole posture. Writing a loader over unauthenticated UDP
        // would be a way to change what every machine on the segment boots.
        let e = parse_request(
            &rrq("evil.kpxe", "octet", &[])
                .iter()
                .enumerate()
                .map(|(i, b)| if i == 1 { OP_WRQ as u8 } else { *b })
                .collect::<Vec<u8>>(),
        )
        .err()
        .expect("must refuse");
        assert_eq!(e.code, ERR_ACCESS);
        assert!(e.message.contains("read-only"), "{}", e.message);
    }

    #[test]
    fn netascii_is_refused_rather_than_mistranslated() {
        // It rewrites line endings. Silently corrupting a loader produces a machine
        // that fetches something and then does nothing anybody can explain.
        let e = parse_request(&rrq("ipxe.kpxe", "netascii", &[]))
            .err()
            .expect("must refuse");
        assert!(e.message.contains("octet"), "{}", e.message);
    }

    #[test]
    fn a_truncated_or_nonsense_request_is_refused_rather_than_panicking() {
        assert!(parse_request(&[]).is_err());
        assert!(parse_request(&[0, 1]).is_err());
        assert!(parse_request(&[0, 9, b'x', 0, b'o', 0]).is_err());
        // A request with no filename at all.
        assert!(parse_request(&[0, 1, 0, b'o', b'c', b't', b'e', b't', 0]).is_err());
    }

    /// A root with one file in it, plus a nested directory, to resolve against.
    fn root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rescriptum-tftp-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("nested")).expect("temp dir");
        std::fs::write(dir.join("ipxe-undionly.kpxe"), b"loader").expect("write");
        std::fs::write(dir.join("nested/deeper.efi"), b"loader").expect("write");
        dir.canonicalize().expect("canonical")
    }

    #[test]
    fn a_name_resolves_inside_the_root() {
        let root = root();
        assert!(resolve(&root, "ipxe-undionly.kpxe").is_some());
        assert!(
            resolve(&root, "/ipxe-undionly.kpxe").is_some(),
            "a leading slash is fine"
        );
        assert!(resolve(&root, "nested/deeper.efi").is_some());
        assert!(
            resolve(&root, "nested\\deeper.efi").is_some(),
            "some ROMs send backslashes"
        );
    }

    #[test]
    fn nothing_resolves_outside_the_root() {
        let root = root();
        for name in [
            "../../../etc/passwd",
            "nested/../../etc/passwd",
            "/etc/passwd",
            "..",
            "",
        ] {
            assert!(resolve(&root, name).is_none(), "{name} escaped the root");
        }
        // And a directory is not a file to send.
        assert!(resolve(&root, "nested").is_none());
    }

    #[test]
    fn a_broadcast_or_multicast_peer_is_never_answered() {
        // Amplification hygiene rather than politeness: a forged request naming a
        // broadcast address would have us shout a megabyte at a whole segment.
        for ip in ["255.255.255.255", "224.0.0.1", "0.0.0.0"] {
            assert!(is_broadcastish(ip.parse().expect("address")), "{ip}");
        }
        for ip in ["10.0.0.5", "192.168.1.1"] {
            assert!(!is_broadcastish(ip.parse().expect("address")), "{ip}");
        }
        assert!(is_broadcastish("ff02::1".parse().expect("address")));
        assert!(!is_broadcastish("2001:db8::1".parse().expect("address")));
    }

    #[test]
    fn the_block_size_is_clamped_to_one_ethernet_frame() {
        // 1500 − 20 (IP) − 8 (UDP) − 4 (TFTP) = 1468. Larger merely invites
        // fragmentation, and a fragmented transfer to a PXE ROM is a coin toss.
        assert_eq!(MAX_BLOCK, 1468);
        assert_eq!(2048usize.clamp(DEFAULT_BLOCK, MAX_BLOCK), 1468);
        assert_eq!(8usize.clamp(DEFAULT_BLOCK, MAX_BLOCK), 512);
        assert_eq!(1024usize.clamp(DEFAULT_BLOCK, MAX_BLOCK), 1024);
    }

    #[test]
    fn a_root_that_is_not_a_directory_is_refused_at_startup() {
        let cfg = Arc::new(Config::from_lookup(|_| None));
        assert!(Tftp::new(Path::new("/nonexistent/rescriptum/boot"), Arc::clone(&cfg)).is_err());
    }
}
