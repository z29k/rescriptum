//! The media listener: images, kernels and initrds, over HTTP, on its own socket.
//!
//! **Its own socket is forced rather than preferred, three times over:**
//!
//! 1. The answer endpoint answers on *any* path — the URL is baked into an ISO and must
//!    never be wrong. A `/media/…` namespace would carve a reserved prefix out of a
//!    space that is deliberately unreserved.
//! 2. `RESCRIPTUM_TIMEOUT_SECS` is a whole-connection deadline of ten seconds. A 1.5 GB
//!    transfer is fifteen seconds on gigabit and two minutes on 100 Mbit, so on the
//!    answer listener **every image download would be killed mid-transfer** — and it
//!    would look like a flaky network rather than a setting.
//! 3. `RESCRIPTUM_MAX_CONNECTIONS` is a semaphore of in-flight connections, and a
//!    download holds its permit for minutes. Shared budgets mean a rollout starves its
//!    own answer requests.
//!
//! `admin.rs` already has its own listener for its own reasons. Same shape.
//!
//! Everything here is **read-only**: no `PUT`, no `DELETE`, no upload. Writing media is
//! an admin-side act, and it is a command rather than a request — no request may ever
//! trigger work proportional to the size of an image.

use super::catalog::{Catalog, Entry};
use super::{cpio, iso};
use crate::config::Config;
use crate::log;
use hyper::body::{Bytes, Frame, Incoming, SizeHint};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::convert::Infallible;
use std::io::{self, Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc};

/// Bytes read and sent at a time. Sixteen concurrent transfers at this size, with one
/// chunk queued behind the one being written, cost about two megabytes of buffers —
/// which is the arithmetic that has to hold on a 512 MB NAS.
const CHUNK: usize = 64 * 1024;

pub struct Media {
    pub cfg: Arc<Config>,
    pub catalog: Arc<Catalog>,
}

pub async fn serve(listener: TcpListener, media: Arc<Media>) {
    let timeout = media.cfg.media_timeout;
    // Its own budget, deliberately small and deliberately not shared with answers.
    let permits = Arc::new(Semaphore::new(media.cfg.media_max_connections));

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                log::server(&format!("media: accept failed: {e}"));
                continue;
            }
        };

        // Over the cap, say so and close rather than queueing: a client waiting behind
        // fifteen image downloads has no way to tell that from a dead server.
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            log::request(&peer.to_string(), 503, "media: 503 — at max transfers");
            drop(stream);
            continue;
        };

        let media = Arc::clone(&media);
        tokio::spawn(async move {
            let _permit = permit;
            let service = service_fn(move |req| {
                let media = Arc::clone(&media);
                async move { Ok::<_, Infallible>(handle(req, media, peer).await) }
            });
            let serving = http1::Builder::new()
                .timer(TokioTimer::new())
                // The header-read timeout stays short — that is the slowloris guard, and
                // it has nothing to do with how long a transfer may take.
                .header_read_timeout(Some(std::time::Duration::from_secs(10)))
                .serve_connection(TokioIo::new(stream), service);
            let _ = tokio::time::timeout(timeout, serving).await;
        });
    }
}

async fn handle(req: Request<Incoming>, media: Arc<Media>, peer: SocketAddr) -> Response<Body> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let peer_label = peer.to_string();

    if method != Method::GET && method != Method::HEAD {
        // Read-only, and the header says which two verbs that means.
        log::request(&peer_label, 405, &format!("media: {method} {path} 405"));
        return Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .header("Allow", "GET, HEAD")
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(Body::once(Bytes::from("405 Method Not Allowed\n")))
            .expect("a static response always builds");
    }

    // The allowlist, when there is one. Boot traffic is unauthenticated by necessity —
    // a PXE ROM has no credentials — so the controls that exist are structural, and
    // this is the only one that can say "not you".
    if !allowed(&media.cfg, peer) {
        log::request(&peer_label, 403, &format!("media: {method} {path} 403"));
        return text(StatusCode::FORBIDDEN, "403 Forbidden\n");
    }

    if path == "/health" {
        log::request(&peer_label, 200, "media: GET /health 200");
        return text(StatusCode::OK, "OK\n");
    }
    if path == "/" || path.is_empty() {
        let json = req
            .headers()
            .get(hyper::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("application/json"));
        return catalogue(&media, &peer_label, json).await;
    }

    // The two generated scripts. Both live here rather than in the answer set because
    // **they have to work when the answer set is empty**, which is the state every new
    // install starts in.
    if path == "/ipxe/bootstrap" {
        let script = super::menu::bootstrap(&media.cfg.endpoints());
        log::request(&peer_label, 200, "media: GET /ipxe/bootstrap 200");
        return script_response(script);
    }
    if path == "/ipxe/menu" {
        let catalog = Arc::clone(&media.catalog);
        let listing = match tokio::task::spawn_blocking(move || catalog.listing()).await {
            Ok(Ok(listing)) => listing,
            _ => {
                log::request(&peer_label, 500, "media: GET /ipxe/menu 500");
                return text(StatusCode::INTERNAL_SERVER_ERROR, "500\n");
            }
        };
        let style = super::menu::Style {
            title: media
                .cfg
                .boot_title
                .clone()
                .unwrap_or_else(super::menu::Style::default_title),
            timeout_millis: media.cfg.boot_timeout_millis(),
        };
        let script = super::menu::menu(&listing, &media.cfg.endpoints(), &style);
        log::request(
            &peer_label,
            200,
            &format!(
                "media: GET /ipxe/menu 200 entries={}",
                listing.entries.len()
            ),
        );
        return script_response(script);
    }

    // The TFTP root over HTTP: the loaders (UEFI HTTP Boot fetches them here rather
    // than over TFTP), the logo, and anything else `boot sync` put there.
    if let Some(name) = path.strip_prefix("/boot/") {
        return boot_asset(&media, name, &peer_label, method == Method::HEAD).await;
    }

    let Some((id, what)) = route(&path) else {
        log::request(
            &peer_label,
            404,
            &format!("media: GET {path} 404 no such route"),
        );
        return text(StatusCode::NOT_FOUND, "404 Not Found\n");
    };

    // The identifier goes through the same guard the admin API and both stores use, and
    // then **into the catalogue**. The filesystem path always comes from the entry,
    // never from the request: the path-traversal guard survives in letter and spirit.
    if !crate::store::valid_id(&id) {
        log::request(&peer_label, 404, &format!("media: GET {path} 404 bad id"));
        return text(StatusCode::NOT_FOUND, "404 Not Found\n");
    }

    let catalog = Arc::clone(&media.catalog);
    let wanted = what.clone();
    let looked_up = tokio::task::spawn_blocking(move || {
        // `read_dir`, `open` and `seek` are all blocking, and blocking an async worker
        // stalls every other transfer that thread is driving. On a NAS with a sleeping
        // disk that is not theoretical.
        let entry = catalog.get(&id)?;
        let Some(entry) = entry else {
            return Ok(None);
        };
        Ok::<_, io::Error>(Some((resolve(&entry, &wanted), entry)))
    })
    .await;

    let (source, entry) = match looked_up {
        Ok(Ok(Some((source, entry)))) => (source, entry),
        Ok(Ok(None)) => {
            log::request(&peer_label, 404, &format!("media: GET {path} 404 no entry"));
            return text(StatusCode::NOT_FOUND, "404 Not Found\n");
        }
        Ok(Err(e)) => {
            log::request(&peer_label, 500, &format!("media: GET {path} 500 {e}"));
            return text(StatusCode::INTERNAL_SERVER_ERROR, "500\n");
        }
        // A panic in the blocking task cannot take the server with it, and must not pass
        // silently either.
        Err(e) => {
            log::request(
                &peer_label,
                500,
                &format!("media: GET {path} 500 lookup panicked: {e}"),
            );
            return text(StatusCode::INTERNAL_SERVER_ERROR, "500\n");
        }
    };

    let source = match source {
        Ok(source) => source,
        Err(why) => {
            log::request(&peer_label, 404, &format!("media: GET {path} 404 {why}"));
            return text(StatusCode::NOT_FOUND, format!("404 Not Found — {why}\n"));
        }
    };

    send(
        req,
        source,
        &entry,
        &peer_label,
        &path,
        method == Method::HEAD,
    )
}

/// `/<id>/<what>` and `/<id>/file/<path>`.
fn route(path: &str) -> Option<(String, What)> {
    let trimmed = path.trim_start_matches('/');
    let (id, rest) = trimmed.split_once('/')?;
    if id.is_empty() {
        return None;
    }
    let what = match rest {
        "iso" | "img" => What::Image,
        "kernel" => What::Kernel,
        "initrd" => What::Initrd,
        // `+` in a path is a literal plus; only a query string reads it as a space.
        "initrd+iso" => What::InitrdIso,
        other => match other.strip_prefix("file/") {
            Some(inside) if !inside.is_empty() => What::Inside(inside.to_string()),
            _ => return None,
        },
    };
    Some((id.to_string(), what))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum What {
    Image,
    Kernel,
    Initrd,
    InitrdIso,
    Inside(String),
}

/// One run of bytes to send: a file, or a generated header.
#[derive(Debug, Clone)]
enum Segment {
    Bytes(Vec<u8>),
    File {
        path: PathBuf,
        offset: u64,
        length: u64,
    },
}

impl Segment {
    fn len(&self) -> u64 {
        match self {
            Segment::Bytes(b) => b.len() as u64,
            Segment::File { length, .. } => *length,
        }
    }
}

/// What to send, and whether a range may be applied to it.
struct Source {
    segments: Vec<Segment>,
    total: u64,
    /// A single contiguous file can be resumed; a synthesised archive cannot.
    resumable: bool,
    content_type: &'static str,
}

/// Turn a request for part of an entry into the bytes that answer it.
///
/// **The image is never modified and never copied.** A kernel is an offset and a length
/// inside the ISO, because a file in an ISO9660 image is one contiguous extent — so this
/// is a seek, not an extraction.
fn resolve(entry: &Entry, what: &What) -> Result<Source, String> {
    let one = |path: PathBuf, offset: u64, length: u64, resumable: bool| Source {
        segments: vec![Segment::File {
            path,
            offset,
            length,
        }],
        total: length,
        resumable,
        content_type: "application/octet-stream",
    };

    match what {
        What::Image => Ok(one(entry.path.clone(), 0, entry.size, true)),

        What::Kernel | What::Initrd => {
            let inside = match what {
                What::Kernel => entry.probed.kernel.as_deref(),
                _ => entry.probed.initrd.as_deref(),
            };
            let Some(inside) = inside else {
                return Err(format!(
                    "{} has no {} — no probe row places one in it",
                    entry.id,
                    if matches!(what, What::Kernel) {
                        "kernel"
                    } else {
                        "initrd"
                    }
                ));
            };
            // `prepare-iso --pxe` leaves the kernel and initrd *beside* the trimmed
            // image rather than inside it, and that directory is a normal thing to
            // point a media directory at.
            if entry.probed.external {
                let beside = entry
                    .beside
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(inside);
                let size = std::fs::metadata(&beside)
                    .map(|m| m.len())
                    .map_err(|e| format!("{}: {e}", beside.display()))?;
                return Ok(one(beside, 0, size, true));
            }
            let extent = extent_of(entry, inside)?;
            Ok(one(entry.path.clone(), extent.offset, extent.size, true))
        }

        What::Inside(path) => {
            let extent = extent_of(entry, path)?;
            Ok(one(entry.path.clone(), extent.offset, extent.size, true))
        }

        // The initrd, then a cpio header naming `proxmox.iso`, then the image, then the
        // padding and the trailer. Synthesised on the wire rather than stored: a 1.5 GB
        // second copy on disk buys nothing, and the arithmetic for `Content-Length` is
        // exact. `200` only — nothing resumes an initrd.
        What::InitrdIso => {
            let initrd = resolve(entry, &What::Initrd)?;
            let member = cpio::member("proxmox.iso", entry.size, 1)?;
            let mut segments = initrd.segments;
            segments.push(Segment::Bytes(member.prefix.clone()));
            segments.push(Segment::File {
                path: entry.path.clone(),
                offset: 0,
                length: entry.size,
            });
            if member.padding > 0 {
                segments.push(Segment::Bytes(vec![0u8; member.padding]));
            }
            segments.push(Segment::Bytes(cpio::trailer()));
            let total = segments.iter().map(Segment::len).sum();
            Ok(Source {
                segments,
                total,
                resumable: false,
                content_type: "application/octet-stream",
            })
        }
    }
}

fn extent_of(entry: &Entry, inside: &str) -> Result<iso::Extent, String> {
    let mut image =
        iso::Iso::open(&entry.path).map_err(|e| format!("{}: {e}", entry.path.display()))?;
    match image.locate(inside) {
        Ok(Some(extent)) if !extent.directory => Ok(extent),
        Ok(_) => Err(format!("{inside} is not a file in {}", entry.id)),
        Err(e) => Err(format!("{}: {e}", entry.path.display())),
    }
}

/// Build the response, ranges and validators included.
fn send(
    req: Request<Incoming>,
    source: Source,
    entry: &Entry,
    peer: &str,
    path: &str,
    head_only: bool,
) -> Response<Body> {
    // A strong validator either way. The digest when somebody pinned the image; size
    // and mtime otherwise, so **a resumed transfer that raced a replacement restarts
    // instead of splicing two images together** even for an image nobody pinned.
    let etag = match &entry.digest {
        Some(digest) => format!("\"{digest}\""),
        None => format!("\"{}-{}\"", entry.size, mtime_token(entry)),
    };

    let header = |name: hyper::header::HeaderName| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };

    // A resumed transfer names the validator it started with. If it no longer matches,
    // the honest answer is the whole entity rather than a splice.
    let raced = header(hyper::header::IF_RANGE).is_some_and(|v| v.trim() != etag);
    let wanted = if source.resumable && !raced {
        header(hyper::header::RANGE)
            .as_deref()
            .map_or(Wanted::Whole, |r| parse_range(r, source.total))
    } else {
        Wanted::Whole
    };

    let mut builder = Response::builder()
        .header("Content-Type", source.content_type)
        .header("ETag", etag);
    if source.resumable {
        builder = builder.header("Accept-Ranges", "bytes");
    } else {
        // Say so rather than letting a client discover it by having a range ignored.
        builder = builder.header("Accept-Ranges", "none");
    }

    let (status, segments, length) = match wanted {
        Wanted::Whole => (StatusCode::OK, source.segments, source.total),
        Wanted::Part(start, end) => {
            builder = builder.header(
                "Content-Range",
                format!("bytes {start}-{end}/{}", source.total),
            );
            (
                StatusCode::PARTIAL_CONTENT,
                slice(source.segments, start, end),
                end - start + 1,
            )
        }
        Wanted::Unsatisfiable => {
            log::request(peer, 416, &format!("media: GET {path} 416"));
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header("Content-Range", format!("bytes */{}", source.total))
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(Body::once(Bytes::from("416 Range Not Satisfiable\n")))
                .expect("a static response always builds");
        }
    };

    log::request(
        peer,
        status.as_u16(),
        &format!(
            "media: {} {path} {} bytes={length}",
            if head_only { "HEAD" } else { "GET" },
            status.as_u16()
        ),
    );

    // `HEAD` is answered because UEFI HTTP Boot asks before it fetches: same headers,
    // same `Content-Length`, no body.
    let body = if head_only {
        Body::empty()
    } else {
        Body::stream(segments, length)
    };
    builder
        .status(status)
        .header("Content-Length", length.to_string())
        .body(body)
        .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "500\n"))
}

/// Narrow a plan to the requested byte range. Only ever called on a single-file plan,
/// which is what `resumable` guarantees.
fn slice(segments: Vec<Segment>, start: u64, end: u64) -> Vec<Segment> {
    segments
        .into_iter()
        .map(|segment| match segment {
            Segment::File { path, offset, .. } => Segment::File {
                path,
                offset: offset + start,
                length: end - start + 1,
            },
            other => other,
        })
        .collect()
}

/// A tiebreaker for the fallback validator: the image's mtime, in nanoseconds.
fn mtime_token(entry: &Entry) -> u64 {
    std::fs::metadata(&entry.path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[derive(Debug, PartialEq, Eq)]
enum Wanted {
    Whole,
    /// Inclusive, as `Content-Range` writes it.
    Part(u64, u64),
    Unsatisfiable,
}

/// One `bytes=` range, in the three forms that exist.
///
/// **A multi-range request is answered `200` with the whole entity.** That is permitted,
/// and far better than half-implementing `multipart/byteranges` for a client that does
/// not exist: five of the seven installers range-fetch, and not one of them asks for
/// more than one range at a time.
fn parse_range(header: &str, total: u64) -> Wanted {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return Wanted::Whole;
    };
    if spec.contains(',') {
        return Wanted::Whole;
    }
    if total == 0 {
        return Wanted::Unsatisfiable;
    }

    let Some((first, last)) = spec.split_once('-') else {
        return Wanted::Whole;
    };
    let (first, last) = (first.trim(), last.trim());

    match (first.is_empty(), last.is_empty()) {
        // `-n`: the final n bytes.
        (true, false) => match last.parse::<u64>() {
            Ok(0) => Wanted::Unsatisfiable,
            Ok(n) => Wanted::Part(total.saturating_sub(n), total - 1),
            Err(_) => Wanted::Whole,
        },
        // `a-`: from a to the end.
        (false, true) => match first.parse::<u64>() {
            Ok(start) if start < total => Wanted::Part(start, total - 1),
            Ok(_) => Wanted::Unsatisfiable,
            Err(_) => Wanted::Whole,
        },
        // `a-b`, with b clamped: a client may ask past the end and expects the rest.
        (false, false) => match (first.parse::<u64>(), last.parse::<u64>()) {
            (Ok(start), Ok(end)) if start <= end && start < total => {
                Wanted::Part(start, end.min(total - 1))
            }
            (Ok(_), Ok(_)) => Wanted::Unsatisfiable,
            _ => Wanted::Whole,
        },
        (true, true) => Wanted::Whole,
    }
}

async fn catalogue(media: &Media, peer: &str, json: bool) -> Response<Body> {
    let catalog = Arc::clone(&media.catalog);
    let listing = match tokio::task::spawn_blocking(move || catalog.listing()).await {
        Ok(Ok(listing)) => listing,
        _ => {
            log::request(peer, 500, "media: GET / 500");
            return text(StatusCode::INTERNAL_SERVER_ERROR, "500\n");
        }
    };

    log::request(
        peer,
        200,
        &format!("media: GET / 200 entries={}", listing.entries.len()),
    );

    if json {
        let rows: Vec<serde_json::Value> = listing
            .entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "family": e.family().label(),
                    "version": e.probed.version,
                    "arch": e.arch().map(|a| a.label()),
                    "size": e.size,
                    "sha256": e.digest,
                    "bootable": e.bootable(),
                })
            })
            .collect();
        let body = serde_json::json!({ "media": rows, "problems": listing.problems }).to_string();
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len().to_string())
            .body(Body::once(Bytes::from(body)))
            .expect("a built response");
    }

    let mut out = String::new();
    for entry in &listing.entries {
        out.push_str(&format!(
            "{:<20} {:<8} {:<24} {:>10}  {}\n",
            entry.id,
            entry.family().label(),
            entry.describe(),
            entry.size,
            if entry.bootable() {
                "bootable"
            } else {
                "image only"
            },
        ));
    }
    if listing.entries.is_empty() {
        out.push_str("no images\n");
    }
    for problem in &listing.problems {
        out.push_str(&format!("problem: {problem}\n"));
    }
    text(StatusCode::OK, out)
}

/// A generated iPXE script. `text/plain` because that is what iPXE reads, and no
/// caching: the menu is a rendering of a catalogue that changes when an ISO is dropped
/// in, and a cached one would show yesterday's images.
fn script_response(script: String) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=us-ascii")
        .header("Cache-Control", "no-store")
        .header("Content-Length", script.len().to_string())
        .body(Body::once(Bytes::from(script)))
        .expect("a built response")
}

/// A file from the boot directory, over HTTP.
///
/// **UEFI HTTP Boot fetches its loader here rather than over TFTP**, which is the
/// shortest chain there is — option 60 `HTTPClient` plus a URL in 67, and no TFTP at
/// all. The same files, the same directory, a faster transport.
async fn boot_asset(media: &Media, name: &str, peer: &str, head_only: bool) -> Response<Body> {
    let Some(root) = media.cfg.boot_dir.clone() else {
        log::request(
            peer,
            404,
            &format!("media: GET /boot/{name} 404 no boot directory"),
        );
        return text(
            StatusCode::NOT_FOUND,
            "404 Not Found — RESCRIPTUM_BOOT_DIR is not set\n",
        );
    };

    let wanted = name.to_string();
    let found = tokio::task::spawn_blocking(move || {
        // The same containment rule TFTP uses, for the same reason and by the same
        // means: strip anything that could climb, then check the resolved path is still
        // inside the canonicalised root. A symlink out of the tree fails the second
        // check even though it passes the first.
        let root = root.canonicalize().ok()?;
        let mut path = root.clone();
        for segment in wanted.split('/') {
            if segment.is_empty() || segment == "." {
                continue;
            }
            if segment == ".." {
                return None;
            }
            path.push(segment);
        }
        let resolved = path.canonicalize().ok()?;
        if !resolved.starts_with(&root) || !resolved.is_file() {
            return None;
        }
        let size = std::fs::metadata(&resolved).ok()?.len();
        Some((resolved, size))
    })
    .await;

    let Ok(Some((path, size))) = found else {
        log::request(peer, 404, &format!("media: GET /boot/{name} 404"));
        return text(StatusCode::NOT_FOUND, "404 Not Found\n");
    };

    log::request(
        peer,
        200,
        &format!("media: GET /boot/{name} 200 bytes={size}"),
    );
    let body = if head_only {
        Body::empty()
    } else {
        Body::stream(
            vec![Segment::File {
                path,
                offset: 0,
                length: size,
            }],
            size,
        )
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", size.to_string())
        .header("Accept-Ranges", "none")
        .body(body)
        .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "500\n"))
}

/// Whether a peer is inside `RESCRIPTUM_BOOT_ALLOW`, which is a comma-separated list of
/// CIDRs. Unset means anyone who can reach the port — which on a boot VLAN is the honest
/// configuration, and the documentation says so.
fn allowed(cfg: &Config, peer: SocketAddr) -> bool {
    let Some(list) = &cfg.boot_allow else {
        return true;
    };
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|cidr| in_cidr(peer.ip(), cidr))
}

pub(crate) fn in_cidr(address: std::net::IpAddr, cidr: &str) -> bool {
    let (network, bits) = match cidr.split_once('/') {
        Some((network, bits)) => match bits.parse::<u32>() {
            Ok(bits) => (network, bits),
            Err(_) => return false,
        },
        // A bare address is that address, which is the same as a full-length prefix.
        None => (
            cidr,
            match address {
                std::net::IpAddr::V4(_) => 32,
                std::net::IpAddr::V6(_) => 128,
            },
        ),
    };
    let Ok(network) = network.parse::<std::net::IpAddr>() else {
        return false;
    };

    // Compare the leading `bits` of both addresses. Mixing families never matches, which
    // is right: a v4 CIDR says nothing about a v6 peer.
    let (a, b) = match (address, network) {
        (std::net::IpAddr::V4(a), std::net::IpAddr::V4(b)) => {
            (a.octets().to_vec(), b.octets().to_vec())
        }
        (std::net::IpAddr::V6(a), std::net::IpAddr::V6(b)) => {
            (a.octets().to_vec(), b.octets().to_vec())
        }
        _ => return false,
    };
    if bits as usize > a.len() * 8 {
        return false;
    }
    let whole = (bits / 8) as usize;
    if a[..whole] != b[..whole] {
        return false;
    }
    let leftover = bits % 8;
    if leftover == 0 {
        return true;
    }
    let mask = 0xffu8 << (8 - leftover);
    a[whole] & mask == b[whole] & mask
}

// ---- the body ------------------------------------------------------------

/// **Streaming, never buffering.** A response is a list of runs of bytes, produced by a
/// blocking task and handed over a channel one chunk at a time, so sixteen concurrent
/// transfers cost megabytes rather than sixteen images.
pub struct Body {
    inner: Inner,
    remaining: u64,
}

enum Inner {
    Once(Option<Bytes>),
    Stream(mpsc::Receiver<Result<Bytes, io::Error>>),
}

impl Body {
    fn once(bytes: Bytes) -> Body {
        Body {
            remaining: bytes.len() as u64,
            inner: Inner::Once(Some(bytes)),
        }
    }

    fn empty() -> Body {
        Body {
            remaining: 0,
            inner: Inner::Once(None),
        }
    }

    fn stream(segments: Vec<Segment>, total: u64) -> Body {
        // One chunk queued behind the one being written: enough to keep the socket fed,
        // little enough that the arithmetic above stays true.
        let (tx, rx) = mpsc::channel(1);
        tokio::task::spawn_blocking(move || produce(segments, &tx));
        Body {
            remaining: total,
            inner: Inner::Stream(rx),
        }
    }
}

/// The blocking half: open, seek, read, send. A closed channel means the client hung up
/// — stop reading rather than finishing an image nobody is receiving.
fn produce(segments: Vec<Segment>, tx: &mpsc::Sender<Result<Bytes, io::Error>>) {
    for segment in segments {
        match segment {
            Segment::Bytes(bytes) => {
                if tx.blocking_send(Ok(Bytes::from(bytes))).is_err() {
                    return;
                }
            }
            Segment::File {
                path,
                offset,
                length,
            } => {
                let mut file = match std::fs::File::open(&path) {
                    Ok(file) => file,
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        return;
                    }
                };
                if let Err(e) = file.seek(SeekFrom::Start(offset)) {
                    let _ = tx.blocking_send(Err(e));
                    return;
                }
                let mut left = length;
                let mut buffer = vec![0u8; CHUNK];
                while left > 0 {
                    let want = (left as usize).min(CHUNK);
                    match file.read(&mut buffer[..want]) {
                        // Short of what the catalogue promised: the file shrank under
                        // us. Ending the body early is what the client will notice
                        // against `Content-Length`, which is the honest signal.
                        Ok(0) => return,
                        Ok(n) => {
                            if tx
                                .blocking_send(Ok(Bytes::copy_from_slice(&buffer[..n])))
                                .is_err()
                            {
                                return;
                            }
                            left -= n as u64;
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            let _ = tx.blocking_send(Err(e));
                            return;
                        }
                    }
                }
            }
        }
    }
}

impl hyper::body::Body for Body {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        let this = self.get_mut();
        match &mut this.inner {
            Inner::Once(slot) => match slot.take() {
                Some(bytes) => {
                    this.remaining = 0;
                    Poll::Ready(Some(Ok(Frame::data(bytes))))
                }
                None => Poll::Ready(None),
            },
            Inner::Stream(rx) => match rx.poll_recv(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    this.remaining = this.remaining.saturating_sub(bytes.len() as u64);
                    Poll::Ready(Some(Ok(Frame::data(bytes))))
                }
                Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining)
    }
}

fn text(status: StatusCode, body: impl Into<Bytes>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(Body::once(body.into()))
        .expect("a static response always builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_range_forms_are_read_correctly() {
        assert_eq!(parse_range("bytes=0-99", 1000), Wanted::Part(0, 99));
        assert_eq!(parse_range("bytes=500-", 1000), Wanted::Part(500, 999));
        assert_eq!(parse_range("bytes=-100", 1000), Wanted::Part(900, 999));
        // A client may ask past the end and expects the rest, not a refusal.
        assert_eq!(parse_range("bytes=990-2000", 1000), Wanted::Part(990, 999));
    }

    #[test]
    fn a_range_that_starts_past_the_end_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=1000-", 1000), Wanted::Unsatisfiable);
        assert_eq!(parse_range("bytes=2000-3000", 1000), Wanted::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 1000), Wanted::Unsatisfiable);
        // Nothing can be satisfied out of an empty entity.
        assert_eq!(parse_range("bytes=0-0", 0), Wanted::Unsatisfiable);
    }

    #[test]
    fn a_multi_range_request_is_answered_whole() {
        // Permitted, and far better than half-implementing multipart/byteranges for a
        // client that does not exist.
        assert_eq!(parse_range("bytes=0-99,200-299", 1000), Wanted::Whole);
    }

    #[test]
    fn something_that_is_not_a_byte_range_is_ignored() {
        assert_eq!(parse_range("items=0-99", 1000), Wanted::Whole);
        assert_eq!(parse_range("bytes=abc-def", 1000), Wanted::Whole);
        assert_eq!(parse_range("bytes=", 1000), Wanted::Whole);
    }

    #[test]
    fn the_routes_are_the_ones_documented() {
        assert_eq!(
            route("/pve-8.4/iso"),
            Some(("pve-8.4".to_string(), What::Image))
        );
        assert_eq!(
            route("/pve-8.4/kernel"),
            Some(("pve-8.4".to_string(), What::Kernel))
        );
        assert_eq!(
            route("/pve-8.4/initrd"),
            Some(("pve-8.4".to_string(), What::Initrd))
        );
        // `+` in a path is a literal plus; only a query string reads it as a space.
        assert_eq!(
            route("/pve-8.4/initrd+iso"),
            Some(("pve-8.4".to_string(), What::InitrdIso))
        );
        assert_eq!(
            route("/pve-8.4/file/boot/linux26"),
            Some((
                "pve-8.4".to_string(),
                What::Inside("boot/linux26".to_string())
            ))
        );
        assert_eq!(route("/pve-8.4"), None);
        assert_eq!(route("/pve-8.4/nonsense"), None);
        assert_eq!(route("/pve-8.4/file/"), None);
    }

    #[test]
    fn a_cidr_allowlist_admits_and_refuses_by_prefix() {
        let matches = |peer: &str, cidr: &str| in_cidr(peer.parse().expect("address"), cidr);
        assert!(matches("10.0.0.5", "10.0.0.0/8"));
        assert!(matches("10.0.0.5", "10.0.0.0/24"));
        assert!(!matches("10.0.1.5", "10.0.0.0/24"));
        assert!(matches("192.168.1.130", "192.168.1.128/25"));
        assert!(!matches("192.168.1.127", "192.168.1.128/25"));
        // A bare address is itself.
        assert!(matches("10.0.0.5", "10.0.0.5"));
        assert!(!matches("10.0.0.6", "10.0.0.5"));
        // Everything matches /0, which is the "why bother" case, and it must still work.
        assert!(matches("203.0.113.9", "0.0.0.0/0"));
        // A v4 CIDR says nothing about a v6 peer.
        assert!(!matches("::1", "10.0.0.0/8"));
        assert!(matches("2001:db8::1", "2001:db8::/32"));
        assert!(!matches("2001:db9::1", "2001:db8::/32"));
        // Nonsense refuses rather than admits: a typo must not open the door.
        assert!(!matches("10.0.0.5", "not-a-network/8"));
        assert!(!matches("10.0.0.5", "10.0.0.0/wide"));
        assert!(!matches("10.0.0.5", "10.0.0.0/99"));
    }

    #[test]
    fn an_unset_allowlist_admits_everyone() {
        let cfg = Config::from_lookup(|_| None);
        assert!(allowed(&cfg, "203.0.113.9:1234".parse().expect("peer")));
    }

    #[test]
    fn a_set_allowlist_refuses_everyone_else() {
        let cfg = Config::from_lookup(|key| {
            (key == "RESCRIPTUM_BOOT_ALLOW").then(|| "10.0.0.0/8, 192.168.0.0/16".to_string())
        });
        assert!(allowed(&cfg, "10.1.2.3:1".parse().expect("peer")));
        assert!(allowed(&cfg, "192.168.5.5:1".parse().expect("peer")));
        assert!(!allowed(&cfg, "203.0.113.9:1".parse().expect("peer")));
    }

    #[test]
    fn a_range_narrows_the_plan_rather_than_re_reading_it() {
        let segments = vec![Segment::File {
            path: PathBuf::from("/srv/media/x.iso"),
            offset: 4096,
            length: 1000,
        }];
        let narrowed = slice(segments, 100, 199);
        match &narrowed[0] {
            Segment::File { offset, length, .. } => {
                // The offset moves *within the extent*, which is what makes a range over
                // a file inside an image work at all.
                assert_eq!(*offset, 4196);
                assert_eq!(*length, 100);
            }
            other => panic!("{other:?}"),
        }
    }
}
