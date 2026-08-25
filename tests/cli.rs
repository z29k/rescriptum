//! `render`, `check`, `import` and `export`, against the real binary.
//!
//! These are not conveniences. `check` is what `deploy.sh` runs before it ships anything,
//! and what the documentation tells people to put in CI — so its **exit code** is
//! load-bearing, and nothing else pins it. `render` is the only way to see a composed
//! answer before a rack does; the stdout/stderr split is what makes `render … > answer`
//! usable, so that is a contract too.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A scratch answers directory, cleaned up on drop.
struct Case {
    dir: PathBuf,
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Case {
    fn new(files: &[(&str, &str)]) -> Case {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pve-cli-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        let case = Case { dir };
        case.write(files);
        case
    }

    fn write(&self, files: &[(&str, &str)]) {
        for (name, contents) in files {
            let path = self.dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("subdirectory");
            }
            fs::write(&path, contents).expect("fixture");
        }
    }

    fn run(&self, args: &[&str]) -> Run {
        self.run_env(&[("RESCRIPTUM_ANSWERS_DIR", self.dir.as_path())], args)
    }

    fn run_env(&self, env: &[(&str, &Path)], args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
        for (key, value) in env {
            cmd.env(key, value);
        }
        Run::from(cmd.args(args).output().expect("run rescriptum"))
    }
}

struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl From<Output> for Run {
    fn from(out: Output) -> Run {
        Run {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

impl std::fmt::Display for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ok={}\n--- stdout ---\n{}--- stderr ---\n{}",
            self.ok, self.stdout, self.stderr
        )
    }
}

const RACK: &str = "\
members = [\"98:fa:9b:50:d8:10\", \"98:fa:9b:50:d8:11\"]

[global]
keyboard = \"fr\"

[disk-setup]
filesystem = \"zfs\"
";

// ---- render ---------------------------------------------------------------

#[test]
fn render_puts_the_document_on_stdout_and_the_provenance_on_stderr() {
    // `render <mac> > answer.toml` has to yield a usable document, so the line
    // explaining how it was reached must not be part of it.
    let c = Case::new(&[
        ("groups/rack-a.toml", RACK),
        ("98-fa-9b-50-d8-10.toml", "[global]\nfqdn = \"node01\"\n"),
    ]);

    let r = c.run(&["render", "98:fa:9b:50:d8:10"]);
    assert!(r.ok, "{r}");

    // The merge really happened: the group underneath, the machine on top.
    assert!(r.stdout.contains("keyboard = \"fr\""), "{r}");
    assert!(r.stdout.contains("fqdn = \"node01\""), "{r}");
    assert!(
        !r.stdout.contains("members"),
        "control keys must be stripped\n{r}"
    );
    assert!(
        !r.stdout.contains('#'),
        "provenance must not be on stdout\n{r}"
    );

    assert!(r.stderr.contains("# format=toml"), "{r}");
    assert!(r.stderr.contains("machine=98-fa-9b-50-d8-10"), "{r}");
    assert!(r.stderr.contains("group=rack-a"), "{r}");
}

#[test]
fn render_by_query_supplies_the_labels_a_selector_needs() {
    // A bare identifier fills the haystack and nothing else, so a selector on `serial`
    // has nothing to test until --query gives it one.
    let c = Case::new(&[(
        "groups/by-serial.toml",
        "[match]\nserial = \"7ABC*\"\n\n[global]\nmarker = \"selected\"\n",
    )]);

    let bare = c.run(&["render", "98:fa:9b:50:d8:10"]);
    assert!(
        !bare.ok,
        "an identity alone must not satisfy a selector\n{bare}"
    );

    let r = c.run(&["render", "--query", "serial=7ABC123"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("selected"), "{r}");
    assert!(r.stderr.contains("group=by-serial"), "{r}");
}

#[test]
fn render_with_a_path_is_constrained_the_way_the_real_url_would_be() {
    // Without `path=`, resolution is unconstrained by format and may pick a document
    // the real endpoint would have excluded — which is exactly the bug being chased.
    let c = Case::new(&[
        ("98fa9b50d810.toml", "marker = \"as-proxmox\"\n"),
        ("98fa9b50d810.ks", "# kickstart\nlang fr_FR\n"),
    ]);

    let ks = c.run(&["render", "--query", "path=/rhel/ks&mac=98:fa:9b:50:d8:10"]);
    assert!(ks.ok, "{ks}");
    assert!(ks.stdout.contains("lang fr_FR"), "{ks}");
    assert!(ks.stderr.contains("format=text"), "{ks}");

    let toml = c.run(&[
        "render",
        "--query",
        "path=/proxmox/answer&mac=98:fa:9b:50:d8:10",
    ]);
    assert!(toml.ok, "{toml}");
    assert!(toml.stdout.contains("as-proxmox"), "{toml}");
    assert!(toml.stderr.contains("format=toml"), "{toml}");
}

#[test]
fn render_replays_a_captured_body() {
    let c = Case::new(&[("98fa9b50d810.toml", "marker = \"replayed\"\n")]);
    let body = c.dir.join("captured.json");
    fs::write(
        &body,
        r#"{"network_interfaces":[{"mac":"98:fa:9b:50:d8:10"}]}"#,
    )
    .unwrap();

    let r = c.run(&["render", "--body", body.to_str().unwrap()]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("replayed"), "{r}");
}

#[test]
fn render_fails_rather_than_printing_something_when_nothing_applies() {
    // Exit status is what a script keys on; printing nothing and succeeding would let a
    // broken pipeline write an empty answer file.
    let c = Case::new(&[("98fa9b50d810.toml", "marker = \"x\"\n")]);
    let r = c.run(&["render", "de:ad:be:ef:00:01"]);
    assert!(!r.ok, "{r}");
    assert!(r.stdout.is_empty(), "{r}");
    assert!(r.stderr.contains("no answer file applies"), "{r}");
}

#[test]
fn render_refuses_a_template_the_request_cannot_fill() {
    let c = Case::new(&[(
        "98fa9b50d810.toml",
        "[global]\nfqdn = \"node-{{ serial }}.example.com\"\n",
    )]);

    let empty = c.run(&["render", "98:fa:9b:50:d8:10"]);
    assert!(!empty.ok, "{empty}");
    assert!(
        empty.stdout.is_empty(),
        "a half-filled answer is worse than none\n{empty}"
    );
    assert!(
        empty.stderr.contains("template needs {{ serial }}"),
        "{empty}"
    );

    // …and succeeds once the fact is there.
    let filled = c.run(&["render", "--query", "mac=98:fa:9b:50:d8:10&serial=7ABC123"]);
    assert!(filled.ok, "{filled}");
    assert!(
        filled.stdout.contains("node-7ABC123.example.com"),
        "{filled}"
    );
}

#[test]
fn render_without_a_usable_argument_prints_its_usage() {
    let c = Case::new(&[]);
    for args in [
        vec!["render"],
        vec!["render", "--body"],
        vec!["render", "--nope"],
    ] {
        let r = c.run(&args);
        assert!(!r.ok, "{args:?}: {r}");
        assert!(
            r.stderr.contains("usage: rescriptum render"),
            "{args:?}: {r}"
        );
    }
}

// ---- check ----------------------------------------------------------------

#[test]
fn check_succeeds_on_a_healthy_set() {
    let c = Case::new(&[
        ("groups/rack-a.toml", RACK),
        ("98-fa-9b-50-d8-10.toml", "[global]\nfqdn = \"node01\"\n"),
        ("default.toml", "[global]\nkeyboard = \"us\"\n"),
    ]);
    let r = c.run(&["check"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("ok — everything renders"), "{r}");
    // The count is of documents that name a machine; `default` is a fallback, not one.
    assert!(r.stdout.contains("1 group(s), 1 machine file(s)"), "{r}");
}

#[test]
fn check_catches_a_broken_default_even_though_it_names_no_machine() {
    // `default` is not rendered by name — it is reached only when nothing else matches
    // — so it would be easy for it to escape the sweep. It must not.
    for broken in ["x = = 1\n", "extends = \"nope\"\n[global]\nx = 1\n"] {
        let c = Case::new(&[("default.toml", broken)]);
        let r = c.run(&["check"]);
        assert!(!r.ok, "{broken:?}: {r}");
        assert!(r.stdout.contains("default.toml"), "{broken:?}: {r}");
    }
}

#[test]
fn check_fails_and_names_what_broke() {
    // The exit code is the contract: deploy.sh refuses to ship on it.
    let c = Case::new(&[(
        "98fa9b50d810.toml",
        "extends = \"nope\"\n\n[global]\nx = 1\n",
    )]);
    let r = c.run(&["check"]);
    assert!(!r.ok, "a broken answer set must not exit 0\n{r}");
    assert!(r.stdout.contains("extends unknown group \"nope\""), "{r}");
    assert!(r.stdout.contains("problem(s)"), "{r}");
    assert!(!r.stdout.contains("ok — everything renders"), "{r}");
}

#[test]
fn check_fails_on_a_document_that_will_not_parse() {
    let c = Case::new(&[("98fa9b50d810.toml", "x = = 1\n")]);
    let r = c.run(&["check"]);
    assert!(!r.ok, "{r}");
    assert!(r.stdout.to_lowercase().contains("toml"), "{r}");
}

#[test]
fn check_fails_on_a_template_it_cannot_fill() {
    // Honest rather than convenient: `check` has no request, so it genuinely cannot
    // prove this answer renders. Saying so beats a green tick.
    let c = Case::new(&[(
        "groups/rack-a.toml",
        "members = [\"98:fa:9b:50:d8:10\"]\n\n[global]\nfqdn = \"node-{{ serial }}\"\n",
    )]);
    let r = c.run(&["check"]);
    assert!(!r.ok, "{r}");
    assert!(r.stdout.contains("template needs {{ serial }}"), "{r}");
}

#[test]
fn check_says_it_could_not_try_a_selector_rather_than_implying_it_did() {
    let c = Case::new(&[(
        "groups/by-serial.toml",
        "[match]\nserial = \"7ABC*\"\n\n[global]\nmarker = \"x\"\n",
    )]);
    let r = c.run(&["check"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("selects on serial=7ABC*"), "{r}");
    assert!(r.stdout.contains("verify with: rescriptum render"), "{r}");
}

#[test]
fn check_flags_a_group_nothing_can_reach() {
    let c = Case::new(&[("groups/orphan.toml", "[global]\nmarker = \"x\"\n")]);
    let r = c.run(&["check"]);
    assert!(r.ok, "an unreachable group is a note, not a failure\n{r}");
    assert!(
        r.stdout.contains("neither members nor a `match` block"),
        "{r}"
    );
}

#[test]
fn check_names_the_formats_it_could_not_schema_check() {
    // A checker that refuses to run without optional tooling is a checker nobody runs,
    // so a missing validator is a note — but it must be said, not silently skipped.
    let c = Case::new(&[("98fa9b50d810.preseed", "d-i marker string x\n")]);
    let r = c.run(&["check"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("preseed"), "{r}");
    assert!(r.stdout.contains("note:"), "{r}");
}

// ---- import / export ------------------------------------------------------

#[test]
fn a_directory_survives_a_round_trip_through_the_database_byte_for_byte() {
    // What makes the database safe to adopt *and* safe to leave. Comments, formatting
    // and all: if this stops being exact, `export` is no longer a way back out.
    let c = Case::new(&[
        (
            "groups/rack-a.toml",
            "# a comment that must survive\nmembers = [\"98:fa:9b:50:d8:10\"]\n\n[global]\nkeyboard = \"fr\"\n",
        ),
        ("98-fa-9b-50-d8-10.toml", "[global]\nfqdn   = \"node01\"\n"),
        (
            "98fa9b50d810.preseed",
            "# answer: extends base\nd-i marker string deb\n",
        ),
        ("groups/base.preseed", "d-i base string yes\n"),
        ("default.toml", "[global]\nkeyboard = \"us\"\n"),
    ]);

    let db = c.dir.join("answers.db");
    let out = c.dir.join("exported");

    let imported = c.run_env(
        &[
            ("RESCRIPTUM_STORE", Path::new("sqlite")),
            ("RESCRIPTUM_DB_PATH", &db),
        ],
        &["import", c.dir.to_str().unwrap()],
    );
    assert!(imported.ok, "{imported}");
    assert!(imported.stdout.contains("group(s)"), "{imported}");

    let exported = c.run_env(
        &[
            ("RESCRIPTUM_STORE", Path::new("sqlite")),
            ("RESCRIPTUM_DB_PATH", &db),
        ],
        &["export", out.to_str().unwrap()],
    );
    assert!(exported.ok, "{exported}");

    for name in [
        "groups/rack-a.toml",
        "groups/base.preseed",
        "98-fa-9b-50-d8-10.toml",
        "98fa9b50d810.preseed",
        "default.toml",
    ] {
        let before = fs::read(c.dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let after = fs::read(out.join(name)).unwrap_or_else(|e| panic!("{name} exported: {e}"));
        assert_eq!(before, after, "{name} changed crossing the database");
    }
}

#[test]
fn the_database_answers_exactly_as_the_directory_did() {
    let c = Case::new(&[
        ("groups/rack-a.toml", RACK),
        ("98-fa-9b-50-d8-10.toml", "[global]\nfqdn = \"node01\"\n"),
    ]);
    let from_files = c.run(&["render", "98:fa:9b:50:d8:10"]);
    assert!(from_files.ok, "{from_files}");

    let db = c.dir.join("answers.db");
    let sqlite = [
        ("RESCRIPTUM_STORE", Path::new("sqlite")),
        ("RESCRIPTUM_DB_PATH", db.as_path()),
    ];
    assert!(c.run_env(&sqlite, &["import", c.dir.to_str().unwrap()]).ok);

    let from_db = c.run_env(&sqlite, &["render", "98:fa:9b:50:d8:10"]);
    assert!(from_db.ok, "{from_db}");
    assert_eq!(
        from_files.stdout, from_db.stdout,
        "the store must not change the answer"
    );
}

#[test]
fn import_and_export_need_a_directory() {
    let c = Case::new(&[]);
    for verb in ["import", "export"] {
        let r = c.run(&[verb]);
        assert!(!r.ok, "{verb}: {r}");
        assert!(r.stderr.contains("usage: rescriptum"), "{verb}: {r}");
    }
}

// ---- argument handling ----------------------------------------------------

#[test]
fn help_and_version_succeed_and_an_unknown_argument_does_not() {
    let c = Case::new(&[]);

    let help = c.run(&["--help"]);
    assert!(help.ok, "{help}");
    assert!(help.stdout.contains("USAGE:"), "{help}");
    assert!(help.stdout.contains("RESCRIPTUM_ANSWERS_DIR"), "{help}");

    let version = c.run(&["--version"]);
    assert!(version.ok, "{version}");
    assert!(version.stdout.starts_with("rescriptum "), "{version}");

    // Not silently ignored: a typo'd subcommand that started a server instead would be
    // a very confusing way to lose an afternoon.
    let bogus = c.run(&["renderr", "98:fa:9b:50:d8:10"]);
    assert!(!bogus.ok, "{bogus}");
    assert!(bogus.stderr.contains("unknown argument"), "{bogus}");
    assert!(bogus.stderr.contains("USAGE:"), "{bogus}");
}

// ---- the env file ---------------------------------------------------------

#[test]
fn an_env_file_supplies_configuration() {
    // systemd has EnvironmentFile= and needs none of this. DSM 7 has no systemd, and its
    // Task Scheduler entry has to source the file with `.` — which fails silently.
    let c = Case::new(&[("groups/rack-a.toml", RACK)]);
    let env = c.dir.join("rescriptum.env");
    fs::write(
        &env,
        format!(
            "# rescriptum\nexport RESCRIPTUM_ANSWERS_DIR={}\nRESCRIPTUM_TIMEOUT_SECS=7\n",
            c.dir.display()
        ),
    )
    .unwrap();

    let r = c.run_env(&[("RESCRIPTUM_ENV_FILE", &env)], &["check"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("1 group(s)"), "{r}");
    assert!(
        r.stderr.contains("reading configuration defaults from"),
        "the file it read must be named in the log\n{r}"
    );
}

#[test]
fn the_real_environment_wins_over_the_env_file() {
    // The file supplies defaults. Something exported deliberately at launch must never be
    // silently overridden by a file someone edited last year.
    let c = Case::new(&[("98fa9b50d810.toml", "marker = \"from-the-flag\"\n")]);
    let elsewhere = c.dir.join("unused");
    fs::create_dir_all(&elsewhere).unwrap();
    let env = c.dir.join("rescriptum.env");
    fs::write(
        &env,
        format!("RESCRIPTUM_ANSWERS_DIR={}\n", elsewhere.display()),
    )
    .unwrap();

    // The file points at an empty directory; the environment points at the real one.
    let r = c.run_env(
        &[
            ("RESCRIPTUM_ENV_FILE", &env),
            ("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path()),
        ],
        &["render", "98:fa:9b:50:d8:10"],
    );
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("from-the-flag"), "{r}");
}

#[test]
fn an_exported_but_empty_variable_still_lets_the_file_apply() {
    // An empty variable is documented as a mistake rather than an instruction, so it does
    // not count as "set in the environment" either.
    let c = Case::new(&[("98fa9b50d810.toml", "marker = \"from-the-file\"\n")]);
    let env = c.dir.join("rescriptum.env");
    fs::write(
        &env,
        format!("RESCRIPTUM_ANSWERS_DIR={}\n", c.dir.display()),
    )
    .unwrap();

    let r = c.run_env(
        &[
            ("RESCRIPTUM_ENV_FILE", &env),
            ("RESCRIPTUM_ANSWERS_DIR", Path::new("   ")),
        ],
        &["render", "98:fa:9b:50:d8:10"],
    );
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("from-the-file"), "{r}");
}

#[test]
fn an_env_file_that_cannot_be_read_refuses_to_start() {
    // The whole point. Carrying on with defaults is what the file exists to prevent: a
    // server that comes up on the wrong answers directory with no admin token, silently.
    let c = Case::new(&[]);
    let r = c.run_env(
        &[("RESCRIPTUM_ENV_FILE", &c.dir.join("absent.env"))],
        &["check"],
    );
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("RESCRIPTUM_ENV_FILE"), "{r}");
    assert!(r.stderr.contains("cannot be read"), "{r}");
}

#[test]
fn a_malformed_env_file_refuses_to_start_and_says_which_line() {
    let c = Case::new(&[]);
    let env = c.dir.join("bad.env");
    for (contents, needle) in [
        ("RESCRIPTUM_STORE sqlite\n", "KEY=value"),
        (
            "RESCRIPTUM_STORE=files\nRESCRIPTUM_STORE=sqlite\n",
            "set twice",
        ),
        ("9NOPE=x\n", "not a usable variable name"),
    ] {
        fs::write(&env, contents).unwrap();
        let r = c.run_env(&[("RESCRIPTUM_ENV_FILE", &env)], &["check"]);
        assert!(!r.ok, "{contents:?}: {r}");
        assert!(r.stderr.contains(needle), "{contents:?}: {r}");
        assert!(r.stderr.contains("line "), "{contents:?}: {r}");
    }
}

#[test]
fn help_still_works_when_the_env_file_is_broken() {
    // `--help` is what you reach for when something is wrong. Refusing to print it
    // because of the very thing you are trying to diagnose would be perverse, so it is
    // answered before the configuration is read at all.
    let c = Case::new(&[]);
    let env = c.dir.join("broken.env");
    fs::write(&env, "this is not an assignment\n").unwrap();

    for arg in ["--help", "--version"] {
        let r = c.run_env(&[("RESCRIPTUM_ENV_FILE", &env)], &[arg]);
        assert!(r.ok, "{arg} must still answer: {r}");
    }

    // Anything that actually needs the configuration still refuses.
    let r = c.run_env(&[("RESCRIPTUM_ENV_FILE", &env)], &["check"]);
    assert!(!r.ok, "{r}");
}

#[test]
fn a_misspelled_variable_in_the_env_file_is_warned_about() {
    // Believing you set a token and not having set one is exactly the failure this is
    // meant to remove, so a key nothing reads has to be said out loud.
    let c = Case::new(&[("groups/rack-a.toml", RACK)]);
    let env = c.dir.join("typo.env");
    fs::write(
        &env,
        format!(
            "RESCRIPTUM_ANSWERS_DIR={}\nRESCRIPTUM_ANSWER_TOKENN=hunter2\n",
            c.dir.display()
        ),
    )
    .unwrap();

    let r = c.run_env(&[("RESCRIPTUM_ENV_FILE", &env)], &["check"]);
    assert!(r.ok, "a typo is a warning, not a refusal\n{r}");
    assert!(r.stderr.contains("RESCRIPTUM_ANSWER_TOKENN"), "{r}");
    assert!(
        !r.stderr.contains("hunter2"),
        "a warning must never print what the file holds\n{r}"
    );
}

#[test]
fn no_env_file_is_ever_discovered_on_its_own() {
    // This binary runs as root. If it picked a file up from whatever directory it was
    // launched in, anyone who could write there would own RESCRIPTUM_ADMIN_TOKEN — and
    // with it the root password of every machine installed afterwards.
    let c = Case::new(&[("98fa9b50d810.toml", "marker = \"real\"\n")]);
    for name in [".env", "rescriptum.env", ".env.local"] {
        fs::write(
            c.dir.join(name),
            "RESCRIPTUM_ANSWERS_DIR=/nonexistent/planted\n",
        )
        .unwrap();
    }

    // Run with the answers directory as the working directory, the way a plant would
    // hope for. Nothing is picked up; only RESCRIPTUM_ENV_FILE can name a file.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
    cmd.env("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path())
        .current_dir(&c.dir);
    let r = Run::from(
        cmd.args(["render", "98:fa:9b:50:d8:10"])
            .output()
            .expect("run"),
    );
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("real"), "{r}");
}

#[test]
fn the_env_file_can_carry_the_logging_variables() {
    // They were added to the known-key list when they were added to the config. If that
    // ever falls out of step, the symptom is silent: a correct file warns that a correct
    // key is a typo, and whoever set it stops trusting the warnings.
    let c = Case::new(&[("groups/rack-a.toml", RACK)]);
    let log = c.dir.join("rescriptum.log");
    let env = c.dir.join("rescriptum.env");
    fs::write(
        &env,
        format!(
            "RESCRIPTUM_ANSWERS_DIR={}\nRESCRIPTUM_LOG=problems\nRESCRIPTUM_LOG_FILE={}\n",
            c.dir.display(),
            log.display()
        ),
    )
    .unwrap();

    let r = c.run_env(&[("RESCRIPTUM_ENV_FILE", &env)], &["check"]);
    assert!(r.ok, "{r}");
    assert!(
        !r.stderr.contains("not a variable this program reads"),
        "a known key must not be reported as a typo\n{r}"
    );
}

#[test]
fn naming_the_env_file_inside_itself_says_why_it_does_nothing() {
    // It is a variable the program reads, just not from here — the file has to be found
    // before it can be parsed. "Not a variable this program reads" would be false, and
    // would send the reader hunting for a typo that is not there.
    let c = Case::new(&[("groups/rack-a.toml", RACK)]);
    let env = c.dir.join("rescriptum.env");
    fs::write(
        &env,
        format!(
            "RESCRIPTUM_ANSWERS_DIR={}\nRESCRIPTUM_ENV_FILE=/somewhere/else.env\n",
            c.dir.display()
        ),
    )
    .unwrap();

    let r = c.run_env(&[("RESCRIPTUM_ENV_FILE", &env)], &["check"]);
    assert!(r.ok, "{r}");
    assert!(
        r.stderr.contains("has no effect inside the file it names"),
        "{r}"
    );
    assert!(
        !r.stderr.contains("not a variable this program reads"),
        "the message must not claim it is unknown\n{r}"
    );
}
