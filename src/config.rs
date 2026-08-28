//! Configuration, entirely from the environment.

use std::path::PathBuf;
use std::time::Duration;

/// `/srv` is where the FHS puts "data for services provided by this system", which is
/// exactly what an answers directory is. It also happens to exist on every Linux host,
/// which the previous default (`/volume1/netboot/answers`) did not: that is a Synology
/// path, and this project being written for a DS416j was never a good enough reason to
/// make the default wrong everywhere else. The NAS sets it explicitly like anyone else.
pub const DEFAULT_ANSWERS_DIR: &str = "/srv/answers";
/// A sibling of the directory above, for the same reasons. The database holds the same
/// curated content, not runtime state, so it belongs in the same tree.
pub const DEFAULT_DB_PATH: &str = "/srv/answers.db";
pub const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8000";
/// Async runtime threads. Unlike a thread-per-connection pool this is a CPU count,
/// not a concurrency limit — connections are tasks, not threads.
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
}
/// In-flight connections allowed before we shed with 503. Async connections cost
/// kilobytes rather than a stack, so this can be far higher than a pool size — but it
/// is still bounded: unbounded accept is how you turn a burst into an out-of-memory.
pub const DEFAULT_MAX_CONNECTIONS: usize = 2048;
/// Read/write timeout. Also the slowloris budget: a stalled client can occupy a
/// worker for at most this long.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// The media listener's port, and **it is a contract rather than a preference.** The
/// loader we ship carries an embedded script that chains to `${next-server}:8001`, and
/// that script is baked in before any deployment exists — it can read no configuration.
/// Moving this is allowed and survivable, but every loader already shipped assumes it.
pub const DEFAULT_MEDIA_ADDR: &str = "0.0.0.0:8001";
/// A whole-transfer deadline, deliberately not the answer listener's ten seconds: a
/// 1.5 GB image is fifteen seconds on gigabit and two minutes on 100 Mbit, and on the
/// answer listener every download would be killed mid-transfer.
pub const DEFAULT_MEDIA_TIMEOUT_SECS: u64 = 600;
/// Concurrent transfers, low on purpose. A download holds its permit for minutes, and
/// the small end of the range this has to work on is a NAS with one spinning disk.
pub const DEFAULT_MEDIA_MAX_CONNECTIONS: usize = 16;
/// TFTP's well-known port, and privileged. It is the only privileged port this server
/// ever wants — with no DHCP responder there is nothing after 67 or 4011.
pub const DEFAULT_TFTP_ADDR: &str = "0.0.0.0:69";
/// Seconds the built-in menu waits before falling through to local boot. **The name
/// spells the unit because `choose` counts milliseconds**: a seconds value passed
/// through unconverted is a menu that flashes past before a human has read its title.
pub const DEFAULT_BOOT_TIMEOUT_SECS: u64 = 15;

/// Where answers are read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    /// A directory of TOML files — the default, and the right fit when dropping a file
    /// onto a NAS is all the administration you need.
    Files,
    /// A SQLite database, for deployments administered over the API.
    Sqlite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub store: StoreKind,
    pub db_path: PathBuf,
    pub answers_dir: PathBuf,
    pub listen_addr: String,
    /// Where the admin API listens. `None` — the default — means it is off entirely.
    pub admin_addr: Option<String>,
    /// Bearer token for the admin API. Required whenever `admin_addr` is set.
    pub admin_token: Option<String>,
    /// Bearer token the *answer* endpoint requires, if any.
    ///
    /// Proxmox sends one when its ISO is prepared with `--answer-auth-token`, as
    /// `Authorization: Bearer <name>:<secret>`. Unset by default and necessarily so:
    /// most installers have no credential to offer, and refusing them would refuse the
    /// install. Set it when every client can present one.
    pub answer_token: Option<String>,
    /// Where to record what machines actually send. Off unless set.
    pub capture_dir: Option<PathBuf>,
    /// How much to log. `All` keeps every request; `Problems` keeps everything except the
    /// requests that worked, which is the only high-volume thing here.
    pub log_level: crate::log::Level,
    /// Where the log goes. `None` is stderr; the literals `stderr` and `stdout` name those
    /// two, and anything else is a file to append to.
    pub log_file: Option<PathBuf>,
    pub workers: usize,
    pub max_connections: usize,
    pub timeout: Duration,

    /// The host this server names itself by in the scripts it writes.
    ///
    /// **A host, never a URL.** The server writes URLs for two listeners plus a bare
    /// address into a DHCP snippet, so a value carrying one port would silently pin
    /// every generated script to one listener. Each URL appends its own port.
    ///
    /// `None` means derive one and say so loudly — a wrong guess here produces a
    /// machine that boots, chains, and hangs on an address that does not exist, and
    /// the startup log line is the only place the answer will ever appear.
    pub public_host: Option<String>,
    /// Where installer images live. **Unset is the whole off switch**: no media
    /// directory, no media listener, nothing changes for an existing deployment.
    pub media_dir: Option<PathBuf>,
    /// The media listener's address, **as the operator set it**. `None` means nobody
    /// did, and `media_addr()` supplies the default. The distinction is kept because
    /// naming an address without naming a directory is a mistake worth refusing, and a
    /// value that had already been defaulted could not be told from one that was asked
    /// for.
    pub media_addr: Option<String>,
    pub media_timeout: Duration,
    pub media_max_connections: usize,
    /// Proxmox's `[post-installation-webhook]` token. **Unset is the whole off switch**:
    /// no token, no endpoint — absent rather than open. See `installed`.
    pub installed_token: Option<String>,
    /// A CIDR allowlist for boot traffic. Unset means anyone who can reach the port.
    pub boot_allow: Option<String>,
    /// Loaders and menus — what TFTP hands out. **Unset means no TFTP at all**, the
    /// same off switch shape the media directory has.
    pub boot_dir: Option<PathBuf>,
    /// The TFTP listener, as the operator set it. `None` means nobody did; see
    /// `tftp_addr()`.
    pub tftp_addr: Option<String>,
    /// The largest TFTP block this server will agree to. See `tftp_blksize`.
    pub tftp_blksize: Option<usize>,
    /// Seconds before the built-in menu falls through to booting from local disk.
    pub boot_timeout: Duration,
    /// What a machine no answer claims is offered. See `unclaimed_boots_local`.
    pub boot_unclaimed: Option<String>,
    /// Replace the embedded logo and the menu's title, for a site that wants its own.
    pub boot_logo: Option<PathBuf>,
    pub boot_title: Option<String>,
    /// Drop to this user and group **after** binding. Binding first is the whole point:
    /// the other order works as root in testing and fails on deployment.
    pub user: Option<String>,
    pub group: Option<String>,
}

impl Config {
    /// The process environment, plus the file `RESCRIPTUM_ENV_FILE` names, if it names
    /// one.
    ///
    /// **The real environment wins**: the file supplies defaults, so something exported
    /// deliberately at launch is never silently overridden. A file that was asked for and
    /// cannot be read is an error rather than a shrug — carrying on with defaults is the
    /// silent failure the file exists to remove. Warnings about it are logged here, the
    /// way `Capture::new` reports its own.
    pub fn from_env() -> Result<Config, String> {
        let named = std::env::var(crate::envfile::ENV_FILE)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let file = match named {
            Some(path) => {
                let file = crate::envfile::EnvFile::load(path)?;
                crate::log::server(&format!(
                    "reading configuration defaults from {} ({} set)",
                    file.path.display(),
                    file.len()
                ));
                for warning in &file.warnings {
                    crate::log::server(&format!("warning: {warning}"));
                }
                Some(file)
            }
            None => None,
        };

        Ok(Config::from_lookup(|key| {
            std::env::var(key)
                .ok()
                // An exported-but-empty variable is a mistake, not an instruction — so it
                // does not count as "set in the environment" and the file still applies.
                .filter(|v| !v.trim().is_empty())
                .or_else(|| file.as_ref().and_then(|f| f.get(key)))
        }))
    }

    /// Split out from `from_env` so tests don't have to mutate the process
    /// environment, which is global and racy under a threaded test runner.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Config {
        // An exported-but-empty variable (`RESCRIPTUM_LISTEN_ADDR=`) is a mistake, not a
        // request to listen on "". Treat it as unset.
        let get = |key: &str, default: &str| -> String {
            match lookup(key) {
                Some(v) if !v.trim().is_empty() => v.trim().to_string(),
                _ => default.to_string(),
            }
        };

        // A nonsense number is a typo, not an instruction. Fall back rather than
        // starting with zero workers and quietly serving nobody.
        let get_usize = |key: &str, default: usize| -> usize {
            match lookup(key).and_then(|v| v.trim().parse::<usize>().ok()) {
                Some(n) if n > 0 => n,
                _ => default,
            }
        };

        // An unrecognised value is a typo. Say so rather than silently serving from
        // somewhere the operator did not mean.
        let store = match lookup("RESCRIPTUM_STORE").as_deref().map(str::trim) {
            None | Some("") | Some("files") => StoreKind::Files,
            Some("sqlite") => StoreKind::Sqlite,
            Some(other) => {
                crate::log::server(&format!(
                    "warning: RESCRIPTUM_STORE={other:?} is not \"files\" or \"sqlite\" — using files"
                ));
                StoreKind::Files
            }
        };

        let optional = |key: &str| -> Option<String> {
            lookup(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };

        Config {
            admin_addr: optional("RESCRIPTUM_ADMIN_ADDR"),
            admin_token: optional("RESCRIPTUM_ADMIN_TOKEN"),
            answer_token: optional("RESCRIPTUM_ANSWER_TOKEN"),
            capture_dir: optional("RESCRIPTUM_CAPTURE_DIR").map(PathBuf::from),
            log_level: match optional("RESCRIPTUM_LOG") {
                Some(value) => crate::log::Level::parse(&value).unwrap_or_else(|| {
                    // A typo must not be the reason nobody can see why a rollout failed.
                    crate::log::server(&format!(
                        "warning: RESCRIPTUM_LOG={value:?} is not \"all\", \"problems\" or \"off\" — using all"
                    ));
                    crate::log::Level::All
                }),
                None => crate::log::Level::All,
            },
            log_file: optional("RESCRIPTUM_LOG_FILE").map(PathBuf::from),
            store,
            db_path: PathBuf::from(get("RESCRIPTUM_DB_PATH", DEFAULT_DB_PATH)),
            answers_dir: PathBuf::from(get("RESCRIPTUM_ANSWERS_DIR", DEFAULT_ANSWERS_DIR)),
            listen_addr: get("RESCRIPTUM_LISTEN_ADDR", DEFAULT_LISTEN_ADDR),
            workers: get_usize("RESCRIPTUM_WORKERS", default_workers()),
            max_connections: get_usize("RESCRIPTUM_MAX_CONNECTIONS", DEFAULT_MAX_CONNECTIONS),
            timeout: Duration::from_secs(get_usize(
                "RESCRIPTUM_TIMEOUT_SECS",
                DEFAULT_TIMEOUT_SECS as usize,
            ) as u64),
            public_host: optional("RESCRIPTUM_PUBLIC_HOST"),
            media_dir: optional("RESCRIPTUM_MEDIA_DIR").map(PathBuf::from),
            media_addr: optional("RESCRIPTUM_MEDIA_ADDR"),
            media_timeout: Duration::from_secs(get_usize(
                "RESCRIPTUM_MEDIA_TIMEOUT_SECS",
                DEFAULT_MEDIA_TIMEOUT_SECS as usize,
            ) as u64),
            media_max_connections: get_usize(
                "RESCRIPTUM_MEDIA_MAX_CONNECTIONS",
                DEFAULT_MEDIA_MAX_CONNECTIONS,
            ),
            installed_token: optional("RESCRIPTUM_INSTALLED_TOKEN"),
            boot_allow: optional("RESCRIPTUM_BOOT_ALLOW"),
            boot_dir: optional("RESCRIPTUM_BOOT_DIR").map(PathBuf::from),
            tftp_addr: optional("RESCRIPTUM_TFTP_ADDR"),
            boot_timeout: Duration::from_secs(get_usize(
                "RESCRIPTUM_BOOT_TIMEOUT_SECS",
                DEFAULT_BOOT_TIMEOUT_SECS as usize,
            ) as u64),
            tftp_blksize: optional("RESCRIPTUM_TFTP_BLKSIZE")
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|n| *n > 0),
            boot_unclaimed: optional("RESCRIPTUM_BOOT_UNCLAIMED"),
            boot_logo: optional("RESCRIPTUM_BOOT_LOGO").map(PathBuf::from),
            boot_title: optional("RESCRIPTUM_BOOT_TITLE"),
            user: optional("RESCRIPTUM_USER"),
            group: optional("RESCRIPTUM_GROUP"),
        }
    }
}

impl Config {
    /// Refuse to start on a combination that would be unsafe or simply not work.
    ///
    /// These are startup errors rather than warnings on purpose: an admin API that
    /// silently came up without a token would let anyone who can reach it rewrite the
    /// root password and SSH keys of every machine subsequently installed.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_media()?;

        let Some(addr) = &self.admin_addr else {
            return Ok(());
        };

        if self.store != StoreKind::Sqlite {
            return Err(format!(
                "RESCRIPTUM_ADMIN_ADDR is set ({addr}), but RESCRIPTUM_STORE is not \"sqlite\". The admin \
                 API is only available over the database — with files, edit the directory."
            ));
        }
        match &self.admin_token {
            None => {
                return Err(
                    "RESCRIPTUM_ADMIN_ADDR is set but RESCRIPTUM_ADMIN_TOKEN is not. The admin API changes \
                     what gets installed on every machine; it does not run unauthenticated."
                        .to_string(),
                );
            }
            // A short token is a guessable token, and this endpoint is worth guessing.
            Some(t) if t.len() < 16 => {
                return Err(format!(
                    "RESCRIPTUM_ADMIN_TOKEN is {} characters; use at least 16.",
                    t.len()
                ));
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// The media half of `validate`. Same rule as everywhere else here: refuse only
    /// what would not work or would not be safe, and warn about everything that can be
    /// fixed while the server runs.
    fn validate_media(&self) -> Result<(), String> {
        // A binary built without the feature must say so rather than ignoring the
        // directory it was pointed at. Same shape as `open_store` refusing
        // `RESCRIPTUM_STORE=sqlite` without the `sqlite` feature: the variable stays
        // described everywhere, and only the binary that cannot honour it objects.
        #[cfg(not(feature = "boot"))]
        if self.media_dir.is_some() {
            return Err(
                "RESCRIPTUM_MEDIA_DIR is set, but this binary was built without the `boot` \
                 feature, so it can serve no media."
                    .to_string(),
            );
        }

        // A host, never a URL. One port in the value would silently pin every generated
        // script to one listener, and the symptom is a machine chaining into nowhere.
        if let Some(host) = &self.public_host {
            let wrong = if host.contains("://") {
                Some("a scheme")
            } else if host.contains('/') {
                Some("a path")
            } else if host.rsplit_once(':').is_some_and(|(head, tail)| {
                // `[::1]` is an address, not a host with a port. Only a trailing
                // `:digits` after something that is not a bracketed address is one.
                !host.starts_with('[')
                    && !head.contains(':')
                    && tail.chars().all(|c| c.is_ascii_digit())
            }) {
                Some("a port")
            } else {
                None
            };
            if let Some(wrong) = wrong {
                return Err(format!(
                    "RESCRIPTUM_PUBLIC_HOST is {host:?}, which carries {wrong}. It is a host \
                     on its own — every generated URL appends its own listener's port."
                ));
            }
        }

        if self.tftp_addr.is_some() && !self.tftp_is_off() && self.boot_dir.is_none() {
            return Err(format!(
                "RESCRIPTUM_TFTP_ADDR is set ({}), but RESCRIPTUM_BOOT_DIR is not. There would \
                 be a listener with no loaders to hand out.",
                self.tftp_addr.as_deref().unwrap_or_default()
            ));
        }

        if self.media_addr.is_some() && self.media_dir.is_none() {
            return Err(format!(
                "RESCRIPTUM_MEDIA_ADDR is set ({}), but RESCRIPTUM_MEDIA_DIR is not. There \
                 would be a listener with nothing to serve.",
                self.media_addr.as_deref().unwrap_or_default()
            ));
        }

        // Two listeners on one port: the second bind fails, and which one loses depends
        // on start order. Saying so beats a race whose symptom is "it worked yesterday".
        //
        // Port zero is exempt, and not as a special case for tests: `:0` asks the kernel
        // for *any* free port, so two of them are never the same port. Refusing them
        // would refuse the one configuration that cannot collide.
        if self.media_dir.is_some() && !ephemeral(&self.media_addr()) {
            let media = self.media_addr();
            for (other, name) in [
                (Some(&self.listen_addr), "RESCRIPTUM_LISTEN_ADDR"),
                (self.admin_addr.as_ref(), "RESCRIPTUM_ADMIN_ADDR"),
            ] {
                if other.is_some_and(|o| o == &media) {
                    return Err(format!(
                        "RESCRIPTUM_MEDIA_ADDR and {name} are both {media}. Media downloads \
                         hold a connection for minutes and answers must not queue behind \
                         them, which is why they are separate listeners."
                    ));
                }
            }
        }
        Ok(())
    }

    /// The TFTP listener's effective address, or `None` when TFTP is off.
    ///
    /// **`off` is a value, not an absence** — it is how an operator says the loader will
    /// come from somebody else's TFTP server while rescriptum keeps serving the rest of
    /// the chain. The loaders stay served over HTTP at `/boot/…` and stay checked by
    /// `boot check`; only the listener is gone.
    ///
    /// **It is a deployment workaround, never a packaged default.** rescriptum *is* the
    /// TFTP server; a build or a package that ships with `off` set has traded away the
    /// thing it is for. Where the platform makes port 69 hard, the answer is to make one
    /// of the three routes durable there — bind then drop, socket activation, `setcap` —
    /// not to hand the port to another daemon. See `tftp_addr_is_named` for what happens
    /// when the route has not been opened yet.
    pub fn tftp_addr(&self) -> Option<String> {
        match self.tftp_addr.as_deref() {
            Some(value) if is_off(value) => None,
            Some(value) => Some(value.to_string()),
            None => Some(DEFAULT_TFTP_ADDR.to_string()),
        }
    }

    /// Whether TFTP was turned off deliberately, as opposed to never asked for. Worth
    /// telling apart: the first deserves a line at startup saying what will hand the
    /// loader over instead.
    pub fn tftp_is_off(&self) -> bool {
        self.tftp_addr.as_deref().is_some_and(is_off)
    }

    /// Whether a machine that no answer claims is sent straight to its own disk instead
    /// of being offered the menu.
    ///
    /// **The default is the menu, and that is the project's thesis rather than an
    /// oversight**: a machine nobody has decided anything about should end up somewhere a
    /// human can decide, not silently do nothing. That is right for a machine being
    /// provisioned, and wrong for a fleet already in production — where most machines are
    /// installed, and showing every one of them a menu for fifteen seconds on every
    /// reboot is noise at best and an accidental reinstall at worst.
    ///
    /// So `local` inverts what an answer file *means*. With the menu, a file claiming a
    /// machine is how you say "leave this one alone"; with `local`, a file is how you say
    /// "install this one", and its absence is the safe state. The second reading is the
    /// one that scales, because the number of machines you want to reinstall is always
    /// smaller than the number you do not.
    pub fn unclaimed_boots_local(&self) -> bool {
        self.boot_unclaimed
            .as_deref()
            .map(str::trim)
            .is_some_and(|v| v.eq_ignore_ascii_case("local"))
    }

    /// The largest TFTP block to agree to, when a client asks for a bigger one.
    ///
    /// **1468 exactly fills a 1500-byte path and leaves nothing over**: 1468 of payload,
    /// 4 of TFTP header, 8 of UDP, 20 of IP. iPXE asks for precisely that, and on a plain
    /// untagged Ethernet it is right. Put one VLAN tag in the way and the frame is 1504,
    /// which is dropped or fragmented — and a PXE ROM meeting either usually just stops,
    /// with no message, having downloaded nothing. Same for PPPoE, and for any tunnel.
    ///
    /// So this exists to be lowered when a boot stalls at the first block. 1400 leaves 68
    /// bytes of headroom, which covers a tag and most tunnels; 512 is the RFC default and
    /// always works. The default stays at what fits a clean path, because lowering it for
    /// everybody costs every deployment throughput to fix a minority's network — but the
    /// failure it causes is now loud enough to find.
    pub fn tftp_blksize(&self) -> usize {
        self.tftp_blksize
            .unwrap_or(crate::boot::tftp::MAX_BLOCK)
            .clamp(
                crate::boot::tftp::DEFAULT_BLOCK,
                crate::boot::tftp::MAX_BLOCK,
            )
    }

    /// The menu timeout **in milliseconds**, which is the unit `choose` counts. The
    /// conversion has exactly one place, and this is it.
    pub fn boot_timeout_millis(&self) -> u64 {
        self.boot_timeout.as_millis() as u64
    }

    /// The media listener's effective address.
    pub fn media_addr(&self) -> String {
        self.media_addr
            .clone()
            .unwrap_or_else(|| DEFAULT_MEDIA_ADDR.to_string())
    }

    /// The host this server names itself by, and where that name came from.
    ///
    /// Derivation opens a UDP socket toward a documentation address and reads back the
    /// local address the routing table chose. **No packet is sent** — connecting a UDP
    /// socket only picks a route. It is the standard trick, it costs nothing, and it is
    /// wrong often enough on multi-homed and NAT hosts to be a warning rather than a
    /// silent success.
    pub fn public_host(&self) -> (String, bool) {
        match &self.public_host {
            Some(host) => (host.clone(), false),
            None => (
                derive_public_host().unwrap_or_else(|| "127.0.0.1".to_string()),
                true,
            ),
        }
    }

    /// The two URLs a generated script needs, each with its own listener's port.
    #[cfg(feature = "boot")]
    pub fn endpoints(&self) -> crate::boot::stanza::Endpoints {
        let (host, _) = self.public_host();
        crate::boot::stanza::Endpoints {
            media: format!("http://{}", join(&host, &self.media_addr())),
            answer: format!("http://{}", join(&host, &self.listen_addr)),
        }
    }

    /// The answer endpoint's own check, separate because a short token there is worth a
    /// warning rather than a refusal: an installer's token format is not ours to choose,
    /// and refusing to start would leave a fleet unable to install.
    pub fn answer_token_warning(&self) -> Option<String> {
        match &self.answer_token {
            Some(t) if t.len() < 16 => Some(format!(
                "RESCRIPTUM_ANSWER_TOKEN is only {} characters — it guards root passwords and \
                 SSH keys, so make it longer",
                t.len()
            )),
            _ => None,
        }
    }

    /// Whether the admin address points somewhere other than loopback. Not an error —
    /// a management network is a legitimate choice — but worth saying out loud.
    pub fn admin_is_exposed(&self) -> bool {
        match &self.admin_addr {
            None => false,
            Some(addr) => {
                let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
                let host = host.trim_start_matches('[').trim_end_matches(']');
                !(host == "127.0.0.1" || host == "::1" || host == "localhost")
            }
        }
    }

    /// Open the configured store. Returns the write-capable trait so the admin API can
    /// use the same object; serving answers only ever needs the read half.
    pub fn open_store(&self) -> std::io::Result<std::sync::Arc<dyn crate::store::StoreWrite>> {
        match self.store {
            StoreKind::Files => Ok(std::sync::Arc::new(crate::store::FileStore::new(
                self.answers_dir.clone(),
            ))),
            #[cfg(feature = "sqlite")]
            StoreKind::Sqlite => Ok(std::sync::Arc::new(crate::store::SqliteStore::open(
                self.db_path.clone(),
            )?)),
            #[cfg(not(feature = "sqlite"))]
            StoreKind::Sqlite => Err(std::io::Error::other(
                "RESCRIPTUM_STORE=sqlite, but this binary was built without the `sqlite` feature",
            )),
        }
    }
}

/// The spellings that mean "not at all", matching `RESCRIPTUM_LOG=off`.
fn is_off(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "off" | "none" | "disabled"
    )
}

/// Whether an address asks the kernel to choose the port. Two such listeners never
/// collide, however identical the strings look.
fn ephemeral(addr: &str) -> bool {
    addr.rsplit_once(':')
        .is_some_and(|(_, port)| port.trim() == "0")
}

/// A reachable host plus the port of a listen address, ready to go into a URL.
///
/// Only `endpoints` calls this, and only a binary that can serve media has one.
#[cfg(feature = "boot")]
///
/// The listen address is usually `0.0.0.0:8001`, which is not something anybody can
/// fetch from — the port is the only part of it worth keeping.
fn join(host: &str, listen_addr: &str) -> String {
    let port = listen_addr
        .rsplit_once(':')
        .map(|(_, port)| port)
        .unwrap_or("80");
    // An IPv6 literal needs brackets before a port can follow it.
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Ask the routing table which of this host's addresses faces the outside world.
///
/// 192.0.2.1 is TEST-NET-1, a documentation address that exists to be written down and
/// never answered. Connecting a UDP socket to it sends nothing; it only makes the
/// kernel choose a source address, which is the answer we are after — and on a host
/// with one interface it is simply the right one.
pub fn derive_public_host() -> Option<String> {
    choose_host(routed_address(), &local_addresses())
}

/// The choice itself, separated from the two syscalls that feed it so it can be tested.
fn choose_host(routed: Option<String>, addresses: &[String]) -> Option<String> {
    if routed.is_some() {
        return routed;
    }
    // No default route — an isolated provisioning segment, which is a perfectly ordinary
    // way to run this. The routing table has nothing to say, but the interface list
    // still does: with exactly one address there is no choice to get wrong. With
    // several there is, and guessing one silently is worse than saying nothing.
    (addresses.len() == 1).then(|| addresses[0].clone())
}

fn routed_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_unspecified()).then(|| address.to_string())
}

/// Every address this host actually has, loopback and link-local excluded.
///
/// The derivation above picks the interface the *default route* uses, which is right on
/// a host with one address and a coin toss on a NAS with two NICs or a bond. Knowing
/// what else is available is what turns "this might be wrong" into something an
/// operator can act on without going to look — and looking is the step nobody takes
/// before a rack is already failing to boot.
pub fn local_addresses() -> Vec<String> {
    #[cfg(not(unix))]
    {
        Vec::new()
    }
    #[cfg(unix)]
    {
        use std::net::{Ipv4Addr, Ipv6Addr};

        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if unsafe { libc::getifaddrs(&mut list) } != 0 {
            return Vec::new();
        }
        let mut found: Vec<String> = Vec::new();
        let mut node = list;
        while !node.is_null() {
            let entry = unsafe { &*node };
            node = entry.ifa_next;
            if entry.ifa_addr.is_null() {
                continue;
            }
            let family = unsafe { (*entry.ifa_addr).sa_family } as i32;
            let address = if family == libc::AF_INET {
                let raw = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
                let octets = u32::from_be(raw.sin_addr.s_addr);
                let v4 = Ipv4Addr::from(octets);
                (!v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified())
                    .then(|| v4.to_string())
            } else if family == libc::AF_INET6 {
                let raw = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in6) };
                let v6 = Ipv6Addr::from(raw.sin6_addr.s6_addr);
                // No link-local: an fe80:: address needs a scope to be usable, and a
                // scope is not something that survives being written into a script.
                let link_local = v6.segments()[0] & 0xffc0 == 0xfe80;
                (!v6.is_loopback() && !link_local && !v6.is_unspecified()).then(|| v6.to_string())
            } else {
                None
            };
            if let Some(address) = address
                && !found.contains(&address)
            {
                found.push(address);
            }
        }
        unsafe { libc::freeifaddrs(list) };
        found
    }
}

/// One configuration variable, **described** rather than merely read.
///
/// `from_lookup` above knows how to interpret each of these. This table is what anything
/// that has to *present* one needs instead: its default, whether printing it would hand
/// out a credential, and a line saying what it does. They sit in the same file so that
/// adding a variable to one and forgetting the other is a test failure rather than a
/// setting nobody can see.
pub struct Known {
    pub key: &'static str,
    /// What is in force when nothing sets it, written the way the file would write it.
    /// `None` means the feature is simply off until someone turns it on.
    pub default: Option<&'static str>,
    /// A credential: its value is never printed, and never leaves this process.
    pub secret: bool,
    /// One line, for whatever has to label the field.
    pub help: &'static str,
}

/// Every variable, in the order a person would want to meet them: what answers come
/// from, where the server listens, how much it says, then the two credentials.
pub const KNOWN: [Known; 29] = [
    Known {
        key: "RESCRIPTUM_STORE",
        default: Some("files"),
        secret: false,
        help: "Where answers come from: a directory of documents, or a database.",
    },
    Known {
        key: "RESCRIPTUM_ANSWERS_DIR",
        default: Some(DEFAULT_ANSWERS_DIR),
        secret: false,
        help: "The directory of answer documents, when the store is files.",
    },
    Known {
        key: "RESCRIPTUM_DB_PATH",
        default: Some(DEFAULT_DB_PATH),
        secret: false,
        help: "The SQLite database, when the store is sqlite.",
    },
    Known {
        key: "RESCRIPTUM_LISTEN_ADDR",
        default: Some(DEFAULT_LISTEN_ADDR),
        secret: false,
        help: "Where installers reach the answer endpoint.",
    },
    Known {
        // The only default that is not a constant: it is this machine's CPU count, so
        // `settings` fills it in rather than the table claiming a number it cannot know.
        key: "RESCRIPTUM_WORKERS",
        default: None,
        secret: false,
        help: "Runtime threads. Not a concurrency limit; the default is the CPU count.",
    },
    Known {
        key: "RESCRIPTUM_MAX_CONNECTIONS",
        default: Some("2048"),
        secret: false,
        help: "In-flight connections before a burst is shed with 503 rather than queued.",
    },
    Known {
        key: "RESCRIPTUM_TIMEOUT_SECS",
        default: Some("10"),
        secret: false,
        help: "Header-read timeout, and the whole-connection deadline.",
    },
    Known {
        key: "RESCRIPTUM_LOG",
        default: Some("all"),
        secret: false,
        help: "all, problems (drops the requests that worked), or off.",
    },
    Known {
        key: "RESCRIPTUM_LOG_FILE",
        default: None,
        secret: false,
        help: "A file to append to, or stdout or stderr. Unset means stderr.",
    },
    Known {
        key: "RESCRIPTUM_CAPTURE_DIR",
        default: None,
        secret: false,
        help: "Record what installers actually send, for when nothing is answered.",
    },
    Known {
        key: "RESCRIPTUM_ANSWER_TOKEN",
        default: None,
        secret: true,
        help: "Required of installers, when they have one to offer. Off by default.",
    },
    Known {
        key: "RESCRIPTUM_ADMIN_ADDR",
        default: None,
        secret: false,
        help: "The write API's own listener. Off unless set; keep it on loopback.",
    },
    Known {
        key: "RESCRIPTUM_ADMIN_TOKEN",
        default: None,
        secret: true,
        help: "Bearer token for the write API. At least 16 characters, and required.",
    },
    Known {
        key: "RESCRIPTUM_PUBLIC_HOST",
        // Not a constant: the default is this host's own LAN address, which is only
        // knowable at runtime. `settings()` fills it in, the way it does the CPU count.
        default: None,
        secret: false,
        help: "The host this server names itself by. A host, never a URL. \
               Unset, the address of the interface that reaches the network is used.",
    },
    Known {
        key: "RESCRIPTUM_MEDIA_DIR",
        default: None,
        secret: false,
        help: "Installer images. Unset means no media and no media listener.",
    },
    Known {
        key: "RESCRIPTUM_MEDIA_ADDR",
        default: Some(DEFAULT_MEDIA_ADDR),
        secret: false,
        help: "The media listener, when there is a media directory. Loaders assume 8001.",
    },
    Known {
        key: "RESCRIPTUM_MEDIA_TIMEOUT_SECS",
        default: Some("600"),
        secret: false,
        help: "Whole-transfer deadline for a download. Not the answer listener's 10.",
    },
    Known {
        key: "RESCRIPTUM_MEDIA_MAX_CONNECTIONS",
        default: Some("16"),
        secret: false,
        help: "Concurrent transfers, low on purpose: each holds its permit for minutes.",
    },
    Known {
        key: "RESCRIPTUM_BOOT_ALLOW",
        default: None,
        secret: false,
        help: "Client CIDRs allowed to fetch boot media. Unset means anyone who can reach it.",
    },
    Known {
        key: "RESCRIPTUM_BOOT_DIR",
        default: None,
        secret: false,
        help: "Loaders and menus, handed out over TFTP. Unset means no TFTP at all.",
    },
    Known {
        key: "RESCRIPTUM_TFTP_ADDR",
        default: Some(DEFAULT_TFTP_ADDR),
        secret: false,
        help: "The TFTP listener, or `off` for none. Port 69 is privileged; see RESCRIPTUM_USER.",
    },
    Known {
        key: "RESCRIPTUM_BOOT_TIMEOUT_SECS",
        default: Some("15"),
        secret: false,
        help: "Seconds before the menu falls through to local disk. Rendered as milliseconds.",
    },
    Known {
        key: "RESCRIPTUM_INSTALLED_TOKEN",
        default: None,
        secret: true,
        help: "Proxmox's post-installation-webhook token. Set it and POST /installed exists, which drops a machine's install claim when it reports success. Unset, there is no endpoint.",
    },
    Known {
        key: "RESCRIPTUM_TFTP_BLKSIZE",
        default: Some("1468"),
        secret: false,
        help: "The largest TFTP block to agree to. 1468 fills a 1500-byte path exactly; lower it (1400, or 512) if a boot stalls at the first block, which is what a VLAN tag or a tunnel does to it.",
    },
    Known {
        key: "RESCRIPTUM_BOOT_UNCLAIMED",
        default: Some("menu"),
        secret: false,
        help: "What a machine no answer claims gets: `menu`, or `local` to send it straight to its own disk.",
    },
    Known {
        key: "RESCRIPTUM_BOOT_LOGO",
        default: None,
        secret: false,
        help: "A PNG to show behind the menu, replacing the built-in one.",
    },
    Known {
        key: "RESCRIPTUM_BOOT_TITLE",
        default: None,
        secret: false,
        help: "The menu's title bar, replacing the built-in one.",
    },
    Known {
        key: "RESCRIPTUM_USER",
        default: None,
        secret: false,
        help: "Drop to this user after binding. Binding first is the point.",
    },
    Known {
        key: "RESCRIPTUM_GROUP",
        default: None,
        secret: false,
        help: "Drop to this group after binding.",
    },
];

/// Which of the three places a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The process environment, which **wins over the file**.
    Environment,
    /// The file `RESCRIPTUM_ENV_FILE` names.
    File,
    /// Nothing set it.
    Default,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Environment => "environment",
            Source::File => "file",
            Source::Default => "default",
        }
    }
}

/// A variable as it currently stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub key: &'static str,
    /// What is in force. **Always `None` for a secret** — this type does not carry one.
    pub value: Option<String>,
    /// Whether anything is in force at all. For a secret this is the whole story.
    pub set: bool,
    pub source: Source,
    pub default: Option<String>,
    pub secret: bool,
    pub help: &'static str,
}

/// Describe every variable: what is in force, and **which of the file and the environment
/// put it there**.
///
/// That distinction is the entire reason this returns a source rather than a map. The
/// file supplies defaults and the real environment wins, so anything offering to edit the
/// file has to know when doing so would change nothing — and say so, rather than write a
/// value the running server will keep ignoring.
///
/// Empty counts as unset in both places, exactly as `from_lookup` treats it: an
/// exported-but-empty variable is a mistake, not an instruction.
pub fn settings(
    file: Option<&crate::envfile::EnvFile>,
    env: impl Fn(&str) -> Option<String>,
) -> Vec<Setting> {
    let useful = |v: String| -> Option<String> {
        let v = v.trim().to_string();
        (!v.is_empty()).then_some(v)
    };

    KNOWN
        .iter()
        .map(|known| {
            let from_env = env(known.key).and_then(useful);
            let from_file = file.and_then(|f| f.get(known.key)).and_then(useful);

            let source = if from_env.is_some() {
                Source::Environment
            } else if from_file.is_some() {
                Source::File
            } else {
                Source::Default
            };

            let default = match known.key {
                "RESCRIPTUM_WORKERS" => Some(default_workers().to_string()),
                // **The other default that cannot be a constant.** The server derives
                // this at startup, so a panel showing an empty field would be showing
                // something other than what the server will use — and the operator has
                // no way to tell whether the guess is right without reading a log.
                "RESCRIPTUM_PUBLIC_HOST" => derive_public_host(),
                _ => known.default.map(str::to_string),
            };
            let value = from_env.or(from_file).or_else(|| default.clone());

            Setting {
                key: known.key,
                set: value.is_some(),
                value: if known.secret { None } else { value },
                source,
                default,
                secret: known.secret,
                help: known.help,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn defaults_when_unset() {
        let c = Config::from_lookup(lookup(&[]));
        assert_eq!(c.answers_dir, PathBuf::from(DEFAULT_ANSWERS_DIR));
        assert_eq!(c.listen_addr, DEFAULT_LISTEN_ADDR);

        // Pinned on purpose. These were `/volume1/netboot/...` once, which exists on one
        // vendor's appliance and nowhere else, so the default was wrong on every other
        // host. A default has to be plausible everywhere or it is not a default.
        assert_eq!(DEFAULT_ANSWERS_DIR, "/srv/answers");
        assert_eq!(DEFAULT_DB_PATH, "/srv/answers.db");
    }

    #[test]
    fn environment_overrides_defaults() {
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_ANSWERS_DIR", "/srv/answers"),
            ("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:9000"),
        ]));
        assert_eq!(c.answers_dir, PathBuf::from("/srv/answers"));
        assert_eq!(c.listen_addr, "127.0.0.1:9000");
    }

    #[test]
    fn empty_or_whitespace_values_fall_back_to_defaults() {
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_ANSWERS_DIR", ""),
            ("RESCRIPTUM_LISTEN_ADDR", "   "),
        ]));
        assert_eq!(c.answers_dir, PathBuf::from(DEFAULT_ANSWERS_DIR));
        assert_eq!(c.listen_addr, DEFAULT_LISTEN_ADDR);
    }

    fn admin_cfg(pairs: &'static [(&'static str, &'static str)]) -> Config {
        Config::from_lookup(lookup(pairs))
    }

    #[test]
    fn the_admin_api_is_off_unless_an_address_is_given() {
        let c = admin_cfg(&[]);
        assert_eq!(c.admin_addr, None);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn the_admin_api_refuses_to_run_without_a_token() {
        let c = admin_cfg(&[
            ("RESCRIPTUM_ADMIN_ADDR", "127.0.0.1:8001"),
            ("RESCRIPTUM_STORE", "sqlite"),
        ]);
        let err = c.validate().expect_err("must refuse");
        assert!(err.contains("RESCRIPTUM_ADMIN_TOKEN"), "{err}");
    }

    #[test]
    fn the_admin_api_refuses_a_short_token() {
        let c = admin_cfg(&[
            ("RESCRIPTUM_ADMIN_ADDR", "127.0.0.1:8001"),
            ("RESCRIPTUM_STORE", "sqlite"),
            ("RESCRIPTUM_ADMIN_TOKEN", "short"),
        ]);
        assert!(c.validate().is_err());
    }

    #[test]
    fn the_admin_api_refuses_to_run_over_the_file_store() {
        let c = admin_cfg(&[
            ("RESCRIPTUM_ADMIN_ADDR", "127.0.0.1:8001"),
            ("RESCRIPTUM_ADMIN_TOKEN", "0123456789abcdef0"),
        ]);
        let err = c.validate().expect_err("must refuse");
        assert!(err.contains("sqlite"), "{err}");
    }

    #[test]
    fn a_valid_admin_configuration_passes() {
        let c = admin_cfg(&[
            ("RESCRIPTUM_ADMIN_ADDR", "127.0.0.1:8001"),
            ("RESCRIPTUM_STORE", "sqlite"),
            ("RESCRIPTUM_ADMIN_TOKEN", "0123456789abcdef0"),
        ]);
        assert!(c.validate().is_ok());
        assert!(!c.admin_is_exposed());
    }

    #[test]
    fn binding_the_admin_api_beyond_loopback_is_noticed() {
        assert!(admin_cfg(&[("RESCRIPTUM_ADMIN_ADDR", "0.0.0.0:8001")]).admin_is_exposed());
        assert!(admin_cfg(&[("RESCRIPTUM_ADMIN_ADDR", "10.0.0.5:8001")]).admin_is_exposed());
        assert!(!admin_cfg(&[("RESCRIPTUM_ADMIN_ADDR", "[::1]:8001")]).admin_is_exposed());
    }

    #[test]
    fn the_answer_endpoint_is_open_unless_a_token_is_set() {
        // Most installers cannot authenticate; refusing them would refuse the install.
        assert_eq!(admin_cfg(&[]).answer_token, None);
        assert!(admin_cfg(&[]).validate().is_ok());
    }

    #[test]
    fn a_short_answer_token_warns_rather_than_refusing_to_start() {
        let c = admin_cfg(&[("RESCRIPTUM_ANSWER_TOKEN", "short")]);
        assert!(c.validate().is_ok(), "must still start");
        assert!(c.answer_token_warning().is_some());
        assert!(
            admin_cfg(&[("RESCRIPTUM_ANSWER_TOKEN", "0123456789abcdef0")])
                .answer_token_warning()
                .is_none()
        );
    }

    #[test]
    fn logging_defaults_to_everything_on_stderr() {
        let c = Config::from_lookup(lookup(&[]));
        assert_eq!(c.log_level, crate::log::Level::All);
        assert!(c.log_file.is_none());
    }

    #[test]
    fn the_log_level_is_read_from_the_environment() {
        for (value, expected) in [
            ("all", crate::log::Level::All),
            ("problems", crate::log::Level::Problems),
            ("off", crate::log::Level::Off),
            // A typo must not silently turn logging off.
            ("verbose", crate::log::Level::All),
            ("", crate::log::Level::All),
        ] {
            // `lookup` wants a 'static slice, which a loop variable is not.
            let c = Config::from_lookup(|key| (key == "RESCRIPTUM_LOG").then(|| value.to_string()));
            assert_eq!(c.log_level, expected, "RESCRIPTUM_LOG={value:?}");
        }
    }

    #[test]
    fn the_file_store_is_the_default() {
        assert_eq!(Config::from_lookup(lookup(&[])).store, StoreKind::Files);
        assert_eq!(
            Config::from_lookup(lookup(&[("RESCRIPTUM_STORE", "sqlite")])).store,
            StoreKind::Sqlite
        );
    }

    #[test]
    fn an_unknown_store_falls_back_to_files() {
        // Better a loud default than serving from somewhere nobody meant.
        assert_eq!(
            Config::from_lookup(lookup(&[("RESCRIPTUM_STORE", "postgres")])).store,
            StoreKind::Files
        );
    }

    #[test]
    fn tuning_knobs_default_sensibly() {
        let c = Config::from_lookup(lookup(&[]));
        assert!(c.workers >= 1);
        assert_eq!(c.max_connections, DEFAULT_MAX_CONNECTIONS);
        assert_eq!(c.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    }

    #[test]
    fn tuning_knobs_can_be_overridden() {
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_WORKERS", "8"),
            ("RESCRIPTUM_MAX_CONNECTIONS", "64"),
            ("RESCRIPTUM_TIMEOUT_SECS", "3"),
        ]));
        assert_eq!(c.workers, 8);
        assert_eq!(c.max_connections, 64);
        assert_eq!(c.timeout, Duration::from_secs(3));
    }

    #[test]
    fn zero_or_garbage_tuning_values_fall_back() {
        // Zero runtime threads would start a server that accepts and never answers.
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_WORKERS", "0"),
            ("RESCRIPTUM_MAX_CONNECTIONS", "lots"),
        ]));
        assert!(c.workers >= 1);
        assert_eq!(c.max_connections, DEFAULT_MAX_CONNECTIONS);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let c = Config::from_lookup(lookup(&[("RESCRIPTUM_LISTEN_ADDR", "  0.0.0.0:8080 ")]));
        assert_eq!(c.listen_addr, "0.0.0.0:8080");
    }

    // ---- media and the public host ---------------------------------------

    #[test]
    fn media_is_off_until_a_directory_is_named() {
        // Nothing changes for an existing deployment: no directory, no listener.
        let c = Config::from_lookup(lookup(&[]));
        assert_eq!(c.media_dir, None);
        assert_eq!(c.media_addr, None);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn a_boot_directory_alone_still_starts_tftp() {
        // The default has not moved: `off` is opt-in, and a plain Linux host that names
        // a boot directory gets the TFTP server the plan calls core.
        let c = Config::from_lookup(lookup(&[("RESCRIPTUM_BOOT_DIR", "/srv/boot")]));
        assert_eq!(c.tftp_addr().as_deref(), Some("0.0.0.0:69"));
        assert!(!c.tftp_is_off());
    }

    #[test]
    fn the_media_listener_defaults_to_the_port_the_loaders_assume() {
        let c = Config::from_lookup(lookup(&[("RESCRIPTUM_MEDIA_DIR", "/srv/media")]));
        assert_eq!(c.media_addr(), "0.0.0.0:8001");
        // Pinned deliberately. The loader we ship embeds a script that chains to
        // `${next-server}:8001` before any deployment exists, so this is a contract in
        // the same way an answer URL baked into an ISO is.
        assert_eq!(DEFAULT_MEDIA_ADDR, "0.0.0.0:8001");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn an_address_with_nothing_to_serve_is_refused() {
        let c = Config::from_lookup(lookup(&[("RESCRIPTUM_MEDIA_ADDR", "0.0.0.0:8001")]));
        let e = c.validate().expect_err("must refuse");
        assert!(e.contains("RESCRIPTUM_MEDIA_DIR"), "{e}");
    }

    #[test]
    fn two_listeners_on_one_port_are_refused_rather_than_raced() {
        // The second bind loses, and which one that is depends on start order.
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_MEDIA_DIR", "/srv/media"),
            ("RESCRIPTUM_MEDIA_ADDR", "0.0.0.0:8000"),
        ]));
        let e = c.validate().expect_err("must refuse");
        assert!(e.contains("RESCRIPTUM_LISTEN_ADDR"), "{e}");

        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_MEDIA_DIR", "/srv/media"),
            ("RESCRIPTUM_MEDIA_ADDR", "127.0.0.1:8001"),
            ("RESCRIPTUM_ADMIN_ADDR", "127.0.0.1:8001"),
            ("RESCRIPTUM_STORE", "sqlite"),
            ("RESCRIPTUM_ADMIN_TOKEN", "0123456789abcdef0"),
        ]));
        let e = c.validate().expect_err("must refuse");
        assert!(e.contains("RESCRIPTUM_ADMIN_ADDR"), "{e}");
    }

    #[test]
    fn tftp_can_be_turned_off_without_giving_up_the_boot_directory() {
        // **Off is a deployment workaround, never a packaged default.** It is how an
        // operator says another daemon on this host hands the loader over while
        // rescriptum serves the rest of the chain. rescriptum *is* the TFTP server; a
        // package that shipped with this set would have traded away the thing it is for.
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_BOOT_DIR", "/srv/boot"),
            ("RESCRIPTUM_TFTP_ADDR", "off"),
        ]));
        assert!(c.validate().is_ok(), "{:?}", c.validate());
        assert_eq!(c.tftp_addr(), None, "no listener");
        assert!(
            c.tftp_is_off(),
            "and deliberately so, not merely unasked for"
        );
        // The directory is still configured, so /boot/ and `boot check` still work.
        assert_eq!(c.boot_dir, Some(PathBuf::from("/srv/boot")));
    }

    #[test]
    fn off_is_spelled_the_way_the_log_level_spells_it() {
        for value in ["off", "OFF", "none", "disabled", " off "] {
            let c = Config::from_lookup(|key| {
                (key == "RESCRIPTUM_TFTP_ADDR").then(|| value.to_string())
            });
            assert_eq!(c.tftp_addr(), None, "{value:?}");
        }
        // And an address is still an address.
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_BOOT_DIR", "/srv/boot"),
            ("RESCRIPTUM_TFTP_ADDR", "0.0.0.0:6969"),
        ]));
        assert_eq!(c.tftp_addr().as_deref(), Some("0.0.0.0:6969"));
        assert!(!c.tftp_is_off());
    }

    #[test]
    fn turning_tftp_off_needs_no_boot_directory_to_justify_it() {
        // Off is off: refusing this would be refusing somebody who said "definitely not"
        // before they said where anything lives.
        let c = Config::from_lookup(lookup(&[("RESCRIPTUM_TFTP_ADDR", "off")]));
        assert!(c.validate().is_ok(), "{:?}", c.validate());
        // But naming a real address with nowhere to serve from is still refused.
        let c = Config::from_lookup(lookup(&[("RESCRIPTUM_TFTP_ADDR", "0.0.0.0:69")]));
        assert!(c.validate().is_err());
    }

    #[test]
    fn two_ephemeral_ports_are_not_a_collision() {
        // `:0` asks the kernel for any free port, so two of them are never the same
        // port. Refusing them would refuse the one configuration that cannot collide —
        // and it is the one every integration test uses.
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_MEDIA_DIR", "/srv/media"),
            ("RESCRIPTUM_MEDIA_ADDR", "127.0.0.1:0"),
            ("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:0"),
        ]));
        assert!(c.validate().is_ok(), "{:?}", c.validate());
    }

    #[test]
    fn the_public_host_refuses_to_be_a_url() {
        // It is written into URLs for *two* listeners plus a bare address in a DHCP
        // snippet. A value carrying one port would pin every generated script to one
        // listener, and the symptom is a machine chaining into nowhere.
        for (value, wrong) in [
            ("http://192.0.2.10", "a scheme"),
            ("192.0.2.10:8001", "a port"),
            ("192.0.2.10/boot", "a path"),
        ] {
            let c = Config::from_lookup(|key| {
                (key == "RESCRIPTUM_PUBLIC_HOST").then(|| value.to_string())
            });
            let e = c.validate().expect_err("must refuse {value}");
            assert!(e.contains(wrong), "{value}: {e}");
        }
    }

    #[test]
    fn a_plain_host_or_an_ipv6_literal_is_accepted() {
        for value in [
            "192.0.2.10",
            "boot.example.com",
            "[2001:db8::1]",
            "2001:db8::1",
        ] {
            let c = Config::from_lookup(|key| {
                (key == "RESCRIPTUM_PUBLIC_HOST").then(|| value.to_string())
            });
            assert!(c.validate().is_ok(), "{value} must be accepted");
            assert_eq!(c.public_host(), (value.to_string(), false));
        }
    }

    #[test]
    fn the_settings_table_shows_the_address_the_server_would_actually_use() {
        // **A panel with an empty field here is showing something other than what the
        // server does.** The value is derived at startup, so the table has to derive it
        // too — the same treatment the CPU count already gets, and for the same reason.
        let s = settings(None, |_| None);
        let host = setting(&s, "RESCRIPTUM_PUBLIC_HOST");
        assert!(host.set, "a derived value is still a value in force");
        assert_eq!(
            host.value, host.default,
            "unset means the derived default is what is in force"
        );
        assert_eq!(
            host.value,
            Config::from_lookup(|_| None).public_host().0.into(),
            "and it is the same address the server itself would pick"
        );
    }

    #[test]
    fn without_a_default_route_a_single_interface_still_answers() {
        // An isolated provisioning segment has no default route, which is exactly the
        // network this server is most often put on. One address there is not a guess.
        let one = vec!["10.0.0.4".to_string()];
        assert_eq!(choose_host(None, &one), Some("10.0.0.4".to_string()));

        // Two, and there is a real choice — one that only the operator can make.
        let two = vec!["10.0.0.4".to_string(), "192.168.1.4".to_string()];
        assert_eq!(choose_host(None, &two), None);
        assert_eq!(choose_host(None, &[]), None);

        // A route beats the interface list even when the list is unambiguous: the
        // kernel knows which way traffic actually leaves.
        assert_eq!(
            choose_host(Some("172.16.0.9".to_string()), &two),
            Some("172.16.0.9".to_string())
        );
    }

    #[test]
    fn the_host_knows_what_addresses_it_has() {
        // Loopback and link-local are excluded: the first is not reachable from a
        // machine, and the second needs a scope that does not survive being written
        // into a script.
        let addresses = local_addresses();
        for address in &addresses {
            assert!(!address.starts_with("127."), "{address} is loopback");
            assert!(!address.starts_with("169.254."), "{address} is link-local");
            assert!(!address.starts_with("fe80:"), "{address} is link-local");
            assert_ne!(address, "::1");
        }
        // The derived host, when there is one, is one of them — it is chosen from this
        // set by the routing table rather than invented.
        if let Some(derived) = derive_public_host()
            && !addresses.is_empty()
        {
            assert!(
                addresses.contains(&derived),
                "derived {derived} is not among {addresses:?}"
            );
        }
    }

    #[test]
    fn a_derived_public_host_is_reported_as_derived() {
        // Derivation is wrong often enough on multi-homed and NAT hosts that it is a
        // warning rather than a silent success — the flag is what makes it sayable.
        let (_host, derived) = Config::from_lookup(lookup(&[])).public_host();
        assert!(derived);
    }

    #[test]
    #[cfg(feature = "boot")]
    fn each_generated_url_carries_its_own_listeners_port() {
        // The whole reason the variable is a host: one value, two listeners.
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_PUBLIC_HOST", "192.0.2.10"),
            ("RESCRIPTUM_MEDIA_DIR", "/srv/media"),
        ]));
        let endpoints = c.endpoints();
        assert_eq!(endpoints.answer, "http://192.0.2.10:8000");
        assert_eq!(endpoints.media, "http://192.0.2.10:8001");
    }

    #[test]
    #[cfg(feature = "boot")]
    fn an_ipv6_host_is_bracketed_before_a_port_is_appended() {
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_PUBLIC_HOST", "2001:db8::1"),
            ("RESCRIPTUM_MEDIA_DIR", "/srv/media"),
        ]));
        assert_eq!(c.endpoints().media, "http://[2001:db8::1]:8001");
    }

    #[test]
    fn media_tuning_falls_back_the_way_everything_else_does() {
        let c = Config::from_lookup(lookup(&[
            ("RESCRIPTUM_MEDIA_DIR", "/srv/media"),
            ("RESCRIPTUM_MEDIA_TIMEOUT_SECS", "0"),
            ("RESCRIPTUM_MEDIA_MAX_CONNECTIONS", "plenty"),
        ]));
        assert_eq!(c.media_timeout, Duration::from_secs(600));
        assert_eq!(c.media_max_connections, 16);
        // And the default is deliberately not the answer listener's ten seconds: a
        // 1.5 GB transfer is two minutes on 100 Mbit, and it would be killed mid-flight.
        assert!(c.media_timeout > c.timeout);
    }

    // ---- the described surface -------------------------------------------

    #[test]
    fn every_variable_is_described_exactly_once() {
        // Two lists of the same thing drift. `KNOWN_KEYS` is what reports a typo in the
        // file; `KNOWN` is what a settings panel shows. A variable in one and not the
        // other is either invisible or unwarned about, and neither failure announces
        // itself.
        let described: Vec<&str> = KNOWN.iter().map(|k| k.key).collect();
        let mut sorted = described.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), described.len(), "a key is described twice");

        for key in crate::envfile::KNOWN_KEYS {
            assert!(described.contains(&key), "{key} is read but not described");
        }
        for key in described {
            assert!(
                crate::envfile::KNOWN_KEYS.contains(&key),
                "{key} is described but not read"
            );
        }
    }

    /// An `EnvFile` can only be loaded from a real file, which is the point of it.
    fn env_file(name: &str, body: &str) -> (std::path::PathBuf, crate::envfile::EnvFile) {
        let dir = std::env::temp_dir().join(format!("pve-settings-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rescriptum.env");
        std::fs::write(&path, body).unwrap();
        let file = crate::envfile::EnvFile::load(&path).expect("parses");
        (dir, file)
    }

    fn setting<'a>(settings: &'a [Setting], key: &str) -> &'a Setting {
        settings.iter().find(|s| s.key == key).expect("described")
    }

    #[test]
    fn the_environment_is_reported_as_beating_the_file() {
        // The file is defaults. Offering to edit a value the environment is overriding
        // would be offering to change nothing, which is worse than refusing.
        let (dir, file) = env_file(
            "override",
            "RESCRIPTUM_LISTEN_ADDR=0.0.0.0:8000\nRESCRIPTUM_LOG=problems\n",
        );
        let s = settings(Some(&file), |k| {
            (k == "RESCRIPTUM_LISTEN_ADDR").then(|| "127.0.0.1:9999".to_string())
        });

        let addr = setting(&s, "RESCRIPTUM_LISTEN_ADDR");
        assert_eq!(addr.source, Source::Environment);
        assert_eq!(addr.value.as_deref(), Some("127.0.0.1:9999"));

        let log = setting(&s, "RESCRIPTUM_LOG");
        assert_eq!(log.source, Source::File);
        assert_eq!(log.value.as_deref(), Some("problems"));

        let store = setting(&s, "RESCRIPTUM_STORE");
        assert_eq!(store.source, Source::Default);
        assert_eq!(store.value.as_deref(), Some("files"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_secret_reports_that_it_is_set_and_nothing_else() {
        // This is the whole reason `value` is separate from `set`: a settings panel has
        // to show that a token exists without ever being handed one.
        let (dir, file) = env_file("secret", "RESCRIPTUM_ADMIN_TOKEN=0123456789abcdef0\n");
        let s = settings(Some(&file), |_| None);

        let token = setting(&s, "RESCRIPTUM_ADMIN_TOKEN");
        assert!(token.secret);
        assert!(token.set, "it is set");
        assert_eq!(token.value, None, "a secret's value must never be carried");
        assert_eq!(token.source, Source::File);

        let unset = setting(&s, "RESCRIPTUM_ANSWER_TOKEN");
        assert!(!unset.set);
        assert_eq!(unset.source, Source::Default);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_value_counts_as_unset_in_both_places() {
        // Matching `from_lookup`, where an exported-but-empty variable is a mistake
        // rather than an instruction — otherwise the panel and the server would disagree
        // about what is in force.
        let (dir, file) = env_file("empty", "RESCRIPTUM_LOG=\n");
        let s = settings(Some(&file), |k| {
            (k == "RESCRIPTUM_STORE").then(|| "   ".to_string())
        });

        assert_eq!(setting(&s, "RESCRIPTUM_LOG").source, Source::Default);
        assert_eq!(setting(&s, "RESCRIPTUM_LOG").value.as_deref(), Some("all"));
        assert_eq!(setting(&s, "RESCRIPTUM_STORE").source, Source::Default);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_one_default_that_is_not_a_constant_is_filled_in() {
        // The CPU count is this machine's, so the table cannot hold it and `settings`
        // has to. A panel showing "default: (none)" for workers would be wrong.
        let s = settings(None, |_| None);
        let workers = setting(&s, "RESCRIPTUM_WORKERS");
        assert_eq!(workers.default, Some(default_workers().to_string()));
        assert_eq!(workers.value, Some(default_workers().to_string()));
        assert!(workers.set);
    }
}
