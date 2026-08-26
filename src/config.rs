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
pub const KNOWN: [Known; 13] = [
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
