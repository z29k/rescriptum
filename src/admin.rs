//! The administration API: read and change the answer set over HTTP.
//!
//! Three things about it are deliberate.
//!
//! **It listens somewhere else.** The answer endpoint cannot authenticate — the Proxmox
//! installer has no credentials to offer — so it is open by necessity. This API decides
//! the root password and SSH keys of every machine installed afterwards, so it never
//! shares that listener. Different port, bearer token, off unless configured.
//!
//! **It only runs over SQLite.** With a directory of files there would be two ways to
//! change the same configuration, by hand and over the wire, racing each other.
//!
//! **It refuses to leave the store broken.** Every write is checked afterwards; if it
//! introduced a problem that would make installs fail, it is rolled back and the caller
//! is told what they broke, rather than finding out at 3am.

use crate::config::Config;
use crate::facts::Facts;
use crate::guard;
use crate::log;
use crate::select::Answers;
use crate::store::{StoreWrite, valid_format, valid_id};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

/// An answer document larger than this is a mistake, not a configuration.
const MAX_DOCUMENT: usize = 256 * 1024;

type Body = Full<Bytes>;

/// Failed attempts allowed from one address before it is shut out.
const MAX_FAILURES: u32 = 5;
/// Failures older than this stop counting, so an occasional typo never accumulates
/// into a lockout.
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// First lockout. Doubles with each repeat, up to `MAX_BLOCK`.
const BASE_BLOCK: Duration = Duration::from_secs(60);
const MAX_BLOCK: Duration = Duration::from_secs(900);
/// Cap on tracked addresses, so the guard cannot itself be turned into a memory leak.
const MAX_TRACKED: usize = 4096;

#[derive(Default)]
struct Failures {
    count: u32,
    window_started: Option<Instant>,
    blocked_until: Option<Instant>,
    /// How many times this address has been blocked — the backoff exponent.
    blocks: u32,
}

/// Slows down guessing at the token.
///
/// A blocked address is refused outright, *including* when it finally presents the right
/// token — otherwise the block would not slow an attacker down at all. The cost is that
/// someone sharing your source address can lock you out for a minute; on a management
/// network that is the right side of the trade.
///
/// Per-address, so an attacker with many addresses is not stopped by this. It raises the
/// cost of guessing; the token's length is what makes guessing hopeless.
#[derive(Default)]
pub struct AuthGuard {
    entries: Mutex<HashMap<IpAddr, Failures>>,
}

impl AuthGuard {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<IpAddr, Failures>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How much longer this address is shut out, if it is.
    fn blocked(&self, ip: IpAddr) -> Option<Duration> {
        let now = Instant::now();
        let entries = self.lock();
        entries
            .get(&ip)
            .and_then(|f| f.blocked_until)
            .filter(|until| *until > now)
            .map(|until| until - now)
    }

    /// Record a rejected attempt. Returns the lockout it just earned, if any.
    fn failure(&self, ip: IpAddr) -> Option<Duration> {
        let now = Instant::now();
        let mut entries = self.lock();

        // Drop entries that have expired before considering growth.
        if entries.len() >= MAX_TRACKED {
            entries.retain(|_, f| {
                f.blocked_until.is_some_and(|u| u > now)
                    || f.window_started.is_some_and(|w| now - w < FAILURE_WINDOW)
            });
            // Still full: refuse to grow further rather than track unboundedly.
            if entries.len() >= MAX_TRACKED && !entries.contains_key(&ip) {
                return None;
            }
        }

        let entry = entries.entry(ip).or_default();
        match entry.window_started {
            Some(started) if now - started < FAILURE_WINDOW => entry.count += 1,
            // Outside the window: this is the start of a fresh run of failures.
            _ => {
                entry.window_started = Some(now);
                entry.count = 1;
            }
        }

        if entry.count >= MAX_FAILURES {
            let block = BASE_BLOCK
                .saturating_mul(1u32 << entry.blocks.min(4))
                .min(MAX_BLOCK);
            entry.blocks = entry.blocks.saturating_add(1);
            entry.count = 0;
            entry.window_started = None;
            entry.blocked_until = Some(now + block);
            return Some(block);
        }
        None
    }

    /// A good token clears the run of failures — but not an active block.
    fn success(&self, ip: IpAddr) {
        let mut entries = self.lock();
        if let Some(entry) = entries.get_mut(&ip) {
            entry.count = 0;
            entry.window_started = None;
            if entry.blocked_until.is_none_or(|u| u <= Instant::now()) {
                entries.remove(&ip);
            }
        }
    }
}

pub struct Admin {
    pub cfg: Arc<Config>,
    pub store: Arc<dyn StoreWrite>,
    pub answers: Arc<Answers>,
    pub guard: AuthGuard,
}

/// Run the admin listener until the process ends.
pub async fn serve(listener: TcpListener, admin: Arc<Admin>) {
    let timeout = admin.cfg.timeout;
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                log::server(&format!("admin: accept failed: {e}"));
                continue;
            }
        };
        let admin = Arc::clone(&admin);
        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let admin = Arc::clone(&admin);
                async move { Ok::<_, Infallible>(handle(req, admin, peer).await) }
            });
            let serving = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(Some(timeout))
                .serve_connection(TokioIo::new(stream), service);
            let _ = tokio::time::timeout(timeout, serving).await;
        });
    }
}

/// Compare without an early return, so a wrong token cannot be found byte by byte.
/// The length is not hidden; the secret is.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn authorized(req: &Request<Incoming>, cfg: &Config) -> bool {
    let Some(expected) = &cfg.admin_token else {
        return false; // `validate` refuses to start in this state; belt and braces.
    };
    let Some(header) = req.headers().get(hyper::header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = header.to_str() else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(token.trim().as_bytes(), expected.as_bytes())
}

fn text(status: StatusCode, body: impl Into<Bytes>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Connection", "close")
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(Full::new(body.into()))
        .expect("a static response always builds")
}

/// A stored document, labelled with the content type its format deserves.
fn document_body(format: &str, body: String) -> Response<Body> {
    let content_type = crate::format::Kind::for_extension(format)
        .map(|k| k.content_type())
        .unwrap_or("text/plain; charset=utf-8");
    Response::builder()
        .status(StatusCode::OK)
        .header("Connection", "close")
        .header("Content-Type", content_type)
        .header("X-Answer-Format", format.to_string())
        .body(Full::new(Bytes::from(body)))
        .expect("a static response always builds")
}

fn json(status: StatusCode, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Connection", "close")
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .expect("a static response always builds")
}

/// Minimal JSON string escaping — enough for identifiers and error messages, which is
/// all this API emits. Not a general encoder.
pub(crate) fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn json_list(key: &str, items: &[String]) -> String {
    let body: Vec<String> = items.iter().map(|s| json_string(s)).collect();
    format!("{{{}:[{}]}}", json_string(key), body.join(","))
}

async fn handle(
    req: Request<Incoming>,
    admin: Arc<Admin>,
    peer_addr: SocketAddr,
) -> Response<Body> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let peer = peer_addr.to_string();
    let ip = peer_addr.ip();

    // Unauthenticated liveness, so a monitor does not need the token.
    if method == Method::GET && path == "/health" {
        return text(StatusCode::OK, "OK\n");
    }

    if let Some(remaining) = admin.guard.blocked(ip) {
        let secs = remaining.as_secs().max(1);
        log::request(
            &peer,
            429,
            &format!("admin {method} {path} 429 blocked for {secs}s"),
        );
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("Retry-After", secs.to_string())
            .header("Connection", "close")
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(Full::new(Bytes::from("429 Too Many Requests\n")))
            .expect("a static response always builds");
    }

    if !authorized(&req, &admin.cfg) {
        // Worth a log line each time: this is what a token being guessed looks like.
        match admin.guard.failure(ip) {
            Some(block) => log::server(&format!(
                "admin: {ip} failed authentication {MAX_FAILURES} times — blocked for {}s",
                block.as_secs()
            )),
            None => log::request(&peer, 401, &format!("admin {method} {path} 401")),
        }
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("WWW-Authenticate", "Bearer")
            .header("Connection", "close")
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(Full::new(Bytes::from("401 Unauthorized\n")))
            .expect("a static response always builds");
    }
    admin.guard.success(ip);

    let query = req.uri().query().map(str::to_string);
    let query = query.as_deref();
    // The format a written document is in, for PUT. Defaults to TOML, which is what
    // Proxmox speaks and what this server started life serving.
    let put_format = query
        .and_then(|q| {
            q.split('&')
                .find_map(|pair| pair.strip_prefix("format=").map(str::to_ascii_lowercase))
        })
        .unwrap_or_else(|| "toml".to_string());

    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let response = match (&method, segments.as_slice()) {
        // **Exactly one new endpoint, and it serves the CLI's model byte for byte.**
        // `GET /machines` returns bare identifiers, so a remote fleet view would need one
        // `GET /resolve/{id}` per machine — two thousand round trips on the fleet this
        // project measures itself against, which is not "the same screens over the wire"
        // but a different and much worse program. One producer, so the command and the
        // API cannot drift.
        (&Method::GET, ["fleet"]) => match crate::cli::fleet::machines(&admin.answers) {
            Ok(machines) => json(StatusCode::OK, crate::cli::fleet::machines_json(&machines)),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        (&Method::GET, ["machines"]) => list(&admin, Kind::Machine),
        (&Method::GET, ["groups"]) => list(&admin, Kind::Group),
        (&Method::GET, ["check"]) => match admin.answers.problems() {
            Ok(problems) => json(StatusCode::OK, json_list("problems", &problems)),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        (&Method::GET, ["resolve", id]) => resolve(&admin, id, query),
        (&Method::GET, ["resolve"]) => resolve(&admin, "", query),

        (&Method::GET, ["machines", id]) => get_document(&admin, Kind::Machine, id, &put_format),
        (&Method::GET, ["groups", id]) => get_document(&admin, Kind::Group, id, &put_format),
        (&Method::GET, ["default"]) => get_document(&admin, Kind::Default, "", &put_format),

        (&Method::PUT, ["machines", id]) => put(&admin, Kind::Machine, id, &put_format, req).await,
        (&Method::PUT, ["groups", id]) => put(&admin, Kind::Group, id, &put_format, req).await,
        (&Method::PUT, ["default"]) => put(&admin, Kind::Default, "", &put_format, req).await,

        (&Method::DELETE, ["machines", id]) => delete(&admin, Kind::Machine, id, &put_format),
        (&Method::DELETE, ["groups", id]) => delete(&admin, Kind::Group, id, &put_format),
        (&Method::DELETE, ["default"]) => delete(&admin, Kind::Default, "", &put_format),

        _ => error(StatusCode::NOT_FOUND, "no such endpoint"),
    };

    log::request(
        &peer,
        response.status().as_u16(),
        &format!("admin {method} {path} {}", response.status().as_u16()),
    );
    response
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Machine,
    Group,
    Default,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Machine => "machine",
            Kind::Group => "group",
            Kind::Default => "default",
        }
    }
}

fn error(status: StatusCode, message: &str) -> Response<Body> {
    json(
        status,
        format!("{{{}:{}}}", json_string("error"), json_string(message)),
    )
}

fn list(admin: &Admin, kind: Kind) -> Response<Body> {
    let result = match kind {
        Kind::Machine => admin.answers.machine_ids(),
        Kind::Group => admin
            .answers
            .group_names()
            .map(|gs| gs.into_iter().map(|(n, _)| n).collect()),
        Kind::Default => return error(StatusCode::NOT_FOUND, "no such endpoint"),
    };
    match result {
        Ok(items) => json(StatusCode::OK, json_list(kind.label(), &items)),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// The stored document, exactly as written — not the merged result. Use `/resolve` for
/// what a machine would actually receive.
fn get_document(admin: &Admin, kind: Kind, id: &str, format: &str) -> Response<Body> {
    if kind != Kind::Default && !valid_id(id) {
        return error(StatusCode::BAD_REQUEST, "invalid identifier");
    }
    let snapshot = match admin.store.snapshot() {
        Ok(s) => s,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let found = match kind {
        Kind::Machine => snapshot
            .machines
            .iter()
            .find(|m| m.id == id && m.format == format)
            .map(|m| (m.format.clone(), m.body.clone())),
        Kind::Group => snapshot
            .groups
            .iter()
            .find(|g| g.name == id && g.format == format)
            .map(|g| (g.format.clone(), g.body.clone())),
        Kind::Default => snapshot
            .fallbacks
            .iter()
            .find(|d| d.format == format)
            .map(|d| (d.format.clone(), d.body.clone())),
    };
    match found {
        Some((format, body)) => document_body(&format, body),
        None => error(StatusCode::NOT_FOUND, "not found"),
    }
}

/// What a machine would actually receive, merged — the thing worth checking before a
/// rack boots.
fn resolve(admin: &Admin, id: &str, query: Option<&str>) -> Response<Body> {
    // Either an identifier in the path, or the same labels a real request would carry.
    // `?path=` lets an operator rehearse a request to a particular URL — the difference
    // between `/user-data` and `/meta-data`, for instance.
    let facts = match query {
        Some(q) if !q.is_empty() => {
            let path = q.split('&').find_map(|p| p.strip_prefix("path="));
            Facts::from_request(path, Some(q), b"")
        }
        _ => Facts::from_identity(id),
    };
    match admin.answers.resolve(&facts) {
        Ok(Some(resolution)) => Response::builder()
            .status(StatusCode::OK)
            .header("Connection", "close")
            .header("Content-Type", resolution.content_type)
            .header("X-Answer-Source", resolution.how())
            .body(Full::new(Bytes::from(resolution.body)))
            .expect("a static response always builds"),
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "no answer applies to that identifier",
        ),
        Err(e) => error(StatusCode::CONFLICT, &e.to_string()),
    }
}

async fn put(
    admin: &Admin,
    kind: Kind,
    id: &str,
    format: &str,
    req: Request<Incoming>,
) -> Response<Body> {
    // Read the body before judging anything about the request. Answering and closing
    // while the client is still writing earns it a reset instead of the response we
    // took the trouble to write — the body is capped, so draining it costs little.
    let body = match Limited::new(req.into_body(), MAX_DOCUMENT).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "document too large, or unreadable",
            );
        }
    };
    let Ok(body) = String::from_utf8(body.to_vec()) else {
        return error(StatusCode::BAD_REQUEST, "body is not UTF-8");
    };

    if kind != Kind::Default && !valid_id(id) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid identifier: letters, digits, and - _ . : only",
        );
    }

    if !valid_format(format) {
        return error(
            StatusCode::BAD_REQUEST,
            &format!("unknown ?format={format} — see the format list in --help"),
        );
    }
    // Refuse a malformed document here rather than storing something that becomes a 500
    // the next time a machine asks for it.
    let kind_of_format = crate::format::Kind::for_extension(format).expect("checked above");
    if let Err(e) = crate::format::Doc::parse(kind_of_format, &body, "document") {
        return error(StatusCode::BAD_REQUEST, &e);
    }

    guarded(admin, kind, id, format, Some(&body))
}

fn delete(admin: &Admin, kind: Kind, id: &str, format: &str) -> Response<Body> {
    if kind != Kind::Default && !valid_id(id) {
        return error(StatusCode::BAD_REQUEST, "invalid identifier");
    }
    guarded(admin, kind, id, format, None)
}

/// Apply a write through the guard, and turn what it did into a response.
///
/// **This is a mapping and nothing else.** The rule — apply, re-read, roll back what
/// broke — lives in `crate::guard`, because it is not an HTTP property and more than one
/// caller needs it. What belongs here is the choice of status code and envelope, which is
/// this API's business alone.
fn guarded(
    admin: &Admin,
    kind: Kind,
    id: &str,
    format: &str,
    body: Option<&str>,
) -> Response<Body> {
    let target = match kind {
        Kind::Machine => guard::Target::Machine(id.to_string()),
        Kind::Group => guard::Target::Group(id.to_string()),
        Kind::Default => guard::Target::Default,
    };

    match guard::write(&admin.answers, admin.store.as_ref(), &target, format, body) {
        guard::Outcome::Unavailable(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e),
        guard::Outcome::Rejected(e) => error(StatusCode::BAD_REQUEST, &e),
        guard::Outcome::NotFound => error(StatusCode::NOT_FOUND, "not found"),
        guard::Outcome::Refused { introduced } => json(
            StatusCode::CONFLICT,
            format!(
                "{{{}:{},{}}}",
                json_string("error"),
                json_string("refused: this would break the answer set (rolled back)"),
                json_list("problems", &introduced)
                    .trim_start_matches('{')
                    .trim_end_matches('}')
            ),
        ),
        // Anything still broken is reported on a success too, so a caller is not misled
        // into thinking all is well just because their own write was clean.
        guard::Outcome::Stored { problems } => stored(StatusCode::OK, "stored", &problems),
        guard::Outcome::Deleted { problems } => stored(StatusCode::OK, "deleted", &problems),
    }
}

fn stored(status: StatusCode, what: &str, problems: &[String]) -> Response<Body> {
    json(
        status,
        format!(
            "{{{}:{},{}}}",
            json_string("status"),
            json_string(what),
            json_list("problems", problems)
                .trim_start_matches('{')
                .trim_end_matches('}')
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, n])
    }

    #[test]
    fn a_constant_time_comparison_still_compares_correctly() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secrets"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
        // The whole point: a token differing only in its last byte must still fail.
        assert!(!constant_time_eq(b"0123456789abcdef", b"0123456789abcdeX"));
    }

    #[test]
    fn failures_below_the_threshold_do_not_block() {
        let guard = AuthGuard::default();
        for _ in 0..MAX_FAILURES - 1 {
            assert!(guard.failure(ip(1)).is_none());
        }
        assert!(guard.blocked(ip(1)).is_none());
    }

    #[test]
    fn enough_failures_earn_a_block() {
        let guard = AuthGuard::default();
        let mut blocked = None;
        for _ in 0..MAX_FAILURES {
            blocked = guard.failure(ip(1));
        }
        assert_eq!(blocked, Some(BASE_BLOCK));
        assert!(guard.blocked(ip(1)).is_some());
    }

    #[test]
    fn a_block_applies_only_to_the_address_that_earned_it() {
        let guard = AuthGuard::default();
        for _ in 0..MAX_FAILURES {
            guard.failure(ip(1));
        }
        assert!(guard.blocked(ip(1)).is_some());
        assert!(
            guard.blocked(ip(2)).is_none(),
            "an innocent address must be unaffected"
        );
    }

    #[test]
    fn a_success_clears_the_run_of_failures() {
        // An occasional typo must never accumulate into a lockout.
        let guard = AuthGuard::default();
        for _ in 0..MAX_FAILURES - 1 {
            guard.failure(ip(1));
        }
        guard.success(ip(1));
        for _ in 0..MAX_FAILURES - 1 {
            assert!(
                guard.failure(ip(1)).is_none(),
                "the counter should have reset"
            );
        }
        assert!(guard.blocked(ip(1)).is_none());
    }

    #[test]
    fn a_success_does_not_lift_an_active_block() {
        // Otherwise guessing until you get it right would cost nothing.
        let guard = AuthGuard::default();
        for _ in 0..MAX_FAILURES {
            guard.failure(ip(1));
        }
        guard.success(ip(1));
        assert!(
            guard.blocked(ip(1)).is_some(),
            "the block must survive a correct token"
        );
    }

    #[test]
    fn repeated_blocks_back_off_exponentially() {
        let guard = AuthGuard::default();
        let mut seen = Vec::new();
        for _ in 0..4 {
            let mut last = None;
            for _ in 0..MAX_FAILURES {
                last = guard.failure(ip(1));
            }
            seen.push(last.expect("each round should block"));
        }
        assert_eq!(seen[0], BASE_BLOCK);
        assert!(seen[1] > seen[0], "{seen:?}");
        assert!(seen[2] > seen[1], "{seen:?}");
        assert!(seen.iter().all(|d| *d <= MAX_BLOCK), "{seen:?}");
    }

    #[test]
    fn the_guard_cannot_be_grown_without_bound() {
        // Tracking attackers must not itself become the attack.
        let guard = AuthGuard::default();
        for n in 0..(MAX_TRACKED + 500) {
            guard.failure(IpAddr::from((n as u32).to_be_bytes()));
        }
        let tracked = guard.lock().len();
        assert!(tracked <= MAX_TRACKED, "tracked {tracked} addresses");
    }

    #[test]
    fn json_strings_are_escaped() {
        // Error messages carry identifiers and parse errors straight from user input.
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("line\nbreak"), "\"line\\nbreak\"");
        assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn json_lists_are_well_formed() {
        assert_eq!(json_list("problems", &[]), "{\"problems\":[]}");
        assert_eq!(
            json_list("problems", &["a".to_string(), "b\"c".to_string()]),
            "{\"problems\":[\"a\",\"b\\\"c\"]}"
        );
    }
}
