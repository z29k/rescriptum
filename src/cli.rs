//! `render` and `check`: the tools that make merged answer files reviewable.
//!
//! Merging creates a gap the old design did not have. An admin used to write a
//! complete file and validate it with `proxmox-auto-install-assistant
//! validate-answer`. Once a served answer is composed from layers, nobody has ever
//! seen the thing the installer receives — and a bad merge surfaces as a failed
//! unattended install at 3am. These subcommands close that gap.

use crate::config::Config;
use crate::facts::Facts;
use crate::select::Answers;
use crate::store::Store;
use std::process::ExitCode;

pub const USAGE: &str = "\
rescriptum — serves Proxmox VE answer files to the automated installer

USAGE:
    rescriptum                    run the server
    rescriptum render <mac>       print the answer a machine would receive
    rescriptum render --body FILE print the answer for a captured request body
    rescriptum render --query Q   ...for labels, e.g. \"mac=aa:bb&serial=7ABC1\"
    rescriptum check              validate the configured store
    rescriptum import <dir>       load a directory of TOML into the store
    rescriptum export <dir>       write the store out as a directory of TOML
    rescriptum config             show the configuration, and where each value comes from
    rescriptum config --json      the same, for a settings panel
    rescriptum config --value K   one value, for a script (never a credential)
    rescriptum config set K=V     edit the file RESCRIPTUM_ENV_FILE names
    rescriptum config unset K     comment a setting back out of it
    rescriptum media list         the installer images this server holds
    rescriptum media add FILE     register one: verify, probe, record its digest
    rescriptum media check        re-verify every recorded digest, report what drifted
    rescriptum media ipxe ID      print the .ipxe answer that boots one image
    rescriptum --help

ENVIRONMENT:
    RESCRIPTUM_ENV_FILE           read these from a file too  (the real environment wins)
    RESCRIPTUM_STORE              files | sqlite              (default files)
    RESCRIPTUM_ANSWERS_DIR        directory of answer files   (default /srv/answers)
    RESCRIPTUM_DB_PATH            sqlite database             (default /srv/answers.db)
    RESCRIPTUM_LISTEN_ADDR        listen address              (default 0.0.0.0:8000)
    RESCRIPTUM_WORKERS            runtime threads             (default: CPU count)
    RESCRIPTUM_MAX_CONNECTIONS    in-flight connections       (default 2048)
    RESCRIPTUM_TIMEOUT_SECS       header + connection timeout (default 10)
    RESCRIPTUM_LOG                all | problems | off        (default all)
    RESCRIPTUM_LOG_FILE           a path, stdout or stderr    (default stderr)

ADMIN API (requires RESCRIPTUM_STORE=sqlite; off unless RESCRIPTUM_ADMIN_ADDR is set):
    RESCRIPTUM_ADMIN_ADDR         admin listener, e.g. 127.0.0.1:9000
    RESCRIPTUM_ADMIN_TOKEN        bearer token, 16 characters or more (required)

BOOT MEDIA (off unless RESCRIPTUM_MEDIA_DIR is set):
    RESCRIPTUM_MEDIA_DIR          directory of installer images
    RESCRIPTUM_MEDIA_ADDR         media listener             (default 0.0.0.0:8001)
    RESCRIPTUM_MEDIA_TIMEOUT_SECS whole-transfer deadline    (default 600)
    RESCRIPTUM_MEDIA_MAX_CONNECTIONS  concurrent transfers   (default 16)
    RESCRIPTUM_PUBLIC_HOST        the host generated URLs name  (a host, not a URL)
    RESCRIPTUM_BOOT_ALLOW         CIDRs allowed to fetch media  (default: anyone)

VALIDATING A MERGED ANSWER:
    rescriptum render 98:fa:9b:50:d8:10 > /tmp/answer.toml
    proxmox-auto-install-assistant validate-answer /tmp/answer.toml
";

/// `render <mac>` / `render --body FILE` / `render --query "mac=…&serial=…"`
pub fn render(cfg: &Config, args: &[String]) -> ExitCode {
    let facts = match args {
        [flag, path] if flag == "--body" => match std::fs::read(path) {
            Ok(bytes) => Facts::new(None, &bytes),
            Err(e) => {
                eprintln!("cannot read {path}: {e}");
                return ExitCode::FAILURE;
            }
        },
        // The same labels a real request would carry, for testing selectors.
        [flag, query] if flag == "--query" => {
            let path = query.split('&').find_map(|p| p.strip_prefix("path="));
            Facts::from_request(path, Some(query), b"")
        }
        // A bare identifier claims nothing about what kind of identifier it is.
        [id] if !id.starts_with('-') => Facts::from_identity(id),
        _ => {
            eprintln!(
                "usage: rescriptum render <mac>\n\
                 \x20      rescriptum render --body FILE\n\
                 \x20      rescriptum render --query \"mac=…&serial=…\""
            );
            return ExitCode::FAILURE;
        }
    };

    let answers = match cfg.open_store() {
        Ok(store) => Answers::new(store),
        Err(e) => {
            eprintln!("cannot open the answer store: {e}");
            return ExitCode::FAILURE;
        }
    };
    for problem in answers.problems().unwrap_or_default() {
        eprintln!("warning: {problem}");
    }

    match answers.resolve(&facts) {
        Ok(Some(resolution)) => {
            eprintln!("# {}", resolution.how());
            print!("{}", resolution.body);
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!("no answer file applies (the server would return 404)");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The URL segment that asks for this format, so `check` can interrogate a document the
/// way its installer would rather than resolving it unconstrained.
fn endpoint_segment(format: &str) -> &str {
    match format {
        // The only extension that is not itself an endpoint alias.
        "seed" => "preseed",
        other => other,
    }
}

/// The tool that can say whether a rendered answer is valid *for its installer*.
///
/// `check` only ever knew that documents were well-formed and merged cleanly, which is
/// not the same as being a legal kickstart. When the real validator is on PATH there is
/// no reason not to ask it.
fn validator_for(format: &str) -> Option<(&'static str, Vec<&'static str>)> {
    match format {
        "toml" => Some(("proxmox-auto-install-assistant", vec!["validate-answer"])),
        "xml" | "autoyast" | "unattend" => Some(("xmllint", vec!["--noout"])),
        "ks" => Some(("ksvalidator", vec![])),
        _ => None,
    }
}

fn on_path(program: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

/// Run the installer's own validator over a rendered answer.
///
/// Returns `None` when there is no validator for this format, or it is not installed —
/// which is reported once rather than treated as a failure: a checker that refuses to
/// run without optional tooling is a checker nobody runs.
fn validate_rendered(format: &str, body: &str) -> Option<Result<(), String>> {
    let (program, args) = validator_for(format)?;
    if !on_path(program) {
        return None;
    }

    let path = std::env::temp_dir().join(format!(
        "rescriptum-check-{}-{}.{format}",
        std::process::id(),
        crate::log::timestamp().replace(':', "")
    ));
    if let Err(e) = std::fs::write(&path, body) {
        return Some(Err(format!("cannot write a temporary file: {e}")));
    }

    let result = std::process::Command::new(program)
        .args(&args)
        .arg(&path)
        .output();
    let _ = std::fs::remove_file(&path);

    Some(match result {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let mut message = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if message.is_empty() {
                message = String::from_utf8_lossy(&out.stdout).trim().to_string();
            }
            if message.is_empty() {
                message = format!("{program} exited with {}", out.status);
            }
            Err(message)
        }
        Err(e) => Err(format!("could not run {program}: {e}")),
    })
}

/// `check` — render everything the directory can produce and report what breaks.
pub fn check(cfg: &Config) -> ExitCode {
    let answers = match cfg.open_store() {
        Ok(store) => Answers::new(store),
        Err(e) => {
            println!("cannot open the answer store: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("checking {}", answers.describe());

    let problems = match answers.problems() {
        Ok(p) => p,
        Err(e) => {
            println!("cannot read the answers directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut failures = problems.len();
    for problem in &problems {
        println!("  problem: {problem}");
    }

    let groups = answers.group_names().unwrap_or_default();
    let machines = answers.machine_ids().unwrap_or_default();
    println!(
        "  {} group(s), {} machine file(s)",
        groups.len(),
        machines.len()
    );

    // Every group must render through at least one of its members, and every machine
    // file must render — that is what actually exercises the merge.
    for (group, origin) in &groups {
        let members = answers.group_members(group).unwrap_or_default();
        let matchers = answers.group_matchers(group).unwrap_or_default();

        if !matchers.is_empty() {
            let criteria: Vec<String> = matchers.iter().map(|(k, v)| format!("{k}={v}")).collect();
            println!("  group {group:?} selects on {}", criteria.join(" "));
            // A selector is tested against a real request, so `check` cannot try it
            // here — say so rather than implying it was verified.
            if members.is_empty() {
                println!("    (verify with: rescriptum render --query \"...\")");
                continue;
            }
        }
        if members.is_empty() && matchers.is_empty() {
            println!(
                "  note: group {group:?} ({origin}) has neither members nor a `match` block \
                 (reachable only via `extends`)"
            );
            continue;
        }
        for member in &members {
            let group_format = answers
                .group_format(group)
                .unwrap_or_default()
                .unwrap_or_else(|| "toml".to_string());
            let path = format!("/{}/{member}", endpoint_segment(&group_format));
            if let Err(e) =
                answers.resolve(&Facts::from_request(Some(&path), None, member.as_bytes()))
            {
                println!("  FAIL group {group:?} member {member:?}: {e}");
                failures += 1;
            }
        }
    }
    let mut validated = 0usize;
    let mut skipped: Vec<String> = Vec::new();

    for (machine, format) in answers.machine_documents().unwrap_or_default() {
        let machine = &machine;
        let path = format!("/{}/{machine}", endpoint_segment(&format));
        match answers.resolve(&Facts::from_request(Some(&path), None, machine.as_bytes())) {
            Ok(Some(resolution)) => {
                match validate_rendered(&resolution.format_name, &resolution.body) {
                    Some(Ok(())) => validated += 1,
                    Some(Err(e)) => {
                        println!("  FAIL machine {machine:?}: {e}");
                        failures += 1;
                    }
                    None => {
                        let label = resolution.format_name.clone();
                        if !skipped.contains(&label) {
                            skipped.push(label);
                        }
                    }
                }
            }
            Ok(None) => {
                println!("  FAIL machine {machine:?}: resolves to nothing");
                failures += 1;
            }
            Err(e) => {
                println!("  FAIL machine {machine:?}: {e}");
                failures += 1;
            }
        }
    }

    if validated > 0 {
        println!("  {validated} answer(s) validated by their installer's own tool");
    }
    for format in &skipped {
        match validator_for(format) {
            Some((program, _)) => {
                println!("  note: {format} answers not schema-checked — {program} is not on PATH")
            }
            None => println!("  note: no schema validator exists for {format} answers"),
        }
    }

    if failures == 0 {
        println!("  ok — everything renders");
        println!("\nWell-formed and merging cleanly is not the same as valid for an");
        println!("installer. Where a validator exists and is installed it was used above;");
        println!("install proxmox-auto-install-assistant, xmllint or ksvalidator for the rest.");
        ExitCode::SUCCESS
    } else {
        println!("  {failures} problem(s)");
        ExitCode::FAILURE
    }
}

/// `import <dir>` — copy a directory of answer files into the configured store.
pub fn import(cfg: &Config, args: &[String]) -> ExitCode {
    let [dir] = args else {
        eprintln!("usage: rescriptum import <dir>");
        return ExitCode::FAILURE;
    };

    let source = crate::store::FileStore::new(dir);
    let snapshot = match source.snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {dir}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let target = match cfg.open_store() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open the answer store: {e}");
            return ExitCode::FAILURE;
        }
    };

    copy(
        &snapshot,
        target.as_ref(),
        &source.describe(),
        &target.describe(),
    )
}

/// `export <dir>` — write the configured store out as a directory of answer files.
pub fn export(cfg: &Config, args: &[String]) -> ExitCode {
    let [dir] = args else {
        eprintln!("usage: rescriptum export <dir>");
        return ExitCode::FAILURE;
    };

    let source = match cfg.open_store() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open the answer store: {e}");
            return ExitCode::FAILURE;
        }
    };
    let snapshot = match source.snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read the store: {e}");
            return ExitCode::FAILURE;
        }
    };
    let target = crate::store::FileStore::new(dir);

    copy(&snapshot, &target, &source.describe(), &target.describe())
}

/// The shared half of import and export — they differ only in which end is a directory.
fn copy(
    snapshot: &crate::store::Snapshot,
    target: &dyn crate::store::StoreWrite,
    from: &str,
    to: &str,
) -> ExitCode {
    println!("copying {from} -> {to}");
    for problem in &snapshot.problems {
        println!("  warning: {problem}");
    }

    let mut failures = 0;
    for group in &snapshot.groups {
        if let Err(e) = target.put_group(&group.name, &group.format, &group.body) {
            println!("  FAIL group {}: {e}", group.name);
            failures += 1;
        }
    }
    for machine in &snapshot.machines {
        if let Err(e) = target.put_machine(&machine.id, &machine.format, &machine.body) {
            println!("  FAIL machine {}: {e}", machine.id);
            failures += 1;
        }
    }
    for default in &snapshot.fallbacks {
        if let Err(e) = target.put_default(&default.format, &default.body) {
            println!("  FAIL default.{}: {e}", default.format);
            failures += 1;
        }
    }

    println!(
        "  {} group(s), {} machine(s){}",
        snapshot.groups.len(),
        snapshot.machines.len(),
        match snapshot.fallbacks.len() {
            0 => String::new(),
            n => format!(", {n} default(s)"),
        }
    );
    if failures == 0 {
        println!("  ok — now run `check` against the target");
        ExitCode::SUCCESS
    } else {
        println!("  {failures} failure(s)");
        ExitCode::FAILURE
    }
}

// ---- media ----------------------------------------------------------------

/// `media list|add|check|ipxe` — the boot-media half of the CLI.
///
/// **Preparation and asset management are commands, never requests.** The rule that
/// keeps the server honest is that no request ever triggers work proportional to the
/// size of an image; hashing 1.5 GB happens here, once, and the result is recorded
/// beside the image so nothing ever recomputes it.
pub fn media(cfg: &Config, args: &[String]) -> ExitCode {
    let Some(dir) = &cfg.media_dir else {
        eprintln!("there is no media directory: RESCRIPTUM_MEDIA_DIR names one, and nothing does");
        return ExitCode::FAILURE;
    };
    let catalog = crate::boot::catalog::Catalog::new(dir);

    match args.split_first() {
        Some((cmd, rest)) if cmd == "list" && rest.is_empty() => media_list(&catalog),
        Some((cmd, rest)) if cmd == "add" && !rest.is_empty() => media_add(&catalog, rest),
        Some((cmd, rest)) if cmd == "check" && rest.is_empty() => media_check(&catalog),
        Some((cmd, rest)) if cmd == "ipxe" && rest.len() == 1 => {
            media_ipxe(cfg, &catalog, &rest[0])
        }
        _ => {
            eprintln!(
                "usage: rescriptum media list\n\
                 \x20      rescriptum media add FILE [--sha256 DIGEST]\n\
                 \x20      rescriptum media check\n\
                 \x20      rescriptum media ipxe ID"
            );
            ExitCode::FAILURE
        }
    }
}

fn media_list(catalog: &crate::boot::catalog::Catalog) -> ExitCode {
    let listing = match catalog.listing() {
        Ok(listing) => listing,
        Err(e) => {
            eprintln!("cannot read {}: {e}", catalog.dir().display());
            return ExitCode::FAILURE;
        }
    };

    println!(
        "{:<20} {:<8} {:<10} {:<28} {:>8}  PINNED",
        "ID", "FAMILY", "ARCH", "VERSION", "SIZE"
    );
    for entry in &listing.entries {
        println!(
            "{:<20} {:<8} {:<10} {:<28} {:>8}  {}",
            entry.id,
            entry.family().label(),
            entry.arch().map(|a| a.label()).unwrap_or("—"),
            truncate(&entry.describe(), 28),
            human(entry.size),
            match &entry.digest {
                Some(digest) => digest[..12.min(digest.len())].to_string(),
                None => "—".to_string(),
            },
        );
    }
    if listing.entries.is_empty() {
        println!("(nothing in {})", catalog.dir().display());
    }
    for problem in &listing.problems {
        eprintln!("warning: {problem}");
    }
    ExitCode::SUCCESS
}

fn media_add(catalog: &crate::boot::catalog::Catalog, args: &[String]) -> ExitCode {
    let mut path: Option<&String> = None;
    let mut expected: Option<&String> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--sha256" {
            match rest.next() {
                Some(digest) => expected = Some(digest),
                None => {
                    eprintln!("--sha256 wants a digest");
                    return ExitCode::FAILURE;
                }
            }
        } else if path.is_none() {
            path = Some(arg);
        } else {
            eprintln!("unexpected argument {arg:?}");
            return ExitCode::FAILURE;
        }
    }
    let Some(path) = path.map(std::path::PathBuf::from) else {
        eprintln!("usage: rescriptum media add FILE [--sha256 DIGEST]");
        return ExitCode::FAILURE;
    };

    if let Some(digest) = expected
        && !crate::boot::sha256::is_digest(digest)
    {
        eprintln!("{digest:?} is not a SHA-256 — it is 64 hexadecimal characters");
        return ExitCode::FAILURE;
    }
    if !path.is_file() {
        eprintln!("{} is not a file", path.display());
        return ExitCode::FAILURE;
    }

    // **The server never downloads images; it receives them.** Dropping the file into
    // the directory is the native act — over SMB, over scp, from wherever the ISO
    // already is — and this only registers what is already there. Registering something
    // outside the directory would record a digest for a file the listener cannot serve.
    let inside = path
        .parent()
        .map(|p| same_directory(p, catalog.dir()))
        .unwrap_or(false);
    if !inside {
        eprintln!(
            "{} is not in {} — put the image there first, then register it.\n\
             Nothing is copied: the catalogue serves the file where it lies.",
            path.display(),
            catalog.dir().display()
        );
        return ExitCode::FAILURE;
    }

    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !crate::store::valid_id(&id) {
        eprintln!("{id:?} is not a usable identifier — it becomes part of a URL");
        return ExitCode::FAILURE;
    }
    if crate::boot::catalog::RESERVED_IDS.contains(&id.as_str()) {
        eprintln!(
            "{id:?} is a reserved name — the media listener answers /{id} itself, so an \
             entry called that could never be reached. Rename the file."
        );
        return ExitCode::FAILURE;
    }

    // Progress, because a minute of silence reads as a hang.
    eprintln!("hashing {} …", path.display());
    let mut last = 0u64;
    let digest = match crate::boot::sha256::file(&path, |done, total| {
        let percent = if total == 0 { 100 } else { done * 100 / total };
        if percent >= last + 10 {
            last = percent - percent % 10;
            eprintln!("  {last}% ({} of {})", human(done), human(total));
        }
    }) {
        Ok(digest) => digest,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };

    if let Some(expected) = expected
        && !expected.eq_ignore_ascii_case(&digest)
    {
        // Loud and fatal. A mismatch here is either a truncated download or the wrong
        // file, and both install the wrong thing on every machine that asks.
        eprintln!("digest mismatch — nothing was recorded");
        eprintln!("  expected {expected}");
        eprintln!("  found    {digest}");
        return ExitCode::FAILURE;
    }

    let probed = match crate::boot::probe::probe(&path) {
        Ok(probed) => probed,
        Err(e) => {
            // Still registrable: an image nothing places can be served whole, and that
            // is a normal thing to want.
            eprintln!("note: cannot read {} as an image ({e})", path.display());
            Default::default()
        }
    };

    let sidecar = crate::boot::catalog::Sidecar::path_for(&path);
    if let Err(e) = std::fs::write(
        &sidecar,
        crate::boot::catalog::Sidecar::render(&digest, &probed),
    ) {
        eprintln!("cannot write {}: {e}", sidecar.display());
        return ExitCode::FAILURE;
    }

    println!("{id}  {digest}");
    println!(
        "  {} {}",
        probed
            .family
            .map(|f| f.label().to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        probed.version.clone().unwrap_or_default()
    );
    match (&probed.kernel, &probed.initrd) {
        (Some(kernel), Some(initrd)) => {
            println!("  kernel {kernel}");
            println!("  initrd {initrd}");
            if probed.external {
                println!("  (both beside the image — this looks like `prepare-iso --pxe` output)");
            }
            if probed.zstd_initrd {
                // The assistant's own source says "iPXE does not support a
                // zstd-compressed initrd" when it recompresses to gzip. Whether that
                // binds through our chain is a bench question; saying so is not.
                println!(
                    "  note: the initrd is zstd-compressed. The Proxmox assistant recompresses\n\
                     \x20       it to gzip when it splits an image, on the grounds that iPXE does\n\
                     \x20       not support zstd. If a loader refuses it, run:\n\
                     \x20       proxmox-auto-install-assistant prepare-iso {} --pxe --output DIR",
                    path.display()
                );
            }
        }
        _ => println!("  no kernel or initrd found — servable whole, but not as a boot stanza"),
    }
    println!("  wrote {}", sidecar.display());
    ExitCode::SUCCESS
}

/// `media check` — re-verify what was recorded. Its exit code is a contract, like
/// `check`'s: `deploy.sh` keys on it.
fn media_check(catalog: &crate::boot::catalog::Catalog) -> ExitCode {
    let listing = match catalog.listing() {
        Ok(listing) => listing,
        Err(e) => {
            println!("cannot read {}: {e}", catalog.dir().display());
            return ExitCode::FAILURE;
        }
    };
    println!("checking {}", catalog.describe());

    let mut failures = listing.problems.len();
    for problem in &listing.problems {
        println!("  problem: {problem}");
    }

    let mut pinned = 0usize;
    let mut unpinned: Vec<&str> = Vec::new();
    for entry in &listing.entries {
        let Some(recorded) = &entry.digest else {
            unpinned.push(&entry.id);
            continue;
        };
        match crate::boot::sha256::file(&entry.path, |_, _| {}) {
            Ok(digest) if digest.eq_ignore_ascii_case(recorded) => pinned += 1,
            Ok(digest) => {
                // An image that changed under a recorded digest is the one failure that
                // silently installs something nobody reviewed.
                println!(
                    "  FAIL {}: the image no longer matches what was recorded",
                    entry.id
                );
                println!("       recorded {recorded}");
                println!("       found    {digest}");
                failures += 1;
            }
            Err(e) => {
                println!("  FAIL {}: {e}", entry.id);
                failures += 1;
            }
        }
        if entry.probed.zstd_initrd {
            println!(
                "  note: {}'s initrd is zstd — `prepare-iso --pxe` recompresses to gzip",
                entry.id
            );
        }
    }

    println!(
        "  {} image(s), {pinned} verified against a recorded digest",
        listing.entries.len()
    );
    for id in &unpinned {
        println!("  note: {id} has no recorded digest — `media add` records one");
    }

    if failures == 0 {
        println!("  ok — everything recorded still matches");
        ExitCode::SUCCESS
    } else {
        println!("  {failures} problem(s)");
        ExitCode::FAILURE
    }
}

/// `media ipxe ID` — **print a script; do not install one.**
///
/// The output is an ordinary `.ipxe` answer document. Saved into the answers directory
/// it goes through the existing selection, layering and templating unchanged, which is
/// the altitude that keeps the model intact: the server does not become clever about
/// booting, it gains a generator.
///
/// stdout is the script and stderr is everything else, so `media ipxe … > file` works.
fn media_ipxe(cfg: &Config, catalog: &crate::boot::catalog::Catalog, id: &str) -> ExitCode {
    let entry = match catalog.get(id) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            eprintln!("no image called {id:?} — `rescriptum media list` shows what there is");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("cannot read {}: {e}", catalog.dir().display());
            return ExitCode::FAILURE;
        }
    };

    let (host, derived) = cfg.public_host();
    if derived {
        eprintln!(
            "# warning: RESCRIPTUM_PUBLIC_HOST is not set, so this script names {host}, \
             derived by asking the routing table. Set it if that is not the address the \
             machines can reach."
        );
    }
    match crate::boot::stanza::ipxe(&entry, &cfg.endpoints()) {
        Ok(script) => {
            eprintln!(
                "# {}",
                crate::boot::stanza::where_the_answer_goes(entry.family())
            );
            print!("{script}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Whether two paths name the same directory, resolving symlinks where it can. A media
/// directory reached as `/srv/media` and as `./media` is the same directory.
fn same_directory(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}{}", UNITS[0])
    } else {
        format!("{size:.1}{}", UNITS[unit])
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

// ---- config ---------------------------------------------------------------

/// `config` / `config --json` / `config set KEY=VALUE` / `config unset KEY`
///
/// **This one deliberately does not take a `Config`.** Every other subcommand is handed
/// one that `main` already built and validated, which is exactly what cannot be relied on
/// here: a file that will not parse, or a token one character too short, are the states in
/// which somebody reaches for this command. It loads the file itself, reports what is
/// wrong rather than dying of it, and is the way back out.
///
/// The exit code is a contract, like `check`'s: **zero when the configuration would
/// start**, one when it would not, or when a write was refused.
pub fn config(args: &[String]) -> ExitCode {
    let path = std::env::var(crate::envfile::ENV_FILE)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());

    match args.split_first() {
        None => show(path.as_deref(), false),
        Some((flag, rest)) if flag == "--json" && rest.is_empty() => show(path.as_deref(), true),
        Some((flag, rest)) if flag == "--value" && rest.len() == 1 => {
            value(path.as_deref(), &rest[0])
        }
        Some((cmd, rest)) if cmd == "set" && !rest.is_empty() => edit(path.as_deref(), rest, true),
        Some((cmd, rest)) if cmd == "unset" && !rest.is_empty() => {
            edit(path.as_deref(), rest, false)
        }
        _ => {
            eprintln!(
                "usage: rescriptum config\n\
                 \x20      rescriptum config --json\n\
                 \x20      rescriptum config --value KEY\n\
                 \x20      rescriptum config set KEY=VALUE [KEY=VALUE …]\n\
                 \x20      rescriptum config unset KEY [KEY …]"
            );
            ExitCode::FAILURE
        }
    }
}

/// One value, on stdout, for a script that wants it.
///
/// The alternative is a shell reading the env file with `sed`, which gets the *defaults*
/// wrong: a variable absent from the file is not unset, it is whatever this program falls
/// back to. Precedence goes the same way — the environment beats the file — and neither is
/// visible to something grepping a file.
///
/// **A secret is never printed**, whatever is asked. Exit code one means "no such value",
/// so `if v=$(rescriptum config --value KEY)` reads correctly.
fn value(path: Option<&str>, key: &str) -> ExitCode {
    let Some(known) = crate::config::KNOWN.iter().find(|k| k.key == key) else {
        eprintln!("{key} is not a variable this program reads");
        return ExitCode::FAILURE;
    };
    if known.secret {
        eprintln!("{key} is a credential and will not be printed");
        return ExitCode::FAILURE;
    }

    let (file, _, unreadable) = load_file(path);
    if let Some(reason) = unreadable {
        eprintln!("{reason}");
        return ExitCode::FAILURE;
    }
    match crate::config::settings(file.as_ref(), from_environment)
        .into_iter()
        .find(|s| s.key == key)
        .and_then(|s| s.value)
    {
        Some(v) => {
            println!("{v}");
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

/// Load the named file, if one is named at all.
///
/// A file that will not parse still **prints**, rather than being a single error line:
/// seeing the other twelve variables next to the reason the file is broken is what makes
/// this usable. It is returned separately from the warnings because it is not one — an
/// unreadable env file is a startup *error*, so it has to reach the exit code.
fn load_file(path: Option<&str>) -> (Option<crate::envfile::EnvFile>, Vec<String>, Option<String>) {
    match path {
        None => (None, Vec::new(), None),
        Some(p) => match crate::envfile::EnvFile::load(p) {
            Ok(file) => {
                let warnings = file.warnings.clone();
                (Some(file), warnings, None)
            }
            Err(e) => (None, Vec::new(), Some(e)),
        },
    }
}

/// The environment as `settings` and `Config` both want to read it.
fn from_environment(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Rebuild the configuration exactly as the server would, from a file plus the real
/// environment, so that what this command reports is what would actually happen.
fn effective(file: Option<&crate::envfile::EnvFile>) -> Config {
    Config::from_lookup(|key| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| file.and_then(|f| f.get(key)))
    })
}

fn show(path: Option<&str>, as_json: bool) -> ExitCode {
    let (file, problems, unreadable) = load_file(path);
    let settings = crate::config::settings(file.as_ref(), from_environment);
    // Either of these stops a server starting, so either of them is the answer here.
    // A file that cannot be read comes first: it is the more basic failure, and the
    // configuration `validate` would inspect is not the one the operator wrote.
    let refusal = unreadable.or_else(|| effective(file.as_ref()).validate().err());

    if as_json {
        println!(
            "{}",
            as_json_text(path, &settings, &problems, refusal.as_deref())
        );
    } else {
        match path {
            Some(p) => println!("env file: {p}"),
            // Not an error. Plenty of deployments configure a container or a unit file
            // and have nothing for this to edit; saying so beats an empty line.
            None => println!(
                "env file: none — {} names one, and nothing does",
                crate::envfile::ENV_FILE
            ),
        }
        println!();

        let width = settings.iter().map(|s| s.key.len()).max().unwrap_or(0);
        for s in &settings {
            let shown = match (&s.value, s.secret, s.set) {
                (_, true, true) => "(set)".to_string(),
                (Some(v), _, _) => v.clone(),
                _ => "(not set)".to_string(),
            };
            println!("  {:<width$}  {:<32}  {}", s.key, shown, s.source.label());
        }

        for problem in &problems {
            eprintln!("warning: {problem}");
        }
        if let Some(reason) = &refusal {
            eprintln!("this configuration would not start: {reason}");
        }
    }

    if refusal.is_some() {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn as_json_text(
    path: Option<&str>,
    settings: &[crate::config::Setting],
    problems: &[String],
    refusal: Option<&str>,
) -> String {
    let rows: Vec<serde_json::Value> = settings
        .iter()
        .map(|s| {
            serde_json::json!({
                "key": s.key,
                // Null for a secret, always: this is what anything reading the output
                // renders, and it must not be able to render a token by accident.
                "value": s.value,
                "set": s.set,
                "source": s.source.label(),
                "default": s.default,
                "secret": s.secret,
                "help": s.help,
            })
        })
        .collect();

    serde_json::json!({
        "env_file": path,
        "writable": path.is_some_and(writable),
        "settings": rows,
        "warnings": problems,
        "starts": refusal.is_none(),
        "error": refusal,
    })
    .to_string()
}

/// Whether this process could actually rewrite the file — which is not the same question
/// as whether it exists. A panel that offered an editable form over a file it cannot write
/// would fail at the save button, having promised otherwise.
///
/// Asked by trying rather than by reading permission bits, so that ownership, groups, ACLs
/// and a read-only mount all count — the same reasoning as the answers-directory check at
/// startup, and the same failure a packaged, non-root run meets first. Opening for append
/// changes nothing; the handle is dropped unused.
fn writable(path: &str) -> bool {
    let path = std::path::Path::new(path);
    if path.exists() {
        return std::fs::OpenOptions::new().append(true).open(path).is_ok();
    }
    // Not there yet, so it would be created and the directory is the real question.
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let probe = dir.join(format!(".rescriptum-writable.{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn edit(path: Option<&str>, args: &[String], setting: bool) -> ExitCode {
    let Some(path) = path else {
        eprintln!(
            "there is no file to edit: {} names one, and nothing does",
            crate::envfile::ENV_FILE
        );
        return ExitCode::FAILURE;
    };

    let mut changes: std::collections::BTreeMap<String, Option<String>> = Default::default();
    for arg in args {
        let (key, value) = if setting {
            match arg.split_once('=') {
                Some((k, v)) => (k.trim().to_string(), Some(v.to_string())),
                None => {
                    eprintln!("expected KEY=VALUE, found {arg:?}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            (arg.trim().to_string(), None)
        };

        // A misspelled name would otherwise be written, read back as a stranger, and
        // warned about only at the next start — by which time nobody connects the two.
        if !crate::envfile::KNOWN_KEYS.contains(&key.as_str()) {
            eprintln!("{key} is not a variable this program reads — check the spelling");
            return ExitCode::FAILURE;
        }
        if changes.insert(key.clone(), value).is_some() {
            eprintln!("{key} given twice");
            return ExitCode::FAILURE;
        }
    }

    // The file has to be readable and sound before it can be edited: rewriting one that
    // does not parse would bake the mistake in rather than report it.
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // A file that is not there yet is one to create — that is how a fresh deployment
        // gets its first setting.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = crate::envfile::parse(&text) {
        eprintln!("{path} does not parse, so it will not be edited: {e}");
        return ExitCode::FAILURE;
    }

    let rewritten = match crate::envfile::rewrite(&text, &changes) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    // **A write may never leave a server that cannot start.** The same reasoning as the
    // admin API's rollback: the panel doing the writing is reached over the very service
    // this would stop, so getting it wrong costs somebody an SSH session at best.
    let parsed = match crate::envfile::parse(&rewritten) {
        Ok(vars) => vars,
        Err(e) => {
            eprintln!("refusing to write a file this program could not read back: {e}");
            return ExitCode::FAILURE;
        }
    };
    let would = Config::from_lookup(|key| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| parsed.get(key).cloned())
    });
    if let Err(reason) = would.validate() {
        eprintln!("refused: this would leave a server that cannot start — {reason}");
        return ExitCode::FAILURE;
    }

    if let Err(e) = crate::envfile::write_atomic(std::path::Path::new(path), &rewritten) {
        eprintln!("cannot write {path}: {e}");
        return ExitCode::FAILURE;
    }

    // Said last, and only on success. A value the environment overrides is written all
    // the same — the file is still the record — but pretending it took effect would be
    // the silent half-failure this whole module exists to remove.
    for key in changes.keys() {
        if std::env::var(key).is_ok_and(|v| !v.trim().is_empty()) {
            eprintln!(
                "note: {key} is also set in the environment, which wins — this file will \
                 not change what a server started from it uses"
            );
        }
    }
    eprintln!("wrote {path}");
    ExitCode::SUCCESS
}
