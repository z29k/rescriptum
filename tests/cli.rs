//! `render`, `check`, `import` and `export`, against the real binary.
//!
//! These are not conveniences. `check` is what `deploy.sh` runs before it ships anything,
//! and what the documentation tells people to put in CI — so its **exit code** is
//! load-bearing, and nothing else pins it. `render` is the only way to see a composed
//! answer before a rack does; the stdout/stderr split is what makes `render … > answer`
//! usable, so that is a contract too.

mod common;

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
        let _ = fs::remove_dir_all(self.etc());
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
            common::seed(&self.dir, name, contents);
        }
    }

    /// Write a document exactly where the name says, without the layout's opinion —
    /// which is how an answers directory from before the layout actually looks.
    fn write_flat(&self, files: &[(&str, &str)]) {
        for (name, contents) in files {
            let path = self.dir.join(name);
            fs::create_dir_all(path.parent().expect("a parent")).expect("subdirectory");
            fs::write(&path, contents).expect("flat fixture");
        }
    }

    /// A scratch directory **beside** the answers directory, for the files that
    /// configure the server rather than being served by it.
    ///
    /// This is not tidiness. Every servable `.toml` at the top of the answers directory
    /// is a misplaced answer document, and a configuration file is not exempt — see
    /// `a_configuration_file_inside_the_answers_directory_is_reported_as_a_stray_answer`,
    /// which pins that rather than leaving it to be discovered.
    fn etc(&self) -> PathBuf {
        let mut name = self.dir.file_name().expect("a name").to_os_string();
        name.push("-etc");
        self.dir.with_file_name(name)
    }

    fn conf(&self, name: &str, body: &str) -> PathBuf {
        let dir = self.etc();
        fs::create_dir_all(&dir).expect("scratch etc");
        let path = dir.join(name);
        fs::write(&path, body).expect("configuration file");
        path
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
    // The **extension**, not the family. This used to read `format=text`, which made
    // `.ks`, `.preseed`, `.cfg`, `.seed` and `.ipxe` indistinguishable in the log — and a
    // log that cannot tell a boot script from an answer document cannot say which
    // machines are mid-install.
    assert!(ks.stderr.contains("format=ks"), "{ks}");

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
    assert!(
        r.stdout.contains("1 group(s), 1 machine document(s)"),
        "{r}"
    );
}

#[test]
fn check_catches_a_broken_default_even_though_it_names_no_machine() {
    // `default` is not rendered by name — it is reached only when nothing else matches
    // — so it would be easy for it to escape the sweep. It must not.
    for broken in ["x = = 1\n", "extends = \"nope\"\n[global]\nx = 1\n"] {
        let c = Case::new(&[("default.toml", broken)]);
        let r = c.run(&["check"]);
        assert!(!r.ok, "{broken:?}: {r}");
        // Named by its path, which is where the operator has to go and fix it.
        assert!(r.stdout.contains("default/proxmox.toml"), "{broken:?}: {r}");
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
        // Same path on both sides as well as the same bytes: `export` writing a
        // document somewhere `import` would not look for it is the failure that makes
        // the database unsafe to leave.
        let before =
            fs::read(common::document_path(&c.dir, name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let after = fs::read(common::document_path(&out, name))
            .unwrap_or_else(|e| panic!("{name} exported: {e}"));
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

// ---- the toml file --------------------------------------------------------
//
// The same job as the env file, in the shape a person edits by hand on a NAS. What these
// pin is that it is the *same* configuration: one set of settings, one precedence rule,
// and a document that cannot mean anything the environment could not.

#[test]
fn a_toml_file_supplies_configuration() {
    let c = Case::new(&[("groups/rack-a.toml", RACK)]);
    let toml = c.conf(
        "rescriptum.toml",
        &format!(
            "answers_dir = \"{}\"\n\n[server]\ntimeout_secs = 7\n",
            c.dir.display()
        ),
    );

    let r = c.run_env(&[("RESCRIPTUM_CONFIG", &toml)], &["check"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("1 group(s)"), "{r}");
    assert!(
        r.stderr.contains("reading configuration defaults from"),
        "the file it read must be named in the log\n{r}"
    );
}

#[test]
fn the_real_environment_wins_over_the_toml_file() {
    // The rule the env file already has, and the reason both files are only defaults.
    let c = Case::new(&[("98fa9b50d810.toml", "marker = \"from-the-flag\"\n")]);
    let elsewhere = c.dir.join("unused");
    fs::create_dir_all(&elsewhere).unwrap();
    let toml = c.conf(
        "rescriptum.toml",
        &format!("answers_dir = \"{}\"\n", elsewhere.display()),
    );

    let r = c.run_env(
        &[
            ("RESCRIPTUM_CONFIG", &toml),
            ("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path()),
        ],
        &["render", "98:fa:9b:50:d8:10"],
    );
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("from-the-flag"), "{r}");
}

#[test]
fn the_toml_file_wins_over_the_env_file_and_says_that_both_are_named() {
    // Naming both is a deployment mid-migration. Which one wins is the single question
    // neither file can answer by itself, so the server answers it out loud.
    let c = Case::new(&[("98fa9b50d810.toml", "marker = \"from-the-toml\"\n")]);
    let elsewhere = c.dir.join("unused");
    fs::create_dir_all(&elsewhere).unwrap();
    let env = c.conf(
        "rescriptum.env",
        &format!("RESCRIPTUM_ANSWERS_DIR={}\n", elsewhere.display()),
    );
    let toml = c.conf(
        "rescriptum.toml",
        &format!("answers_dir = \"{}\"\n", c.dir.display()),
    );

    let r = c.run_env(
        &[("RESCRIPTUM_CONFIG", &toml), ("RESCRIPTUM_ENV_FILE", &env)],
        &["render", "98:fa:9b:50:d8:10"],
    );
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("from-the-toml"), "{r}");
    assert!(
        r.stderr.contains("are both named"),
        "an operator must be told which file is winning\n{r}"
    );
}

#[test]
fn a_toml_file_that_cannot_be_read_refuses_to_start() {
    // Carrying on with defaults is what a named file exists to prevent, whichever format
    // it is written in.
    let c = Case::new(&[]);
    let r = c.run_env(
        &[("RESCRIPTUM_CONFIG", &c.dir.join("absent.toml"))],
        &["check"],
    );
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("RESCRIPTUM_CONFIG"), "{r}");
    assert!(r.stderr.contains("cannot be read"), "{r}");
}

#[test]
fn a_malformed_toml_file_refuses_to_start_and_says_where() {
    let c = Case::new(&[]);
    let toml = c.conf("bad.toml", "answers_dir = \n");

    let r = c.run_env(&[("RESCRIPTUM_CONFIG", &toml)], &["check"]);
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("bad.toml"), "{r}");
    // The parser's own message carries the line and column; what matters here is that it
    // arrives on one line, since this is a startup error in a log.
    assert_eq!(
        r.stderr.lines().filter(|l| l.contains("bad.toml")).count(),
        1,
        "{r}"
    );
}

#[test]
fn a_setting_given_a_list_is_refused_rather_than_quietly_defaulted() {
    // A misspelling is a warning; this was aimed at something real, so serving the
    // default while the file plainly says otherwise would be the silent failure.
    let c = Case::new(&[]);
    let toml = c.conf("list.toml", "answers_dir = [\"/srv/answers\"]\n");

    let r = c.run_env(&[("RESCRIPTUM_CONFIG", &toml)], &["check"]);
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("answers_dir"), "{r}");
}

#[test]
fn a_misspelled_setting_in_the_toml_file_is_warned_about() {
    let c = Case::new(&[("groups/rack-a.toml", RACK)]);
    let toml = c.conf(
        "typo.toml",
        &format!(
            "answers_dir = \"{}\"\n\n[admin]\ntoken = \"0123456789abcdef0\"\ntokenn = \"hunter2\"\n",
            c.dir.display()
        ),
    );

    let r = c.run_env(&[("RESCRIPTUM_CONFIG", &toml)], &["check"]);
    assert!(r.ok, "a typo is a warning, not a refusal\n{r}");
    assert!(r.stderr.contains("admin.tokenn"), "{r}");
    assert!(
        !r.stderr.contains("hunter2"),
        "a warning must never print what the file holds\n{r}"
    );
}

#[test]
fn no_toml_file_is_ever_discovered_on_its_own() {
    // Same reasoning as the env file, and it is not negotiable: this binary runs as root,
    // and the file holds admin.token.
    let c = Case::new(&[("98fa9b50d810.toml", "marker = \"real\"\n")]);
    for name in ["rescriptum.toml", "config.toml", ".rescriptum.toml"] {
        fs::write(c.dir.join(name), "answers_dir = \"/nonexistent/planted\"\n").unwrap();
    }

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
fn a_configuration_file_inside_the_answers_directory_is_reported_as_a_stray_answer() {
    // Found by writing the tests above: a `.toml` at the top of the answers directory is
    // a misplaced answer document, and this format shares that extension. Nothing here
    // makes an exception for it — a configuration file belongs beside the store, not in
    // it — so `check` says the same thing it says about any stray document. Better said
    // than discovered when `migrate --apply` offers to move somebody's configuration.
    let c = Case::new(&[("groups/rack-a.toml", RACK)]);
    let inside = c.dir.join("rescriptum.toml");
    fs::write(&inside, format!("answers_dir = \"{}\"\n", c.dir.display())).unwrap();

    let r = c.run_env(&[("RESCRIPTUM_CONFIG", &inside)], &["check"]);
    assert!(
        !r.ok,
        "a stray document is a problem, and problems fail check\n{r}"
    );
    assert!(r.stdout.contains("rescriptum.toml"), "{r}");
    assert!(r.stdout.contains("an answer is a directory now"), "{r}");
    // And it is still read as configuration, because the variable named it.
    assert!(r.stdout.contains("1 group(s)"), "{r}");
}

// ---- config ---------------------------------------------------------------
//
// The command a settings panel drives, and the one people reach for when the server will
// not start — so its exit code is a contract like `check`'s: zero when the configuration
// would start, one when it would not.

impl Case {
    /// `run_env` takes paths, which most of these do not have. Configuration is strings.
    fn run_config(&self, env: &[(&str, &str)], args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rescriptum"));
        // Inherited from the test runner's own environment, these would silently beat the
        // file and make every assertion below about the wrong thing.
        for (key, _, _) in [("RESCRIPTUM_LOG", "", ""), ("RESCRIPTUM_STORE", "", "")] {
            cmd.env_remove(key);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }
        Run::from(cmd.args(args).output().expect("run rescriptum"))
    }

    fn env_file(&self, body: &str) -> String {
        let path = self.dir.join("rescriptum.env");
        fs::write(&path, body).expect("env file");
        path.to_string_lossy().into_owned()
    }

    /// Beside the answers directory, where a configuration file belongs — see `etc`.
    fn toml_file(&self, body: &str) -> String {
        self.conf("rescriptum.toml", body)
            .to_string_lossy()
            .into_owned()
    }
}

#[test]
fn config_says_which_of_the_file_and_the_environment_is_winning() {
    // The file is only defaults. A panel that offered to edit a value the environment
    // overrides would be offering to change nothing at all.
    let c = Case::new(&[]);
    let env = c.env_file("RESCRIPTUM_LOG=problems\nRESCRIPTUM_LISTEN_ADDR=0.0.0.0:8000\n");

    let r = c.run_config(
        &[
            ("RESCRIPTUM_ENV_FILE", &env),
            ("RESCRIPTUM_LISTEN_ADDR", "127.0.0.1:9999"),
        ],
        &["config"],
    );
    assert!(r.ok, "{r}");

    let listen = line_for(&r.stdout, "RESCRIPTUM_LISTEN_ADDR");
    assert!(listen.contains("127.0.0.1:9999"), "{r}");
    assert!(listen.ends_with("environment"), "{listen:?}\n{r}");

    let log = line_for(&r.stdout, "RESCRIPTUM_LOG ");
    assert!(
        log.contains("problems") && log.ends_with("file"),
        "{log:?}\n{r}"
    );

    let store = line_for(&r.stdout, "RESCRIPTUM_STORE");
    assert!(
        store.contains("files") && store.ends_with("default"),
        "{store:?}\n{r}"
    );
}

/// The row for one variable, trimmed — the table is padded.
fn line_for(stdout: &str, key: &str) -> String {
    stdout
        .lines()
        .find(|l| l.trim_start().starts_with(key))
        .unwrap_or_else(|| panic!("no row for {key} in\n{stdout}"))
        .trim()
        .to_string()
}

#[test]
fn config_never_prints_a_token() {
    // This output is what the DSM panel renders. A token reaching it once is a token in
    // somebody's browser, their history, and a screenshot in a support thread.
    let c = Case::new(&[]);
    let env = c.env_file(
        "RESCRIPTUM_ADMIN_TOKEN=sup3rs3cr3ttok3nvalue\nRESCRIPTUM_ANSWER_TOKEN=an0th3rs3cr3tvalue\n",
    );

    for args in [vec!["config"], vec!["config", "--json"]] {
        let r = c.run_config(&[("RESCRIPTUM_ENV_FILE", &env)], &args);
        assert!(r.ok, "{r}");
        assert!(
            !r.stdout.contains("sup3rs3cr3ttok3nvalue") && !r.stdout.contains("an0th3rs3cr3tvalue"),
            "a token reached the output of {args:?}\n{r}"
        );
        assert!(
            !r.stderr.contains("sup3rs3cr3ttok3nvalue"),
            "a token reached stderr\n{r}"
        );
    }

    // And it still has to say that there *is* one, or the panel cannot tell you whether
    // the endpoint is guarded.
    let r = c.run_config(&[("RESCRIPTUM_ENV_FILE", &env)], &["config"]);
    assert!(
        line_for(&r.stdout, "RESCRIPTUM_ADMIN_TOKEN").contains("(set)"),
        "{r}"
    );
}

#[test]
fn config_set_keeps_the_comments_that_explain_the_file() {
    // On a packaged install those comments are the only documentation the configuration
    // has. A writer that regenerated the file would eat them on the first save.
    let c = Case::new(&[]);
    let env = c.env_file(
        "# Where answers live.\nRESCRIPTUM_ANSWERS_DIR=/srv/answers\n\n# Off by default.\n# RESCRIPTUM_LOG=problems\n",
    );

    let r = c.run_config(
        &[("RESCRIPTUM_ENV_FILE", &env)],
        &["config", "set", "RESCRIPTUM_LOG=problems"],
    );
    assert!(r.ok, "{r}");

    let after = fs::read_to_string(&env).expect("still there");
    assert!(after.contains("# Where answers live."), "{after}");
    assert!(after.contains("# Off by default."), "{after}");
    // Uncommented in place rather than appended below, or the paragraph above it now
    // explains a line that is no longer there.
    assert!(after.contains("\nRESCRIPTUM_LOG=problems\n"), "{after}");
    assert_eq!(
        after.matches("RESCRIPTUM_LOG").count(),
        1,
        "the setting was duplicated\n{after}"
    );
}

#[test]
fn config_set_refuses_to_leave_a_server_that_cannot_start() {
    // The panel doing the writing is reached over the very service this would stop. Get
    // it wrong and the way back in is SSH — which is the thing the panel exists to avoid.
    let c = Case::new(&[]);
    let before = "RESCRIPTUM_STORE=sqlite\n";
    let env = c.env_file(before);

    let r = c.run_config(
        &[("RESCRIPTUM_ENV_FILE", &env)],
        &["config", "set", "RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001"],
    );
    assert!(!r.ok, "an unauthenticated admin API must be refused\n{r}");
    assert!(r.stderr.contains("refused"), "{r}");
    assert_eq!(
        fs::read_to_string(&env).expect("still there"),
        before,
        "the file must be untouched when the write is refused"
    );
}

#[test]
fn config_set_refuses_a_misspelled_variable() {
    // Written, it would be read back as a stranger and warned about only at the next
    // start — by which time nobody connects the two.
    let c = Case::new(&[]);
    let env = c.env_file("RESCRIPTUM_STORE=files\n");

    let r = c.run_config(
        &[("RESCRIPTUM_ENV_FILE", &env)],
        &["config", "set", "RESCRIPTUM_ADMIN_TOKENN=0123456789abcdef0"],
    );
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("check the spelling"), "{r}");
    assert_eq!(
        fs::read_to_string(&env).unwrap(),
        "RESCRIPTUM_STORE=files\n"
    );
}

#[test]
fn config_exit_code_says_whether_the_server_would_start() {
    // The same contract `check` has, for the same reason: something automated keys on it.
    let c = Case::new(&[]);

    let healthy = c.env_file("RESCRIPTUM_STORE=files\n");
    let r = c.run_config(&[("RESCRIPTUM_ENV_FILE", &healthy)], &["config"]);
    assert!(r.ok, "{r}");

    // A file that will not parse is a *startup error*, not a warning — the server refuses
    // to come up on it — so it has to reach the exit code rather than only the text.
    let broken = c.dir.join("broken.env");
    fs::write(&broken, "RESCRIPTUM_STORE files\n").unwrap();
    let broken = broken.to_string_lossy().into_owned();
    let r = c.run_config(&[("RESCRIPTUM_ENV_FILE", &broken)], &["config"]);
    assert!(!r.ok, "an unparseable file must fail\n{r}");
    assert!(r.stderr.contains("would not start"), "{r}");
    // It still prints the table: seeing the other variables beside the reason is what
    // makes this usable when everything is broken.
    assert!(r.stdout.contains("RESCRIPTUM_ANSWERS_DIR"), "{r}");
}

#[test]
fn config_works_when_the_configuration_is_too_broken_to_start_a_server() {
    // Every other subcommand is handed a configuration `validate` has accepted. This one
    // must survive one it would reject, or the diagnostic tool is the first casualty of
    // the thing it diagnoses.
    let c = Case::new(&[]);
    let env = c.env_file(
        "RESCRIPTUM_STORE=sqlite\nRESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001\nRESCRIPTUM_ADMIN_TOKEN=short\n",
    );

    let r = c.run_config(&[("RESCRIPTUM_ENV_FILE", &env)], &["config"]);
    assert!(!r.ok, "{r}");
    assert!(
        r.stdout.contains("RESCRIPTUM_ADMIN_ADDR"),
        "it still prints\n{r}"
    );
    assert!(r.stderr.contains("16"), "and says why\n{r}");

    // And it can put it right, which is the whole point.
    let fix = c.run_config(
        &[("RESCRIPTUM_ENV_FILE", &env)],
        &["config", "set", "RESCRIPTUM_ADMIN_TOKEN=0123456789abcdef0"],
    );
    assert!(fix.ok, "{fix}");
    let r = c.run_config(&[("RESCRIPTUM_ENV_FILE", &env)], &["config"]);
    assert!(r.ok, "{r}");
}

#[test]
fn config_json_carries_the_source_and_the_help_a_panel_needs() {
    let c = Case::new(&[]);
    let env = c.env_file("RESCRIPTUM_STORE=sqlite\n");

    let r = c.run_config(&[("RESCRIPTUM_ENV_FILE", &env)], &["config", "--json"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("\"key\":\"RESCRIPTUM_STORE\""), "{r}");
    assert!(r.stdout.contains("\"source\":\"env file\""), "{r}");
    assert!(
        r.stdout.contains("\"secret\":true"),
        "the tokens are marked\n{r}"
    );
    assert!(r.stdout.contains("\"starts\":true"), "{r}");
    assert!(r.stdout.contains("\"writable\":true"), "{r}");
    // One line, so a CGI can hand it straight to a browser.
    assert_eq!(r.stdout.lines().count(), 1, "{r}");
}

#[test]
fn config_with_no_file_named_says_so_rather_than_failing() {
    // A container or a systemd unit configures the environment directly and has nothing
    // here to edit. That is a normal deployment, not an error.
    let c = Case::new(&[]);
    let r = c.run_config(&[], &["config"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("none"), "{r}");

    let w = c.run_config(&[], &["config", "set", "RESCRIPTUM_LOG=off"]);
    assert!(!w.ok, "there is nothing to write to\n{w}");
    assert!(w.stderr.contains("RESCRIPTUM_ENV_FILE"), "{w}");
}

#[test]
fn config_writes_the_toml_file_and_leaves_its_documentation_standing() {
    // On a packaged install the comments *are* the configuration's documentation. A
    // writer that regenerated the file would throw them away the first time anyone
    // changed a setting, which is why this edits the document rather than rendering one.
    let c = Case::new(&[]);
    let before = "# How much to log.\nlog = \"all\"  # all | problems | off\n\n[store]\n# Where answers come from.\nkind = \"files\"\n";
    let toml = c.toml_file(before);

    let r = c.run_config(
        &[("RESCRIPTUM_CONFIG", &toml)],
        &["config", "set", "RESCRIPTUM_LOG=problems"],
    );
    assert!(r.ok, "{r}");

    let after = fs::read_to_string(&toml).expect("still there");
    assert!(after.contains("# How much to log."), "{after}");
    assert!(after.contains("# all | problems | off"), "{after}");
    assert!(after.contains("# Where answers come from."), "{after}");
    assert!(after.contains("log = \"problems\""), "{after}");
    assert_eq!(after.matches("log =").count(), 1, "duplicated\n{after}");

    // And the server reads back what was written, from the file that was written.
    let r = c.run_config(&[("RESCRIPTUM_CONFIG", &toml)], &["config"]);
    let log = line_for(&r.stdout, "RESCRIPTUM_LOG ");
    assert!(
        log.contains("problems") && log.ends_with("toml file"),
        "{log:?}\n{r}"
    );
}

#[test]
fn config_unset_empties_a_setting_rather_than_deleting_its_paragraph() {
    // Deleting the key would take the comment above it with it. Empty already counts as
    // unset everywhere else in this program, so the line stays and says nothing is set.
    let c = Case::new(&[]);
    let toml =
        c.toml_file("# The token every installer must present.\n[answer]\ntoken = \"hunter2\"\n");

    let r = c.run_config(
        &[("RESCRIPTUM_CONFIG", &toml)],
        &["config", "unset", "RESCRIPTUM_ANSWER_TOKEN"],
    );
    assert!(r.ok, "{r}");

    let after = fs::read_to_string(&toml).expect("still there");
    assert!(
        after.contains("# The token every installer must present."),
        "{after}"
    );
    assert!(after.contains("token = \"\""), "{after}");
    assert!(!after.contains("hunter2"), "{after}");

    let r = c.run_config(&[("RESCRIPTUM_CONFIG", &toml)], &["config"]);
    let token = line_for(&r.stdout, "RESCRIPTUM_ANSWER_TOKEN");
    assert!(token.contains("(not set)"), "{token:?}\n{r}");
}

#[test]
fn config_set_refuses_to_leave_a_server_that_cannot_start_in_toml_too() {
    // The same guard, over the other format. A write that reaches the disk and stops the
    // server is the failure; the format it was written in is not the interesting part.
    let c = Case::new(&[]);
    let before = "[store]\nkind = \"sqlite\"\n";
    let toml = c.toml_file(before);

    let r = c.run_config(
        &[("RESCRIPTUM_CONFIG", &toml)],
        &["config", "set", "RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001"],
    );
    assert!(!r.ok, "an unauthenticated admin API must be refused\n{r}");
    assert!(r.stderr.contains("refused"), "{r}");
    assert_eq!(
        fs::read_to_string(&toml).expect("still there"),
        before,
        "the file must be untouched when the write is refused"
    );
}

#[test]
fn config_set_writes_the_toml_file_when_both_files_are_named() {
    // It is the one the server reads first, so writing the other would be a change that
    // silently does nothing — which is the whole failure mode this area exists to remove.
    let c = Case::new(&[]);
    let env = c.env_file("RESCRIPTUM_STORE=files\n");
    let toml = c.toml_file("log = \"all\"\n");

    let r = c.run_config(
        &[("RESCRIPTUM_CONFIG", &toml), ("RESCRIPTUM_ENV_FILE", &env)],
        &["config", "set", "RESCRIPTUM_LOG=off"],
    );
    assert!(r.ok, "{r}");
    assert!(
        r.stderr.contains(&toml),
        "it must say which file it wrote\n{r}"
    );
    assert!(fs::read_to_string(&toml).unwrap().contains("log = \"off\""));
    assert_eq!(
        fs::read_to_string(&env).unwrap(),
        "RESCRIPTUM_STORE=files\n",
        "the env file must be left alone"
    );
}

#[test]
fn config_set_answers_the_documents_name_with_the_one_that_works() {
    // Somebody reading the file types the name they see in it. "Not a setting this
    // program reads" would be true of the command line and useless to them.
    let c = Case::new(&[]);
    let toml = c.toml_file("log = \"all\"\n");

    let r = c.run_config(
        &[("RESCRIPTUM_CONFIG", &toml)],
        &["config", "set", "answers_dir=/srv/answers"],
    );
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("RESCRIPTUM_ANSWERS_DIR"), "{r}");
}

#[test]
fn config_json_names_both_files_and_the_name_each_setting_has_in_one() {
    // The panel renders this. It has to be able to say which file a save would land in,
    // and to show the line somebody would edit by hand.
    let c = Case::new(&[]);
    let toml = c.toml_file("[store]\nkind = \"sqlite\"\n");

    let r = c.run_config(&[("RESCRIPTUM_CONFIG", &toml)], &["config", "--json"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("\"source\":\"toml file\""), "{r}");
    assert!(r.stdout.contains("\"path\":\"store.kind\""), "{r}");
    assert!(
        r.stdout.contains(&format!("\"toml_file\":\"{toml}\"")),
        "{r}"
    );
    assert!(r.stdout.contains(&format!("\"target\":\"{toml}\"")), "{r}");
    assert!(r.stdout.contains("\"writable\":true"), "{r}");
    assert_eq!(r.stdout.lines().count(), 1, "{r}");
}

#[test]
fn config_value_prints_one_setting_but_never_a_credential() {
    // The DSM panel's backend reads a value this way rather than grepping the file,
    // because a variable absent from the file is not unset — it is the default, and a
    // grep cannot know that. The refusal matters just as much: this output is a shell
    // variable, and a shell variable ends up in a log or a `set -x` trace.
    let c = Case::new(&[]);
    let env = c.env_file("RESCRIPTUM_ADMIN_TOKEN=sup3rs3cr3ttok3nvalue\n");

    let d = c.run_config(
        &[("RESCRIPTUM_ENV_FILE", &env)],
        &["config", "--value", "RESCRIPTUM_ANSWERS_DIR"],
    );
    assert!(d.ok, "{d}");
    assert_eq!(
        d.stdout.trim(),
        "/srv/answers",
        "the default, not nothing\n{d}"
    );

    let s = c.run_config(
        &[("RESCRIPTUM_ENV_FILE", &env)],
        &["config", "--value", "RESCRIPTUM_ADMIN_TOKEN"],
    );
    assert!(!s.ok, "a credential must not be printable\n{s}");
    assert!(!s.stdout.contains("sup3rs3cr3ttok3nvalue"), "{s}");
    assert!(!s.stderr.contains("sup3rs3cr3ttok3nvalue"), "{s}");

    // Unset is an exit code, not an empty line that a script would mistake for a value.
    let u = c.run_config(
        &[("RESCRIPTUM_ENV_FILE", &env)],
        &["config", "--value", "RESCRIPTUM_CAPTURE_DIR"],
    );
    assert!(!u.ok, "{u}");
    assert!(u.stdout.is_empty(), "{u}");
}

// ---- boot ------------------------------------------------------------------

/// A snippet is generated so an operator can *copy* rather than compose, so the shape
/// of what comes out is the contract — and stdout has to be the file, with everything
/// else on stderr, for `> dhcp.conf` to work.
#[cfg(feature = "boot")]
fn snippet(case: &Case, args: &[&str]) -> Run {
    let mut all = vec!["boot", "dhcp-snippet"];
    all.extend_from_slice(args);
    case.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", case.dir.as_path()),
            ("RESCRIPTUM_PUBLIC_HOST", Path::new("192.0.2.10")),
        ],
        &all,
    )
}

#[test]
#[cfg(feature = "boot")]
fn every_dhcp_format_names_the_loaders_the_tftp_table_actually_serves() {
    // **The two are generated from one table precisely so a test can pin them
    // together.** A snippet naming a loader the server does not hand out fails
    // silently, at the ROM, with nothing on any console — it is the least diagnosable
    // failure in the whole chain, and nothing else would catch it.
    let case = Case::new(&[]);
    let served = rescriptum::boot::loaders::loaders();

    for format in ["dnsmasq", "isc", "kea", "powershell", "pfsense", "mikrotik"] {
        let r = snippet(&case, &["--format", format]);
        assert!(r.ok, "{format}: {r}");
        assert!(r.stdout.contains("192.0.2.10"), "{format}: {r}");

        for loader in &served {
            // pfSense and RouterOS are interfaces rather than files, and both say in
            // their own output which architectures they cannot express.
            if matches!(format, "pfsense" | "mikrotik") && loader.contains("arm64") {
                continue;
            }
            assert!(
                r.stdout.contains(loader),
                "{format} does not name {loader}: {r}"
            );
        }
    }
}

#[test]
#[cfg(feature = "boot")]
fn a_snippet_goes_to_stdout_and_warnings_go_to_stderr() {
    // `boot dhcp-snippet > dhcpd.conf` has to produce a file that can be included.
    let case = Case::new(&[]);
    let r = snippet(&case, &["--format", "isc"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.starts_with("# rescriptum "), "{r}");
    assert!(
        r.stderr.is_empty(),
        "nothing on stderr when nothing is wrong: {r}"
    );
}

#[test]
#[cfg(feature = "boot")]
fn a_derived_public_host_warns_on_stderr_without_spoiling_the_snippet() {
    // A DHCP server handing out an address the machines cannot reach is the hardest
    // failure in this chain to diagnose, so it is said — but on stderr, so the snippet
    // is still usable when redirected.
    let case = Case::new(&[]);
    let r = case.run_env(
        &[("RESCRIPTUM_ANSWERS_DIR", case.dir.as_path())],
        &["boot", "dhcp-snippet"],
    );
    assert!(r.ok, "{r}");
    assert!(r.stderr.contains("RESCRIPTUM_PUBLIC_HOST"), "{r}");
    assert!(r.stdout.starts_with("# rescriptum "), "{r}");
}

#[test]
#[cfg(feature = "boot")]
fn an_unknown_dhcp_format_lists_the_ones_that_exist() {
    let case = Case::new(&[]);
    let r = snippet(&case, &["--format", "bind"]);
    assert!(!r.ok, "{r}");
    assert!(
        r.stderr.contains("dnsmasq"),
        "the error must name what does work: {r}"
    );
    // `netsh` is deliberately not an alias, because it cannot express this.
    let r = snippet(&case, &["--format", "netsh"]);
    assert!(!r.ok, "{r}");
}

#[test]
#[cfg(feature = "boot")]
fn boot_check_fails_when_a_snippet_names_a_loader_that_is_not_there() {
    // The exit code is a contract, like `check`'s: `deploy.sh` keys on it.
    //
    // **TFTP off, deliberately.** This test is about the loaders, and `boot check` also
    // probes the TFTP address — which defaults to the privileged port 69. macOS lets an
    // unprivileged process bind UDP 69 and Linux does not, so leaving it on makes the
    // verdict depend on the platform and on who is running the suite. Turning it off
    // isolates what is being measured; `tests/tftp.rs` covers the unbindable port.
    let case = Case::new(&[]);
    let boot_dir = case.dir.join("boot");
    fs::create_dir_all(&boot_dir).expect("boot dir");

    let r = case.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", case.dir.as_path()),
            ("RESCRIPTUM_BOOT_DIR", boot_dir.as_path()),
            ("RESCRIPTUM_TFTP_ADDR", Path::new("off")),
        ],
        &["boot", "check"],
    );
    assert!(!r.ok, "an empty boot directory must fail: {r}");
    assert!(r.stdout.contains("MISSING"), "{r}");
    assert!(
        r.stdout.contains("get nothing, and stop"),
        "the reason has to say what the machine will do: {r}"
    );

    // Put every loader there and it passes.
    for loader in rescriptum::boot::loaders::loaders() {
        fs::write(boot_dir.join(loader), b"not really a loader").expect("write");
    }
    let r = case.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", case.dir.as_path()),
            ("RESCRIPTUM_BOOT_DIR", boot_dir.as_path()),
            ("RESCRIPTUM_TFTP_ADDR", Path::new("off")),
        ],
        &["boot", "check"],
    );
    assert!(r.ok, "{r}");
    assert!(
        r.stdout
            .contains("ok — the loaders a snippet names are all here"),
        "{r}"
    );
}

#[test]
#[cfg(feature = "boot")]
fn boot_check_says_nothing_is_wrong_when_boot_assets_are_simply_off() {
    // Off is a normal state, not a failure: media can be served with no TFTP at all.
    let case = Case::new(&[]);
    let r = case.run(&["boot", "check"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("boot assets are off"), "{r}");
}

#[test]
#[cfg(feature = "boot")]
fn boot_check_warns_when_the_media_port_is_not_the_one_loaders_embed() {
    // The embedded script in every loader already shipped chains to a fixed port and
    // can read no configuration — it is baked in before any deployment exists.
    let case = Case::new(&[]);
    let boot_dir = case.dir.join("boot");
    let media_dir = case.dir.join("media");
    fs::create_dir_all(&boot_dir).expect("boot dir");
    fs::create_dir_all(&media_dir).expect("media dir");
    for loader in rescriptum::boot::loaders::loaders() {
        fs::write(boot_dir.join(loader), b"x").expect("write");
    }

    let r = case.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", case.dir.as_path()),
            ("RESCRIPTUM_BOOT_DIR", boot_dir.as_path()),
            ("RESCRIPTUM_MEDIA_DIR", media_dir.as_path()),
            ("RESCRIPTUM_MEDIA_ADDR", Path::new("0.0.0.0:9999")),
            // Off for the same reason as above: this asserts on the media port, and a
            // TFTP probe that fails only on Linux would make it pass for two reasons on
            // one platform and one on the other.
            ("RESCRIPTUM_TFTP_ADDR", Path::new("off")),
        ],
        &["boot", "check"],
    );
    assert!(!r.ok, "{r}");
    assert!(r.stdout.contains("8001"), "{r}");
    assert!(r.stdout.contains("already shipped"), "{r}");
}

#[test]
#[cfg(feature = "boot")]
fn the_bootstrap_and_the_menu_can_be_printed_for_review() {
    // Everything a machine will execute has to be readable by a human before it runs on
    // a rack, which is the same argument `render` makes for answers.
    let case = Case::new(&[]);
    let r = case.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", case.dir.as_path()),
            ("RESCRIPTUM_PUBLIC_HOST", Path::new("192.0.2.10")),
        ],
        &["boot", "bootstrap"],
    );
    assert!(r.ok, "{r}");
    assert!(r.stdout.starts_with("#!ipxe\n"), "{r}");
    assert!(r.stdout.contains("${netX/mac}"), "{r}");

    let media_dir = case.dir.join("media");
    fs::create_dir_all(&media_dir).expect("media dir");
    let r = case.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", case.dir.as_path()),
            ("RESCRIPTUM_MEDIA_DIR", media_dir.as_path()),
            ("RESCRIPTUM_PUBLIC_HOST", Path::new("192.0.2.10")),
        ],
        &["boot", "menu"],
    );
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("item local"), "{r}");
    assert!(r.stdout.is_ascii(), "a BIOS text console is not UTF-8: {r}");
}

// ---------------------------------------------------------------------------
// migrate — the way out of the layout that came before
// ---------------------------------------------------------------------------

/// A directory from before the layout: named, unchanged, and told what to type.
#[test]
fn migrate_shows_the_moves_and_changes_nothing_until_told_to() {
    let c = Case::new(&[]);
    c.write_flat(&[
        ("98fa9b50d810.toml", "marker = \"machine\"\n"),
        ("98fa9b50d810.ipxe", "#!ipxe\n"),
        ("groups/rack-a.toml", "members = [\"98:fa:9b:50:d8:10\"]\n"),
        ("default.toml", "[global]\nkeyboard = \"us\"\n"),
        ("README.md", "notes\n"),
    ]);

    let r = c.run(&["migrate"]);
    assert!(r.ok, "{r}");
    for line in [
        "98fa9b50d810.toml -> 98fa9b50d810/proxmox.toml",
        "98fa9b50d810.ipxe -> 98fa9b50d810/boot.ipxe",
        "groups/rack-a.toml -> groups/rack-a/proxmox.toml",
        "default.toml -> default/proxmox.toml",
    ] {
        assert!(r.stdout.contains(line), "missing {line:?}: {r}");
    }
    assert!(
        !r.stdout.contains("README"),
        "an unservable file is not ours: {r}"
    );
    assert!(
        r.stdout.contains("nothing has been changed"),
        "a dry run has to say so: {r}"
    );
    // **And it really did nothing.** The dry run is the default, so this is the
    // assertion that matters most in the whole command.
    assert!(
        c.dir.join("98fa9b50d810.toml").is_file(),
        "the dry run moved a file"
    );
    assert!(
        !c.dir.join("98fa9b50d810").exists(),
        "the dry run created a directory"
    );
}

#[test]
fn migrate_apply_moves_them_and_the_answers_work_afterwards() {
    let c = Case::new(&[]);
    c.write_flat(&[
        ("98fa9b50d810.toml", "marker = \"machine\"\n"),
        (
            "groups/rack-a.toml",
            "members = [\"98:fa:9b:50:d8:10\"]\n[global]\nx = 1\n",
        ),
        ("default.toml", "[global]\nkeyboard = \"us\"\n"),
    ]);

    // Before: the documents are there, and none of them is being served.
    let before = c.run(&["check"]);
    assert!(before.stdout.contains("rescriptum migrate"), "{before}");

    let r = c.run(&["migrate", "--apply"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("moved 3 document(s)"), "{r}");
    assert!(c.dir.join("98fa9b50d810/proxmox.toml").is_file());
    assert!(c.dir.join("groups/rack-a/proxmox.toml").is_file());
    assert!(c.dir.join("default/proxmox.toml").is_file());
    assert!(
        !c.dir.join("98fa9b50d810.toml").exists(),
        "the original was left behind"
    );

    // After: clean, and composing exactly as it did before the move.
    let after = c.run(&["check"]);
    assert!(after.ok, "{after}");
    let rendered = c.run(&["render", "98:fa:9b:50:d8:10"]);
    assert!(rendered.ok, "{rendered}");
    assert!(rendered.stdout.contains("machine"), "{rendered}");
    assert!(
        rendered.stdout.contains("x = 1"),
        "the group stopped applying: {rendered}"
    );

    // Running it again is a no-op that says so, not an error.
    let again = c.run(&["migrate", "--apply"]);
    assert!(again.ok, "{again}");
    assert!(again.stdout.contains("nothing to move"), "{again}");
}

/// A flat document whose destination is taken. Nothing moves, including the ones that
/// could have — a half-migrated directory is the state nobody can reason about.
#[test]
fn migrate_refuses_the_whole_run_when_a_destination_is_taken() {
    let c = Case::new(&[("98fa9b50d810.toml", "marker = \"already here\"\n")]);
    c.write_flat(&[
        ("98fa9b50d810.toml", "marker = \"flat\"\n"),
        ("aabbccddeeff.toml", "marker = \"could have moved\"\n"),
    ]);

    let r = c.run(&["migrate", "--apply"]);
    assert!(!r.ok, "a blocked migration has to fail: {r}");
    assert!(r.stdout.contains("BLOCKED"), "{r}");
    assert!(r.stdout.contains("nothing has been changed"), "{r}");
    assert_eq!(
        fs::read_to_string(c.dir.join("98fa9b50d810/proxmox.toml")).unwrap(),
        "marker = \"already here\"\n",
        "the existing document was overwritten"
    );
    assert!(
        c.dir.join("aabbccddeeff.toml").is_file(),
        "an unrelated document moved during a run that failed"
    );
}

// ---- power ---------------------------------------------------------------

/// A controllers file, written `0600` the way the parser insists on, and **outside the
/// answers directory** — it is a `.toml`, so dropped inside one it would be a misplaced
/// answer document, exactly as a configuration file is. `Case::etc` is that place.
#[cfg(unix)]
fn controllers(case: &Case, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = case.etc();
    fs::create_dir_all(&dir).expect("scratch etc");
    let path = dir.join("controllers.toml");
    fs::write(&path, body).expect("write controllers");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
    path
}

#[cfg(unix)]
const TWO_CONTROLLERS: &str = r#"
["98-fa-9b-50-d8-10"]
kind = "redfish"
url = "https://10.0.0.51"
user = "root"
pass = "calvin"
verify = false

["aa-bb-cc-dd-ee-ff"]
kind = "command"
on  = ["/usr/local/bin/pdu", "outlet", "7", "on"]
off = ["/usr/local/bin/pdu", "outlet", "7", "off"]
"#;

/// Unset means the feature does not exist, the way `RESCRIPTUM_MEDIA_DIR` works.
#[test]
fn power_without_a_controllers_file_says_which_variable_names_one() {
    let c = Case::new(&[("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n")]);
    let r = c.run(&["power", "list"]);
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("RESCRIPTUM_CONTROLLERS_FILE"), "{r}");
}

/// **Both sides of the join**, and the machine/group distinction on the answer side —
/// which is what `install` will refuse on, so it had better be visible before then.
#[cfg(unix)]
#[test]
fn power_list_joins_controllers_to_the_answer_set() {
    let c = Case::new(&[
        ("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n"),
        (
            "groups/rack-a.toml",
            "members = [\"11:22:33:44:55:66\"]\n[global]\nkeyboard = \"us\"\n",
        ),
    ]);
    let file = controllers(
        &c,
        &format!("{TWO_CONTROLLERS}\n[\"11-22-33-44-55-66\"]\nkind = \"command\"\non = [\"x\"]\n"),
    );
    let r = c.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path()),
            ("RESCRIPTUM_CONTROLLERS_FILE", file.as_path()),
        ],
        &["power", "list"],
    );

    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("98-fa-9b-50-d8-10"), "{r}");
    assert!(r.stdout.contains("answered by its own document"), "{r}");
    // A controller for a machine nothing answers is not an error — you may be able to
    // power a machine you have no answer for.
    assert!(r.stdout.contains("nothing answers it"), "{r}");
    // And one answered only by its group, which is the case that cannot disarm itself.
    assert!(r.stdout.contains("answered by a group"), "{r}");
    assert!(r.stdout.contains("3 controller(s)"), "{r}");
}

/// Said every time, because it is the thing somebody meant to fix later and did not.
#[cfg(unix)]
#[test]
fn power_list_says_out_loud_which_controllers_are_unverified() {
    let c = Case::new(&[("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n")]);
    let file = controllers(&c, TWO_CONTROLLERS);
    let r = c.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path()),
            ("RESCRIPTUM_CONTROLLERS_FILE", file.as_path()),
        ],
        &["power", "list"],
    );
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("verify = false"), "{r}");
    // A controller that cannot arm a one-time boot is described, not treated as broken:
    // the boot order stays on PXE and the server decides.
    assert!(r.stdout.contains("the server decides"), "{r}");
}

/// Refused at use, not warned about — unlike the env file, where refusing would stop an
/// otherwise healthy server.
#[cfg(unix)]
#[test]
fn power_refuses_a_controllers_file_others_can_read() {
    use std::os::unix::fs::PermissionsExt;
    let c = Case::new(&[("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n")]);
    let file = controllers(&c, TWO_CONTROLLERS);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).expect("chmod");

    let r = c.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path()),
            ("RESCRIPTUM_CONTROLLERS_FILE", file.as_path()),
        ],
        &["power", "list"],
    );
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("chmod 600"), "{r}");
}

/// The server must not read this file, so a broken one cannot stop it answering — that is
/// the whole reason it is not a startup error.
#[cfg(unix)]
#[test]
fn a_broken_controllers_file_does_not_stop_the_other_commands() {
    let c = Case::new(&[("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n")]);
    let file = controllers(&c, "this is not toml at all");
    let r = c.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path()),
            ("RESCRIPTUM_CONTROLLERS_FILE", file.as_path()),
        ],
        &["check"],
    );
    assert!(r.ok, "check must not care about the controllers file\n{r}");

    // But the command that does read it says why, naming the line.
    let r = c.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path()),
            ("RESCRIPTUM_CONTROLLERS_FILE", file.as_path()),
        ],
        &["power", "list"],
    );
    assert!(!r.ok, "{r}");
}

/// The same rule the configuration file has, for the same reason: this format shares the
/// `.toml` extension with an answer document, so a controllers file dropped at the top of
/// the answers directory is a misplaced answer and is reported rather than served.
#[cfg(unix)]
#[test]
fn a_controllers_file_inside_the_answers_directory_is_reported_as_a_stray_answer() {
    let c = Case::new(&[("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n")]);
    let stray = c.dir.join("controllers.toml");
    fs::write(&stray, TWO_CONTROLLERS).expect("write");

    let r = c.run(&["check"]);
    assert!(!r.ok, "a stray .toml must fail check\n{r}");
    assert!(r.stdout.contains("controllers.toml"), "{r}");
}

/// Resolved rather than hard-coded: `/bin/true` exists on Linux and not on macOS. A
/// hard-coded path here is the "passed locally, failed in CI" trap this repository has
/// already been caught by once.
#[cfg(unix)]
fn tool(name: &str) -> String {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|dir| format!("{dir}/{name}"))
        .find(|p| Path::new(p).is_file())
        .unwrap_or_else(|| panic!("no {name} on this system"))
}

#[cfg(unix)]
fn power_case() -> (Case, PathBuf) {
    let c = Case::new(&[("aabbccddeeff.toml", "[global]\nkeyboard = \"fr\"\n")]);
    let file = controllers(
        &c,
        &format!(
            "[\"aa-bb-cc-dd-ee-ff\"]\nkind = \"command\"\non = [\"{}\"]\noff = [\"{}\"]\n",
            tool("true"),
            tool("false")
        ),
    );
    (c, file)
}

#[cfg(unix)]
fn run_power(c: &Case, file: &Path, args: &[&str]) -> Run {
    c.run_env(
        &[
            ("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path()),
            ("RESCRIPTUM_CONTROLLERS_FILE", file),
        ],
        args,
    )
}

/// A controller that presses a button has no way to know, and saying "off" would be an
/// invention an operator would act on.
#[cfg(unix)]
#[test]
fn power_status_of_a_command_controller_is_unknown_rather_than_guessed() {
    let (c, file) = power_case();
    let r = run_power(&c, &file, &["power", "status", "aa:bb:cc:dd:ee:ff"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("unknown"), "{r}");
}

#[cfg(unix)]
#[test]
fn power_on_runs_the_hook_and_power_off_reports_its_exit_code() {
    let (c, file) = power_case();
    let on = run_power(&c, &file, &["power", "on", "aa:bb:cc:dd:ee:ff"]);
    assert!(on.ok, "{on}");
    assert!(on.stdout.contains("power on sent"), "{on}");

    // The fixture's `off` is `false`, so a failing hook must fail the command rather than
    // being reported as done.
    let off = run_power(&c, &file, &["power", "off", "aa:bb:cc:dd:ee:ff"]);
    assert!(!off.ok, "a failing hook must not report success\n{off}");
    assert!(off.stderr.contains("exited 1"), "{off}");
}

/// Not a failure: the boot order stays on the network and the server decides whether the
/// machine installs — which is what `RESCRIPTUM_BOOT_UNCLAIMED` already does.
#[cfg(unix)]
#[test]
fn power_pxe_on_a_controller_without_one_explains_rather_than_failing() {
    let (c, file) = power_case();
    let r = run_power(&c, &file, &["power", "pxe", "aa:bb:cc:dd:ee:ff"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("no boot override"), "{r}");
    assert!(r.stdout.contains("let the server decide"), "{r}");
}

#[cfg(unix)]
#[test]
fn an_unknown_machine_is_told_what_is_configured_instead() {
    let (c, file) = power_case();
    let r = run_power(&c, &file, &["power", "on", "11:22:33:44:55:66"]);
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("aa-bb-cc-dd-ee-ff"), "{r}");
}

/// `list` must not probe, `list --state` must — and must say it is going to, because a
/// rack of unreachable BMCs otherwise looks like a hang.
#[cfg(unix)]
#[test]
fn only_list_with_state_asks_the_controllers_anything() {
    let (c, file) = power_case();
    let plain = run_power(&c, &file, &["power", "list"]);
    assert!(plain.ok, "{plain}");
    assert!(
        !plain.stderr.contains("asking"),
        "a plain listing must not probe\n{plain}"
    );

    let probed = run_power(&c, &file, &["power", "list", "--state"]);
    assert!(probed.ok, "{probed}");
    assert!(probed.stderr.contains("asking 1 controller"), "{probed}");
    assert!(probed.stdout.contains("unknown"), "{probed}");
}

// ---- install -------------------------------------------------------------

#[cfg(unix)]
const ARMED_MACHINE: &[(&str, &str)] = &[
    ("aabbccddeeff.toml", "[global]\nkeyboard = \"fr\"\n"),
    ("aabbccddeeff.ipxe", "#!ipxe\nchain http://x/answer\n"),
];

#[cfg(unix)]
fn install_case(files: &[(&str, &str)]) -> (Case, PathBuf) {
    let c = Case::new(files);
    let file = controllers(
        &c,
        &format!(
            "[\"aabbccddeeff\"]\nkind = \"command\"\non = [\"{}\"]\n",
            tool("true")
        ),
    );
    (c, file)
}

#[cfg(unix)]
fn run_install(c: &Case, file: &Path, env: &[(&str, &Path)], args: &[&str]) -> Run {
    let mut all: Vec<(&str, &Path)> = vec![
        ("RESCRIPTUM_ANSWERS_DIR", c.dir.as_path()),
        ("RESCRIPTUM_CONTROLLERS_FILE", file),
    ];
    all.extend_from_slice(env);
    c.run_env(&all, args)
}

#[cfg(unix)]
#[test]
fn install_renders_every_format_before_anything_is_powered() {
    let (c, file) = install_case(ARMED_MACHINE);
    let r = run_install(&c, &file, &[], &["install", "aa:bb:cc:dd:ee:ff"]);
    assert!(r.ok, "{r}");
    // Both documents, not a guess at which one the boot script leads to.
    assert!(r.stdout.contains("ok   toml"), "{r}");
    assert!(r.stdout.contains("ok   ipxe"), "{r}");
    assert!(r.stdout.contains("installing"), "{r}");
}

/// Powering on a machine that boots into a broken answer leaves an installer sitting at a
/// prompt in a rack — the exact failure this project exists to prevent.
#[cfg(unix)]
#[test]
fn install_refuses_when_a_document_would_not_render() {
    let (c, file) = install_case(&[
        // A machine whose answer names a group that does not exist.
        ("aabbccddeeff.toml", "extends = \"nowhere\"\n"),
        ("aabbccddeeff.ipxe", "#!ipxe\nchain http://x/answer\n"),
    ]);
    let r = run_install(&c, &file, &[], &["install", "aa:bb:cc:dd:ee:ff"]);
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("nothing has been powered on"), "{r}");
}

/// **The refusal the whole group-arming finding produced.** A machine armed only by its
/// group installs, reports success, is never disarmed, and installs again forever.
#[cfg(unix)]
#[test]
fn install_refuses_a_machine_armed_only_by_its_group() {
    let (c, file) = install_case(&[
        ("aabbccddeeff.toml", "[global]\nkeyboard = \"fr\"\n"),
        (
            "groups/rack-a.ipxe",
            "# answer: member aa:bb:cc:dd:ee:ff\n#!ipxe\nchain http://x/answer\n",
        ),
    ]);
    let r = run_install(&c, &file, &[], &["install", "aa:bb:cc:dd:ee:ff"]);
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("a group is never disarmed"), "{r}");
    assert!(r.stderr.contains("reinstall itself"), "{r}");
}

/// Refused in **both** unclaimed modes, for different reasons — one is dangerous and the
/// other is merely useless, and an operator should be told which.
#[cfg(unix)]
#[test]
fn install_refuses_an_unarmed_machine_differently_in_each_unclaimed_mode() {
    let (c, file) = install_case(&[("aabbccddeeff.toml", "[global]\nkeyboard = \"fr\"\n")]);

    let menu = run_install(&c, &file, &[], &["install", "aa:bb:cc:dd:ee:ff"]);
    assert!(!menu.ok, "{menu}");
    assert!(menu.stderr.contains("boot menu"), "{menu}");

    let local = run_install(
        &c,
        &file,
        &[("RESCRIPTUM_BOOT_UNCLAIMED", Path::new("local"))],
        &["install", "aa:bb:cc:dd:ee:ff"],
    );
    assert!(!local.ok, "{local}");
    assert!(
        local
            .stderr
            .contains("looks exactly like a successful install"),
        "the dangerous case must say so\n{local}"
    );
}

/// The archive is reused rather than an image argument invented, so the operator's own
/// document comes back byte for byte — and `installed-` directories stop accumulating.
#[cfg(unix)]
#[test]
fn install_puts_back_what_a_previous_install_archived() {
    let (c, file) = install_case(&[
        ("aabbccddeeff.toml", "[global]\nkeyboard = \"fr\"\n"),
        (
            "installed-aabbccddeeff.ipxe",
            "#!ipxe\n# the operator's own words\nchain http://x/answer\n",
        ),
    ]);
    let r = run_install(&c, &file, &[], &["install", "aa:bb:cc:dd:ee:ff"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("disarmed by a previous install"), "{r}");
    assert!(r.stdout.contains("boot script is back"), "{r}");

    // Byte for byte, and the archive is gone rather than left to accumulate.
    let back = fs::read_to_string(c.dir.join("aabbccddeeff/boot.ipxe")).expect("restored");
    assert!(back.contains("the operator's own words"), "{back}");
    assert!(!c.dir.join("installed-aabbccddeeff").exists());
}

/// `--dry-run` proves the whole chain without touching the hardware, which is what makes
/// it safe to check a rack before powering any of it.
#[cfg(unix)]
#[test]
fn install_dry_run_changes_nothing() {
    let (c, file) = install_case(&[
        ("aabbccddeeff.toml", "[global]\nkeyboard = \"fr\"\n"),
        ("installed-aabbccddeeff.ipxe", "#!ipxe\nchain http://x/a\n"),
    ]);
    let r = run_install(
        &c,
        &file,
        &[],
        &["install", "aa:bb:cc:dd:ee:ff", "--dry-run"],
    );
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("nothing was powered on"), "{r}");
    // The archive is still archived: a dry run must not arm anything either.
    assert!(c.dir.join("installed-aabbccddeeff").exists());
}

/// A second armed machine, so the refusal is genuinely about the missing controller
/// rather than about the answer set — the checks run in that order on purpose, and a test
/// that cannot tell them apart proves the wrong one.
#[cfg(unix)]
#[test]
fn install_without_a_controller_says_so_rather_than_half_arming() {
    let mut files = ARMED_MACHINE.to_vec();
    files.push(("112233445566.toml", "[global]\nkeyboard = \"fr\"\n"));
    files.push(("112233445566.ipxe", "#!ipxe\nchain http://x/answer\n"));
    let (c, file) = install_case(&files);

    let r = run_install(&c, &file, &[], &["install", "11:22:33:44:55:66"]);
    assert!(!r.ok, "{r}");
    assert!(r.stderr.contains("no controller"), "{r}");
    // It got past the answer checks, which is what makes this about the controller.
    assert!(r.stdout.contains("armed by its own document"), "{r}");
}

// ---- the read model ------------------------------------------------------

const FLEET: &[(&str, &str)] = &[
    ("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n"),
    ("98fa9b50d810.ipxe", "#!ipxe\nchain http://x/a\n"),
    (
        "groups/rack-a.toml",
        "extends = \"base\"\nmembers = [\"11:22:33:44:55:66\"]\n[global]\nkeyboard = \"us\"\n",
    ),
    ("groups/base.toml", "[global]\ncountry = \"fr\"\n"),
    ("installed-aabbccddeeff.ipxe", "#!ipxe\nchain http://x/a\n"),
];

#[test]
fn machines_lists_what_answers_each_one_and_how_it_is_armed() {
    let c = Case::new(FLEET);
    let r = c.run(&["machines"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("98fa9b50d810"), "{r}");
    assert!(r.stdout.contains("armed"), "{r}");
    // A machine a group only names has no documents of its own, and is still a machine.
    assert!(
        r.stdout.contains("11:22:33:44:55:66") || r.stdout.contains("112233445566"),
        "{r}"
    );
    // An archive is a *state* of the machine it names, not a machine of its own.
    assert!(!r.stdout.contains("installed-"), "{r}");
    assert!(r.stdout.contains("disarmed by a previous install"), "{r}");
}

#[test]
fn groups_shows_the_extends_chain_and_what_each_one_claims() {
    let c = Case::new(FLEET);
    let r = c.run(&["groups"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("extends base"), "{r}");
    assert!(r.stdout.contains("1 member(s)"), "{r}");
}

#[test]
fn status_counts_the_fleet_and_names_the_group_armed_ones() {
    let c = Case::new(&[
        (
            "groups/rack-a.ipxe",
            "# answer: member aa:bb:cc:dd:ee:ff\n#!ipxe\nchain http://x/a\n",
        ),
        (
            "groups/rack-a.toml",
            "members = [\"aa:bb:cc:dd:ee:ff\"]\n[g]\nx = 1\n",
        ),
    ]);
    let r = c.run(&["status"]);
    assert!(r.ok, "{r}");
    assert!(r.stdout.contains("armed by a group"), "{r}");
    assert!(r.stdout.contains("cannot disarm"), "{r}");
}

/// Zero problems is the normal state, so a non-zero exit here would cry wolf. `check` is
/// what keys an exit code on the answer set.
#[test]
fn status_succeeds_even_when_the_answer_set_has_problems() {
    let c = Case::new(&[("98fa9b50d810.toml", "extends = \"nowhere\"\n")]);
    let r = c.run(&["status"]);
    assert!(r.ok, "status must report, not judge\n{r}");
    assert!(r.stdout.contains("problem:"), "{r}");
    // And `check` still fails on the same set, which is the one that gates a deploy.
    assert!(!c.run(&["check"]).ok);
}

#[test]
fn the_json_form_is_parseable_and_says_the_same_thing() {
    let c = Case::new(FLEET);
    for (args, key) in [
        (["machines", "--json"], "machines"),
        (["groups", "--json"], "groups"),
        (["status", "--json"], "armed"),
    ] {
        let r = c.run(&args);
        assert!(r.ok, "{r}");
        let v: serde_json::Value =
            serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| panic!("{args:?}: {e}\n{r}"));
        assert!(v.get(key).is_some(), "{args:?} has no {key}: {r}");
    }
}

/// A machine holding two formats is the case the layout exists for, so asking for one of
/// them should not be a trick.
#[test]
fn render_can_be_asked_for_one_named_format() {
    let c = Case::new(&[
        ("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n"),
        (
            "98fa9b50d810.preseed",
            "d-i debian-installer/locale string fr_FR\n",
        ),
    ]);
    let toml = c.run(&["render", "98:fa:9b:50:d8:10", "--format", "toml"]);
    assert!(toml.ok, "{toml}");
    assert!(toml.stdout.contains("keyboard"), "{toml}");

    let preseed = c.run(&["render", "98:fa:9b:50:d8:10", "--format", "preseed"]);
    assert!(preseed.ok, "{preseed}");
    assert!(preseed.stdout.contains("debian-installer"), "{preseed}");

    let nonsense = c.run(&["render", "98:fa:9b:50:d8:10", "--format", "txt"]);
    assert!(!nonsense.ok, "txt is deliberately not a format\n{nonsense}");
}
