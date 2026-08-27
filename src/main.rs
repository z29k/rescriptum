//! rescriptum — serves Proxmox VE answer files to the automated installer.
//!
//! See CLAUDE.md for the design constraints. The short version: the request body is
//! never parsed as JSON, the answers directory is re-read on every request, and the
//! server is async so that a slow client costs a task rather than a thread.

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use rescriptum::capture::Capture;
use rescriptum::config::{self, Config};
use rescriptum::facts::Facts;
use rescriptum::select::Answers;
use rescriptum::{cli, log};
use std::convert::Infallible;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

/// Refuse a body larger than this rather than buffering it.
const MAX_BODY: usize = 1024 * 1024;

/// How long a shed connection is drained before it is closed, so the `503` actually
/// reaches a client that was still writing. See `shed`.
const SHED_DRAIN: std::time::Duration = std::time::Duration::from_millis(250);

type Body = Full<Bytes>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Answered before the configuration is even read: `--help` is what you reach for when
    // something is wrong, so a broken RESCRIPTUM_ENV_FILE must not be what stops you
    // reading it.
    //
    // `config` is here for a stronger version of the same reason. Every other subcommand
    // is handed a configuration that `Config::from_env` built and `validate` accepted —
    // and a file that will not parse, or a token one character short, are precisely the
    // states somebody runs `config` to get *out* of. Dispatching it below would make the
    // diagnostic tool the first casualty of the thing it diagnoses.
    match args.first().map(String::as_str) {
        Some("--help" | "-h" | "help") => {
            print!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Some("--version" | "-V") => {
            println!("rescriptum {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("config") => return cli::config(&args[1..]),
        _ => {}
    }

    let cfg = match Config::from_env() {
        Ok(cfg) => Arc::new(cfg),
        Err(problem) => {
            log::server(&format!("configuration error: {problem}"));
            return ExitCode::FAILURE;
        }
    };

    // Point logging at its destination before anything else is said. Whatever the
    // configuration itself had to report has already gone to stderr: until this runs,
    // there is nowhere else it could go.
    if let Err(problem) = log::init(cfg.log_level, cfg.log_file.as_deref()) {
        log::server(&format!("configuration error: {problem}"));
        return ExitCode::FAILURE;
    }

    // Refuse an unsafe or impossible combination before anything is listening.
    if let Err(problem) = cfg.validate() {
        log::server(&format!("configuration error: {problem}"));
        return ExitCode::FAILURE;
    }

    match args.split_first() {
        None => {}
        Some((cmd, rest)) if cmd == "render" => return cli::render(&cfg, rest),
        Some((cmd, _)) if cmd == "check" => return cli::check(&cfg),
        Some((cmd, rest)) if cmd == "import" => return cli::import(&cfg, rest),
        Some((cmd, rest)) if cmd == "export" => return cli::export(&cfg, rest),
        Some((cmd, rest)) if cmd == "media" => return cli::media(&cfg, rest),
        Some((cmd, _)) => {
            eprintln!("unknown argument {cmd:?}\n");
            eprint!("{}", cli::USAGE);
            return ExitCode::FAILURE;
        }
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cfg.workers)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::server(&format!("cannot start the async runtime: {e}"));
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(serve(cfg))
}

async fn serve(cfg: Arc<Config>) -> ExitCode {
    let listener = match TcpListener::bind(&cfg.listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            log::server(&format!("cannot bind {}: {e}", cfg.listen_addr));
            return ExitCode::FAILURE;
        }
    };

    // Shared across every connection: it holds the cached, parsed answer set.
    let store = match cfg.open_store() {
        Ok(store) => store,
        Err(e) => {
            log::server(&format!("cannot open the answer store: {e}"));
            return ExitCode::FAILURE;
        }
    };
    let answers = Arc::new(Answers::new(store.clone()));
    let capture = Arc::new(Capture::new(cfg.capture_dir.as_deref()));

    // The admin API, if it was configured. Separate listener, never this one.
    if let Some(addr) = cfg.admin_addr.clone() {
        let admin_listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                log::server(&format!("cannot bind the admin API on {addr}: {e}"));
                return ExitCode::FAILURE;
            }
        };
        let bound = admin_listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or(addr);
        log::server(&format!("admin API listening on {bound}"));
        if cfg.admin_is_exposed() {
            log::server(
                "warning: the admin API is not bound to loopback — it rewrites what gets \
                 installed on every machine, so restrict it to a management network",
            );
        }
        let admin = Arc::new(rescriptum::admin::Admin {
            cfg: Arc::clone(&cfg),
            store: store.clone(),
            answers: Arc::clone(&answers),
            guard: Default::default(),
        });
        tokio::spawn(rescriptum::admin::serve(admin_listener, admin));
    }

    // The media listener, if a media directory was named. Its own socket, its own
    // timeout and its own connection budget — see `boot::media` for why all three are
    // forced rather than preferred.
    #[cfg(feature = "boot")]
    if let Some(dir) = cfg.media_dir.clone() {
        let addr = cfg.media_addr();
        let media_listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                log::server(&format!("cannot bind the media listener on {addr}: {e}"));
                return ExitCode::FAILURE;
            }
        };
        let bound = media_listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or(addr);

        // Said out loud because it is the value every generated script is written
        // against, and a wrong guess here produces a machine that boots, chains, and
        // hangs on an address that does not exist. This log line is the only place the
        // answer will ever appear.
        let (host, derived) = cfg.public_host();
        if derived {
            log::server(&format!(
                "warning: RESCRIPTUM_PUBLIC_HOST is not set — derived {host}, which is what \
                 every generated URL will name. Multi-homed and NAT hosts get this wrong; \
                 set it explicitly if that address is not reachable from the machines."
            ));
        }
        log::server(&format!(
            "media listening on {bound} — serving {} as http://{host}",
            dir.display()
        ));

        let catalog = Arc::new(rescriptum::boot::catalog::Catalog::new(dir.clone()));
        // Load it now rather than on the first request, so a broken catalogue is known
        // before a machine asks rather than at 3am.
        for problem in catalog.problems().unwrap_or_default() {
            log::server(&format!("warning: media: {problem}"));
        }
        let media = Arc::new(rescriptum::boot::media::Media {
            cfg: Arc::clone(&cfg),
            catalog,
        });
        tokio::spawn(rescriptum::boot::media::serve(media_listener, media));
    }

    // TFTP, if a boot directory was named. It hands over **one file** — the loader —
    // and everything after that is HTTP: at 1468 bytes a round-trip, an image would
    // take twenty minutes where HTTP takes fifteen seconds.
    #[cfg(feature = "boot")]
    if let Some(dir) = cfg.boot_dir.clone() {
        let tftp = match rescriptum::boot::tftp::Tftp::new(&dir, Arc::clone(&cfg)) {
            Ok(tftp) => Arc::new(tftp),
            // Fatal: a boot directory that cannot be resolved is not something that
            // fixes itself, and every path check below compares against it.
            Err(e) => {
                log::server(&format!("configuration error: {e}"));
                return ExitCode::FAILURE;
            }
        };
        let addr = cfg.tftp_addr();
        let socket = match tokio::net::UdpSocket::bind(&addr).await {
            Ok(socket) => socket,
            Err(e) => {
                log::server(&format!(
                    "cannot bind TFTP on {addr}: {e}{}",
                    if addr.ends_with(":69") {
                        " — port 69 is privileged; run as root and set RESCRIPTUM_USER to \
                         drop afterwards, use setcap, or choose another port"
                    } else {
                        ""
                    }
                ));
                return ExitCode::FAILURE;
            }
        };
        let bound = socket
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| addr.clone());
        log::server(&format!(
            "tftp listening on {bound} — serving {}",
            tftp.root().display()
        ));
        tokio::spawn(rescriptum::boot::tftp::serve(socket, tftp));
    }

    // **Bind everything first, then drop.** The other order works as root in testing
    // and fails on deployment, which is the bug this ordering exists to prevent.
    #[cfg(feature = "boot")]
    if let Err(e) = rescriptum::boot::privileges::drop_to(cfg.user.as_deref(), cfg.group.as_deref())
    {
        log::server(&format!("configuration error: {e}"));
        return ExitCode::FAILURE;
    }

    // Report the address actually bound, not the one requested: with `:0` (used by the
    // integration tests, and handy for debugging) they differ.
    let bound = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| cfg.listen_addr.clone());

    log::server(&format!(
        "rescriptum {} listening on {bound} — store={} workers={} max_conn={} timeout={}s log={}",
        env!("CARGO_PKG_VERSION"),
        answers.describe(),
        cfg.workers,
        cfg.max_connections,
        cfg.timeout.as_secs(),
        // Named here so "why is my log empty" is answered by the log itself.
        cfg.log_level.label(),
    ));
    if cfg.store == config::StoreKind::Files {
        // None of these is fatal: the path may appear, or its permissions be fixed, and it
        // is re-read as it changes. But say so loudly, and say which one it is — between
        // them they are the most likely reason for a 404 storm, and "does not exist" sends
        // you looking in the wrong place when the path exists and is a file.
        let dir = &cfg.answers_dir;
        if !dir.exists() {
            log::server(&format!(
                "warning: {} does not exist yet — every request will 404 until it does",
                dir.display()
            ));
        } else if !dir.is_dir() {
            log::server(&format!(
                "warning: {} is not a directory — every request will 404 until it is",
                dir.display()
            ));
        } else if let Err(e) = std::fs::read_dir(&cfg.answers_dir) {
            // Asking the filesystem beats inspecting permission bits: it accounts for the
            // owner, the group, ACLs and whatever the mount decided. This is the failure a
            // packaged install meets first — DSM runs a package as a non-root user, so a
            // directory created by hand as root is readable to nobody who matters.
            log::server(&format!(
                "warning: {} cannot be read: {e} — every request will 404 until that is \
                 fixed; check the directory's owner against the user this server runs as",
                cfg.answers_dir.display()
            ));
        }
    }
    // Load the answer set now rather than on the first request, so a broken one is known
    // before a machine asks rather than at 3am. `listing()` logs each problem itself, so
    // the result is deliberately discarded here: reporting it again would print every
    // problem twice at startup.
    let _ = answers.problems();

    // Async connections are cheap, but "cheap" is not "free": still bound them, so a
    // burst degrades into prompt 503s rather than an out-of-memory.
    let permits = Arc::new(Semaphore::new(cfg.max_connections));

    loop {
        tokio::select! {
            _ = shutdown_signal() => {
                log::server("shutting down");
                return ExitCode::SUCCESS;
            }
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(pair) => pair,
                    // An accept failure (fd exhaustion, say) must not end the loop.
                    Err(e) => {
                        log::server(&format!("accept failed: {e}"));
                        continue;
                    }
                };

                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    log::request(&peer.to_string(), 503, "503 — at max connections, shedding");
                    tokio::spawn(shed(stream));
                    continue;
                };

                let cfg = Arc::clone(&cfg);
                let answers = Arc::clone(&answers);
                let capture = Arc::clone(&capture);
                tokio::spawn(async move {
                    let _permit = permit; // released when the connection ends
                    connection(stream, peer.to_string(), cfg, answers, capture).await;
                });
            }
        }
    }
}

/// SIGTERM (what DSM's task scheduler sends) or Ctrl-C.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(_) => return ctrl_c.await,
            };
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await
}

/// Answer a shed connection honestly instead of dropping it silently, so the client
/// knows to retry rather than guessing.
async fn shed(mut stream: TcpStream) {
    const BODY: &str = "503 Service Unavailable\n";
    let response = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
        BODY.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;

    // Then read whatever the client was in the middle of sending, and only close after
    // that. Closing on a peer that still has data in flight makes the kernel send a
    // reset, and a reset **discards the response we just wrote** — the installer sees a
    // dropped connection instead of "503, come back", which is precisely the guessing
    // this response exists to prevent. The same trap is already avoided in the admin
    // API's `put()`.
    //
    // Bounded by a short deadline: a shed connection holds no permit, but it must not
    // become a way to hold this task open either.
    let mut scratch = [0u8; 8 * 1024];
    let _ = tokio::time::timeout(SHED_DRAIN, async {
        while let Ok(n) = stream.read(&mut scratch).await {
            if n == 0 {
                break;
            }
        }
    })
    .await;
    let _ = stream.shutdown().await;
}

async fn connection(
    stream: TcpStream,
    peer: String,
    cfg: Arc<Config>,
    answers: Arc<Answers>,
    capture: Arc<Option<Capture>>,
) {
    let timeout = cfg.timeout;
    let peer_label = peer.clone();
    let io = TokioIo::new(stream);
    let service = service_fn(move |req| {
        let cfg = Arc::clone(&cfg);
        let answers = Arc::clone(&answers);
        let capture = Arc::clone(&capture);
        let peer = peer.clone();
        async move { Ok::<_, Infallible>(handle(req, cfg, answers, capture, peer).await) }
    });

    // `header_read_timeout` is the slowloris guard: a client that opens a socket and
    // dawdles over its headers is dropped rather than parked forever.
    let serving = http1::Builder::new()
        // hyper panics at connection time if a timeout is set without a timer to drive
        // it. Not optional, and not caught at compile time.
        .timer(TokioTimer::new())
        .header_read_timeout(Some(timeout))
        .serve_connection(io, service);

    // `header_read_timeout` stops at the end of the headers; hyper has no equivalent
    // for a body that trickles or never arrives. A whole-connection deadline covers
    // both, and costs nothing here: one request, tiny response, then close.
    match tokio::time::timeout(timeout, serving).await {
        Err(_elapsed) => log::request(&peer_label, 0, "- connection timed out"),
        Ok(Err(e)) => {
            // Clients hanging up mid-request is routine, not worth a log line each time.
            if !e.is_incomplete_message() {
                log::server(&format!("connection error: {e}"));
            }
        }
        Ok(Ok(())) => {}
    }
}

async fn handle(
    req: Request<Incoming>,
    cfg: Arc<Config>,
    answers: Arc<Answers>,
    capture: Arc<Option<Capture>>,
    peer: String,
) -> Response<Body> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let full_target = req
        .uri()
        .path_and_query()
        .map(|pq| pq.to_string())
        .unwrap_or_else(|| path.clone());

    if method == Method::GET && path == "/health" {
        log::request(&peer, 200, "GET /health 200");
        return text(StatusCode::OK, "OK\n");
    }

    // An installer that was given a token must present it. Proxmox does when its ISO
    // was prepared with `--answer-auth-token`; nothing else can, which is why this is
    // off unless configured. What it guards is the root password hash and the SSH keys
    // of every machine installed afterwards.
    if let Some(expected) = &cfg.answer_token
        && !bearer_matches(&req, expected)
    {
        // Logged every time and never rate-limited: a whole rack can sit behind one
        // address, and shutting it out would turn a bad token into a failed rollout.
        log::request(
            &peer,
            401,
            &format!("{method} {full_target} 401 bad or missing token"),
        );
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("WWW-Authenticate", "Bearer")
            .header("Connection", "close")
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(Full::new(Bytes::from("401 Unauthorized\n")))
            .expect("a static response always builds");
    }

    // Any path, because the URL is baked into the ISO and we do not get to choose it.
    // GET as well as POST: Proxmox posts its hardware inventory, but a Debian preseed
    // or a RHEL kickstart is fetched, with the machine's identity in the query string.
    if method != Method::POST && method != Method::GET {
        log::request(&peer, 405, &format!("{method} {full_target} 405"));
        return text(StatusCode::METHOD_NOT_ALLOWED, "405 Method Not Allowed\n");
    }
    let query = req.uri().query().map(str::to_string);
    let request_path = path.clone();

    // Refuse an aberrant Content-Length up front, so an absurd declared size is never
    // read at all rather than being read until it trips the limit.
    let declared = req
        .headers()
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_BODY as u64) {
        let n = declared.unwrap_or(0);
        log::request(
            &peer,
            413,
            &format!("POST {full_target} 413 content-length {n} exceeds {MAX_BODY}"),
        );
        return text(StatusCode::PAYLOAD_TOO_LARGE, "413 Content Too Large\n");
    }

    let body = match Limited::new(req.into_body(), MAX_BODY).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            let too_large = e
                .downcast_ref::<http_body_util::LengthLimitError>()
                .is_some();
            let (status, why) = if too_large {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("body exceeds {MAX_BODY} bytes"),
                )
            } else {
                (StatusCode::BAD_REQUEST, format!("cannot read body: {e}"))
            };
            log::request(
                &peer,
                status.as_u16(),
                &format!("POST {full_target} {} {why}", status.as_u16()),
            );
            return text(
                status,
                format!(
                    "{} {}\n",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                ),
            );
        }
    };

    let size = body.len();
    let prefix = format!("{method} {full_target} body={size}");

    // `read_dir` and `read` are blocking, and blocking an async worker stalls every
    // other connection it is driving. Push both onto the blocking pool.
    // The recorder needs the body after resolution has taken it.
    let captured: Vec<u8> = if capture.is_some() {
        body.to_vec()
    } else {
        Vec::new()
    };

    // Parsing the body into facts is CPU work on an arbitrary-sized payload, and the
    // lookup below is blocking IO — both belong off the async worker.
    let picked = tokio::task::spawn_blocking(move || {
        let facts = Facts::from_request(Some(&request_path), query.as_deref(), &body);
        answers.resolve(&facts)
    })
    .await;

    let record = |outcome: &str| {
        if let Some(capture) = capture.as_ref() {
            capture.record(&peer, method.as_str(), &full_target, &captured, outcome);
        }
    };

    match picked {
        Ok(Ok(Some(resolution))) => {
            record(&format!("200 {}", resolution.how()));
            log::request(
                &peer,
                200,
                &format!(
                    "{prefix} 200 {} bytes={}",
                    resolution.how(),
                    resolution.body.len()
                ),
            );
            // text/plain, per the installer's expectation — not application/toml.
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", resolution.content_type)
                .header("Connection", "close")
                .body(Full::new(Bytes::from(resolution.body)))
                .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "500\n"))
        }
        Ok(Ok(None)) => {
            record("404");
            log::request(&peer, 404, &format!("{prefix} 404 no answer file applies"));
            text(StatusCode::NOT_FOUND, "404 Not Found\n")
        }
        // A misconfiguration (bad TOML, a missing group) must be loud: a half-built
        // answer file would install a machine wrongly, which is worse than not at all.
        Ok(Err(e)) => {
            record("500");
            log::request(&peer, 500, &format!("{prefix} 500 {e}"));
            text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "500 Internal Server Error\n",
            )
        }
        // A panic in the blocking task. It cannot take the server with it, but it must
        // not pass silently either.
        Err(e) => {
            log::request(
                &peer,
                500,
                &format!("{prefix} 500 answer lookup panicked: {e}"),
            );
            text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "500 Internal Server Error\n",
            )
        }
    }
}

/// Compare the request's bearer token without an early return, so a wrong one cannot be
/// recovered a byte at a time by whoever is timing the responses.
fn bearer_matches(req: &Request<Incoming>, expected: &str) -> bool {
    let Some(value) = req
        .headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let (a, b) = (value.trim().as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn text(status: StatusCode, body: impl Into<Bytes>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Connection", "close")
        .body(Full::new(body.into()))
        .expect("a static response always builds")
}
