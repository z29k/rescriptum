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
    rescriptum media add FILE     register one already in the media directory
    rescriptum media add URL      fetch one into it, then register it
    rescriptum media check        re-verify every recorded digest, report what drifted
    rescriptum media ipxe ID      print the .ipxe answer that boots one image
    rescriptum boot dhcp-snippet  their DHCP server's two lines, generated
    rescriptum boot check         are the loaders a snippet names actually here?
    rescriptum boot bootstrap     print the stage-two script
    rescriptum boot menu          print the built-in menu
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
    RESCRIPTUM_BOOT_DIR           loaders and menus, served over TFTP
    RESCRIPTUM_TFTP_ADDR          TFTP listener              (default 0.0.0.0:69)
    RESCRIPTUM_BOOT_TIMEOUT_SECS  menu timeout               (default 15)
    RESCRIPTUM_USER / _GROUP      drop to these after binding port 69

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
#[cfg(not(feature = "boot"))]
pub fn media(_cfg: &Config, _args: &[String]) -> ExitCode {
    eprintln!("this binary was built without the `boot` feature, so it has no media commands");
    ExitCode::FAILURE
}

#[cfg(feature = "boot")]
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
        Some((cmd, rest)) if cmd == "prepare" && !rest.is_empty() => {
            media_prepare(cfg, &catalog, rest)
        }
        Some((cmd, rest)) if cmd == "export" && rest.len() == 2 => {
            media_export(&catalog, &rest[0], &rest[1])
        }
        Some((cmd, rest)) if cmd == "sources" && rest.len() < 2 => {
            media_sources(rest.first().map(String::as_str))
        }
        _ => {
            eprintln!(
                "usage: rescriptum media list\n\
                 \x20      rescriptum media sources [SOURCE]\n\
                 \x20      rescriptum media add FILE|URL [--sha256 D] [--as NAME]\n\
                 \x20      rescriptum media add --from SOURCE NAME\n\
                 \x20      rescriptum media check\n\
                 \x20      rescriptum media ipxe ID\n\
                 \x20      rescriptum media prepare ID [--as NAME] [--url URL]\n\
                 \x20      rescriptum media export ID FILE"
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "boot")]
fn media_sources(which: Option<&str>) -> ExitCode {
    use crate::boot::{fetch, sources};

    let Some(id) = which else {
        println!("{:<12} {:<16}  WHAT IT INSTALLS", "SOURCE", "NAME");
        for s in sources::SOURCES {
            println!("{:<12} {:<16}  {}", s.id, s.label, s.about);
        }
        println!();
        println!("`media sources <SOURCE>` lists what one offers, reading the vendor's own");
        println!("checksum index — so the list is current and the digests are theirs.");
        println!("`media add --from <SOURCE> <NAME>` fetches one.");
        return ExitCode::SUCCESS;
    };

    let Some(source) = sources::source(id) else {
        eprintln!(
            "no source called {id:?}. There are: {}",
            sources::SOURCES
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return ExitCode::FAILURE;
    };

    // Said before the wait, not after: on a NAS with a slow uplink this is several
    // seconds of apparent nothing, and silence there reads as a hang.
    eprintln!("reading {} …", source.index);
    let text = match fetch::fetch_text(source.index) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let offers = source.offers(&text);
    if offers.is_empty() {
        eprintln!(
            "{} answered, but nothing in it looks like an installer image. The index may \
             have moved or changed format.",
            source.index
        );
        return ExitCode::FAILURE;
    }

    println!("{} — {}", source.label, source.about);
    for offer in &offers {
        println!("  {}", offer.name);
    }
    println!();
    println!("  rescriptum media add --from {id} {}", offers[0].name);
    ExitCode::SUCCESS
}

#[cfg(feature = "boot")]
fn media_list(catalog: &crate::boot::catalog::Catalog) -> ExitCode {
    let listing = match catalog.listing() {
        Ok(listing) => listing,
        Err(e) => {
            eprintln!("cannot read {}: {e}", catalog.dir().display());
            return ExitCode::FAILURE;
        }
    };

    // **The last column is what makes the archive visible.** A base image is what the
    // vendor published, on disk, never modified; a prepared one is a few hundred bytes
    // of sidecar over it. Seeing which is which is the difference between a directory
    // and an archive somebody can reason about.
    println!(
        "{:<20} {:<8} {:<10} {:<24} {:>8}  SOURCE",
        "ID", "FAMILY", "ARCH", "VERSION", "SIZE"
    );
    for entry in &listing.entries {
        let source = match &entry.prepared {
            Some(prepared) => format!("{} -> {}", prepared.source_id, prepared.url),
            None => match &entry.digest {
                Some(digest) => format!("base, pinned {}", &digest[..12.min(digest.len())]),
                None => "base".to_string(),
            },
        };
        println!(
            "{:<20} {:<8} {:<10} {:<24} {:>8}  {}",
            entry.id,
            entry.family().label(),
            entry.arch().map(|a| a.label()).unwrap_or("-"),
            truncate(&entry.describe(), 24),
            human(entry.size),
            source,
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

/// `media add FILE` or `media add URL` — register an image, or fetch one and register it.
///
/// **No base image is ever in this repository or in a release.** An ISO is somebody
/// else's artefact, it is gigabytes, and it changes on its own schedule; it belongs on
/// the deployment's disk. Two ways to get it there, and the difference is only who does
/// the download:
///
/// - **Drop it in the media directory** — over SMB, over `scp`, from wherever it already
///   is — and register it. The native act on a NAS.
/// - **Give this a URL** and the server fetches it, through `curl` or `wget`, straight
///   into that directory.
///
/// Either way the file lands in the directory and **is never modified afterwards**. That
/// is what makes the media directory the archive: preparing an image produces a sidecar
/// and an injection applied on the wire, so the bytes on disk stay exactly what the
/// vendor published and their digest stays checkable against the vendor's own checksums.
#[cfg(feature = "boot")]
fn media_add(catalog: &crate::boot::catalog::Catalog, args: &[String]) -> ExitCode {
    let mut source: Option<&String> = None;
    let mut expected: Option<&String> = None;
    let mut name: Option<String> = None;
    let mut unverified = false;
    let mut from: Option<String> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--sha256" => match rest.next() {
                Some(digest) => expected = Some(digest),
                None => {
                    eprintln!("--sha256 wants a digest");
                    return ExitCode::FAILURE;
                }
            },
            "--as" => match rest.next() {
                Some(value) => name = Some(value.clone()),
                None => {
                    eprintln!("--as wants a filename");
                    return ExitCode::FAILURE;
                }
            },
            "--unverified" => unverified = true,
            "--from" => match rest.next() {
                Some(value) => from = Some(value.clone()),
                None => {
                    eprintln!("--from wants a source; `media sources` lists them");
                    return ExitCode::FAILURE;
                }
            },
            _ if source.is_none() => source = Some(arg),
            other => {
                eprintln!("unexpected argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }
    // **`--from` turns a name into a URL and a digest, both read from the vendor.** It is
    // not a shortcut around the digest rule — it is the strictest way to satisfy it that
    // does not involve a human copying 64 characters correctly. What it is *not* is a
    // signature check: the digest comes from the same host as the image, so it proves the
    // download matches what that vendor is publishing right now, and nothing about
    // whether the vendor is who you think. Somebody who needs that pastes a digest they
    // obtained out of band, which is why --sha256 stays.
    let resolved;
    let source = if let Some(id) = &from {
        let Some(wanted) = source else {
            eprintln!("--from {id} wants an image name too; `media sources {id}` lists them");
            return ExitCode::FAILURE;
        };
        let Some(src) = crate::boot::sources::source(id) else {
            eprintln!(
                "no source called {id:?}. There are: {}",
                crate::boot::sources::SOURCES
                    .iter()
                    .map(|s| s.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return ExitCode::FAILURE;
        };
        if expected.is_some() {
            eprintln!("--from and --sha256 disagree about where the digest comes from; pick one");
            return ExitCode::FAILURE;
        }
        eprintln!("reading {} …", src.index);
        let text = match crate::boot::fetch::fetch_text(src.index) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        };
        let offers = src.offers(&text);
        let Some(offer) = offers.iter().find(|o| o.name == *wanted) else {
            eprintln!("{} does not offer {wanted:?}.", src.label);
            if let Some(newest) = offers.first() {
                eprintln!("The newest it has is {}.", newest.name);
            }
            eprintln!("`media sources {id}` lists them all.");
            return ExitCode::FAILURE;
        };
        resolved = (offer.url.clone(), offer.digest.clone());
        eprintln!("{} publishes it as {}", src.label, &resolved.1[..16]);
        expected = Some(&resolved.1);
        &resolved.0
    } else {
        let Some(source) = source else {
            eprintln!(
                "usage: rescriptum media add FILE [--sha256 DIGEST]\n\
                 \x20      rescriptum media add URL --sha256 DIGEST [--as NAME.iso]\n\
                 \x20      rescriptum media add --from SOURCE NAME"
            );
            return ExitCode::FAILURE;
        };
        source
    };

    if let Some(digest) = expected
        && !crate::boot::sha256::is_digest(digest)
    {
        eprintln!("{digest:?} is not a SHA-256 — it is 64 hexadecimal characters");
        return ExitCode::FAILURE;
    }

    let path = if crate::boot::fetch::looks_like_a_url(source) {
        // **A digest is required for a URL**, and `--unverified` is what makes going
        // without one a deliberate act rather than the default. This decides what every
        // machine on the network installs; an image pulled off a mirror with nothing
        // checking it is the one place in this design where that would be a shrug.
        if expected.is_none() && !unverified {
            eprintln!(
                "fetching {source} needs --sha256, because nothing else would check what \
                 arrived. Vendors publish a SHA256SUMS beside the image.\n\
                 If you genuinely mean to skip it, say --unverified."
            );
            return ExitCode::FAILURE;
        }
        match crate::boot::fetch::fetch(
            source,
            catalog.dir(),
            name.as_deref(),
            expected.map(String::as_str),
        ) {
            Ok(fetched) => {
                eprintln!(
                    "fetched {} via {}{}",
                    human(fetched.bytes),
                    fetched.via,
                    if expected.is_some() {
                        ", digest verified"
                    } else {
                        " — UNVERIFIED"
                    }
                );
                fetched.path
            }
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        std::path::PathBuf::from(source)
    };

    if !path.is_file() {
        eprintln!("{} is not a file", path.display());
        return ExitCode::FAILURE;
    }

    // A file already on disk is registered where it lies: nothing is copied, so
    // registering one outside the directory would record a digest for a file the
    // listener cannot serve.
    let inside = path
        .parent()
        .map(|p| same_directory(p, catalog.dir()))
        .unwrap_or(false);
    if !inside {
        eprintln!(
            "{} is not in {} — put the image there first, then register it, or give a \
             URL and let the server fetch it.\n\
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
#[cfg(feature = "boot")]
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
#[cfg(feature = "boot")]
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

/// `media prepare ID` — the one command that removes the last external tool.
///
/// It writes a **sidecar**, not an image: two hundred bytes standing in for 1.5 GB. The
/// source is never modified, never copied, and its published digest stays verifiable;
/// the injection happens on the wire, and changing the answer URL later rewrites those
/// two hundred bytes rather than a gigabyte.
#[cfg(feature = "boot")]
fn media_prepare(
    cfg: &Config,
    catalog: &crate::boot::catalog::Catalog,
    args: &[String],
) -> ExitCode {
    let mut id: Option<&String> = None;
    let mut name: Option<String> = None;
    let mut url: Option<String> = None;
    let mut fingerprint: Option<String> = None;
    let mut token: Option<String> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut take = |what: &str| -> Option<String> {
            match rest.next() {
                Some(value) => Some(value.clone()),
                None => {
                    eprintln!("{what} wants a value");
                    None
                }
            }
        };
        match arg.as_str() {
            "--as" => match take("--as") {
                Some(v) => name = Some(v),
                None => return ExitCode::FAILURE,
            },
            "--url" => match take("--url") {
                Some(v) => url = Some(v),
                None => return ExitCode::FAILURE,
            },
            "--cert-fingerprint" => match take("--cert-fingerprint") {
                Some(v) => fingerprint = Some(v),
                None => return ExitCode::FAILURE,
            },
            "--token" => match take("--token") {
                Some(v) => token = Some(v),
                None => return ExitCode::FAILURE,
            },
            other if id.is_none() && !other.starts_with('-') => id = Some(arg),
            other => {
                eprintln!("unexpected argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    let Some(id) = id else {
        eprintln!("usage: rescriptum media prepare ID [--as NAME] [--url URL]");
        return ExitCode::FAILURE;
    };
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
    if entry.family() != crate::boot::probe::Family::Proxmox {
        // Every other family takes the answer's URL on the kernel command line, where
        // `media ipxe` already puts it. Injecting a file they never read would be a
        // no-op that looks like a step.
        eprintln!(
            "{id} is {}, and only Proxmox reads its answer's location from inside the image.              For every other family the URL goes on the kernel command line, which              `rescriptum media ipxe {id}` already writes.",
            entry.family().label()
        );
        return ExitCode::FAILURE;
    }

    let url = url.unwrap_or_else(|| format!("{}/proxmox", cfg.endpoints().answer));
    let derived = name.unwrap_or_else(|| format!("{id}-http"));
    if !crate::store::valid_id(&derived)
        || crate::boot::catalog::RESERVED_IDS.contains(&derived.as_str())
    {
        eprintln!("{derived:?} is not a usable identifier — it becomes part of a URL");
        return ExitCode::FAILURE;
    }

    // Plan it now rather than at request time, so a refusal is reported to the person
    // who can act on it instead of to a machine at 3am.
    let mode = crate::boot::patch::mode_file(&url, fingerprint.as_deref(), token.as_deref());
    let plan = match crate::boot::patch::add_file(
        &entry.path,
        "auto-installer-mode.toml",
        mode.as_bytes(),
    ) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut sidecar = String::from(
        "# rescriptum prepared entry — written by `media prepare`.\n         # Two hundred bytes standing in for an image: nothing was copied, and the\n         # source is untouched. The file is injected on the wire, so changing the URL\n         # below is all it takes to point this at a different answer endpoint.\n",
    );
    sidecar.push_str(&format!("source = {id}\n"));
    sidecar.push_str(&format!("prepare-url = {url}\n"));
    if let Some(fingerprint) = &fingerprint {
        sidecar.push_str(&format!("prepare-cert-fingerprint = {fingerprint}\n"));
    }
    if let Some(token) = &token {
        sidecar.push_str(&format!("prepare-token = {token}\n"));
    }
    // The length the offsets were computed against. A source that changed underneath
    // would be patched in the wrong place, and the catalogue refuses rather than
    // serving an image that mounts and is wrong.
    sidecar.push_str(&format!("source-bytes = {}\n", entry.size));
    if let Some(digest) = &entry.digest {
        sidecar.push_str(&format!("sha256 = {digest}\n"));
    }

    let path = catalog.dir().join(format!(
        "{derived}.{}",
        crate::boot::catalog::SIDECAR_EXTENSION
    ));
    if let Err(e) = std::fs::write(&path, sidecar) {
        eprintln!("cannot write {}: {e}", path.display());
        return ExitCode::FAILURE;
    }

    println!("{derived}  prepared from {id}");
    println!("  answer   {url}");
    println!(
        "  injects  /auto-installer-mode.toml ({} bytes)",
        mode.len()
    );
    println!(
        "  image    {} bytes (source {} + {} appended)",
        plan.len(),
        entry.size,
        plan.len() - entry.size
    );
    println!("  wrote    {}", path.display());
    println!();
    println!("Nothing was copied. Serve it as /{derived}/iso, or write it to a stick with");
    println!("  rescriptum media export {derived} /tmp/{derived}.iso");
    ExitCode::SUCCESS
}

/// `media export ID FILE` — materialise what the listener would have served.
///
/// **One code path with the streaming one.** A stick written from a different code path
/// than the one a machine downloads is a second implementation to keep honest, and the
/// difference would only show on somebody's desk.
#[cfg(feature = "boot")]
fn media_export(catalog: &crate::boot::catalog::Catalog, id: &str, to: &str) -> ExitCode {
    let entry = match catalog.get(id) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            eprintln!("no image called {id:?}");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("cannot read {}: {e}", catalog.dir().display());
            return ExitCode::FAILURE;
        }
    };
    let Some(prepared) = &entry.prepared else {
        eprintln!(
            "{id} is not a prepared entry — it is the image itself, so copy it.              `rescriptum media prepare {id}` makes one that needs exporting."
        );
        return ExitCode::FAILURE;
    };

    let mode = crate::boot::patch::mode_file(
        &prepared.url,
        prepared.fingerprint.as_deref(),
        prepared.token.as_deref(),
    );
    let plan = match crate::boot::patch::add_file(
        &entry.path,
        "auto-installer-mode.toml",
        mode.as_bytes(),
    ) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    eprintln!("writing {} bytes to {to} …", plan.len());
    if let Err(e) = plan.materialise(std::path::Path::new(to)) {
        eprintln!("cannot write {to}: {e}");
        return ExitCode::FAILURE;
    }
    println!("{to}");
    ExitCode::SUCCESS
}

/// Whether two paths name the same directory, resolving symlinks where it can. A media
/// directory reached as `/srv/media` and as `./media` is the same directory.
#[cfg(feature = "boot")]
fn same_directory(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(feature = "boot")]
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

#[cfg(feature = "boot")]
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

// ---- boot -----------------------------------------------------------------

#[cfg(not(feature = "boot"))]
pub fn boot(_cfg: &Config, _args: &[String]) -> ExitCode {
    eprintln!("this binary was built without the `boot` feature, so it has no boot commands");
    ExitCode::FAILURE
}

/// `boot dhcp-snippet` / `boot check` / `boot bootstrap` / `boot menu`.
#[cfg(feature = "boot")]
pub fn boot(cfg: &Config, args: &[String]) -> ExitCode {
    match args.split_first() {
        Some((cmd, rest)) if cmd == "dhcp-snippet" => boot_snippet(cfg, rest),
        Some((cmd, rest)) if cmd == "check" && rest.is_empty() => boot_check(cfg),
        Some((cmd, rest)) if cmd == "bootstrap" && rest.is_empty() => {
            print!(
                "{}",
                crate::boot::menu::bootstrap(&cfg.endpoints(), cfg.unclaimed_boots_local())
            );
            ExitCode::SUCCESS
        }
        Some((cmd, rest)) if cmd == "menu" && rest.is_empty() => boot_menu(cfg),
        _ => {
            eprintln!(
                "usage: rescriptum boot dhcp-snippet [--format F] [--one-loader]\n\
                 \x20      rescriptum boot check\n\
                 \x20      rescriptum boot bootstrap\n\
                 \x20      rescriptum boot menu\n\
                 \n\
                 \x20      --format: dnsmasq | isc | kea | powershell | pfsense | mikrotik"
            );
            ExitCode::FAILURE
        }
    }
}

/// The DHCP configuration an operator pastes into a server we do not speak to.
///
/// stdout is the snippet and stderr is everything else, so redirecting it produces a
/// file that can be included as-is.
#[cfg(feature = "boot")]
fn boot_snippet(cfg: &Config, args: &[String]) -> ExitCode {
    use crate::boot::dhcp;

    let mut format = dhcp::Format::Dnsmasq;
    let mut one_loader = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--one-loader" => one_loader = true,
            "--format" => match rest.next().map(|f| dhcp::Format::parse(f)) {
                Some(Some(parsed)) => format = parsed,
                Some(None) => {
                    eprintln!(
                        "unknown --format. Known: {}",
                        dhcp::Format::ALL
                            .iter()
                            .map(|f| f.label())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    return ExitCode::FAILURE;
                }
                None => {
                    eprintln!("--format wants a name");
                    return ExitCode::FAILURE;
                }
            },
            other => {
                eprintln!("unexpected argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    let (host, derived) = cfg.public_host();
    if derived {
        eprintln!(
            "# warning: RESCRIPTUM_PUBLIC_HOST is not set, so this snippet points machines \
             at {host}, derived by asking the routing table. A DHCP server handing out an \
             address the machines cannot reach is the hardest failure here to diagnose."
        );
    }
    print!(
        "{}",
        dhcp::snippet(
            format,
            &dhcp::Handoff {
                host,
                media: cfg.endpoints().media,
                version: env!("CARGO_PKG_VERSION"),
                one_loader,
            }
        )
    );
    ExitCode::SUCCESS
}

#[cfg(feature = "boot")]
fn boot_menu(cfg: &Config) -> ExitCode {
    let Some(dir) = &cfg.media_dir else {
        eprintln!("there is no media directory: RESCRIPTUM_MEDIA_DIR names one, and nothing does");
        return ExitCode::FAILURE;
    };
    let catalog = crate::boot::catalog::Catalog::new(dir);
    let listing = match catalog.listing() {
        Ok(listing) => listing,
        Err(e) => {
            eprintln!("cannot read {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    let style = crate::boot::menu::Style {
        title: cfg
            .boot_title
            .clone()
            .unwrap_or_else(crate::boot::menu::Style::default_title),
        timeout_millis: cfg.boot_timeout_millis(),
    };
    print!(
        "{}",
        crate::boot::menu::menu(&listing, &cfg.endpoints(), &style)
    );
    ExitCode::SUCCESS
}

/// `boot check` — is the boot chain actually complete?
///
/// **The failure this exists for is silent at the ROM.** A generated snippet names a
/// loader; if that file is not on disk, the machine asks for it, gets nothing, and
/// stops with no message anybody sees. Nothing else in the chain will notice.
#[cfg(feature = "boot")]
fn boot_check(cfg: &Config) -> ExitCode {
    use crate::boot::loaders;
    use crate::boot::tftp::ProbeResult;

    let mut failures = 0usize;
    let mut notes: Vec<String> = Vec::new();

    let Some(dir) = &cfg.boot_dir else {
        println!("boot assets are off — RESCRIPTUM_BOOT_DIR names a directory, and nothing does");
        println!("  nothing to check; TFTP is not running either");
        return ExitCode::SUCCESS;
    };
    println!("checking boot assets in {}", dir.display());

    // Every loader the table can hand out, plus the `snp` variants that exist because
    // the plain UEFI build cannot always see the NIC.
    for loader in loaders::loaders() {
        let path = dir.join(loader);
        if path.is_file() {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            println!("  ok   {loader} ({})", human(size));
        } else {
            // Named by a snippet, absent from the disk: the silent failure.
            println!(
                "  MISSING {loader} — every machine the snippet sends here will ask for it, \
                 get nothing, and stop"
            );
            failures += 1;
        }
        for variant in loaders::variants(loader) {
            if variant != loader && !dir.join(&variant).is_file() {
                notes.push(format!(
                    "{variant} is absent — it is the one to reach for when the plain UEFI \
                     build cannot see a NIC"
                ));
            }
        }
    }

    // **Can a loader actually be handed over?** Everything above is about the files
    // being on disk; this is about anything reaching them over UDP, which is the first
    // question a booting machine asks and the one nothing else here answers.
    //
    // **Binding is not the check, and finding that out cost a test.** A bind that
    // *succeeds* means nothing is listening — the degraded state, not the healthy one —
    // and a bind that fails cannot tell this server apart from another daemon squatting
    // the port, because both are `AddrInUse`. So the probe is a real read request: what
    // comes back is what a machine would get.
    match cfg.tftp_addr() {
        None => notes.push(
            "TFTP is off (RESCRIPTUM_TFTP_ADDR) — the loaders above are served over HTTP \
             at /boot/ and something else has to hand one over on port 69"
                .to_string(),
        ),
        Some(addr) => {
            // Ask for a loader that is actually here, so `Refused` means what it says.
            let wanted = loaders::loaders()
                .iter()
                .find(|l| dir.join(l).is_file())
                .copied()
                .unwrap_or("ipxe-undionly.kpxe");
            match crate::boot::tftp::probe(&addr, wanted, std::time::Duration::from_secs(2)) {
                ProbeResult::Served => println!("  ok   {addr} handed over {wanted}"),
                ProbeResult::Refused => {
                    println!(
                        "  BROKEN a TFTP server answered on {addr} but would not serve \
                         {wanted} — it is not this one, or not rooted at {}",
                        dir.display()
                    );
                    failures += 1;
                }
                // Silence splits on whether the port is even obtainable, and the two
                // halves are different problems. Cannot bind: something else holds it,
                // or the privilege is missing — the DSM case, where an upgrade drops the
                // `setcap` and the server warns and carries on. Can bind: nothing is
                // there at all, which is simply what "the server is not running" looks
                // like from a command run before starting it, so it is a note.
                ProbeResult::Silent => match std::net::UdpSocket::bind(&addr) {
                    Ok(_) => notes.push(format!(
                        "nothing is listening on {addr} — expected if the server is not \
                         running; if it is, it failed to bind and said so at startup"
                    )),
                    Err(e) => {
                        println!(
                            "  BROKEN nothing answers on {addr} and it cannot be bound \
                             either: {e}{} — the server still answers and still serves \
                             media, but a machine sent here by DHCP asks for a loader and \
                             gets nothing",
                            if addr.ends_with(":69") {
                                ". Port 69 is privileged: run as root and set \
                                 RESCRIPTUM_USER to drop afterwards, or grant the binary \
                                 cap_net_bind_service with setcap"
                            } else {
                                ""
                            }
                        );
                        failures += 1;
                    }
                },
            }
        }
    }

    // The embedded script in every loader already shipped chains to a fixed port, and
    // it can read no configuration — it is baked in before any deployment exists.
    let media = cfg.media_addr();
    let port = media.rsplit_once(':').map(|(_, p)| p).unwrap_or("");
    let expected = crate::config::DEFAULT_MEDIA_ADDR
        .rsplit_once(':')
        .map(|(_, p)| p)
        .unwrap_or("8001");
    if port != expected {
        println!(
            "  WARNING the media listener is on port {port}, but every loader already shipped \
             embeds a script chaining to :{expected}. The generated autoexec.ipxe and the \
             script's own relative fallback are the recovery; moving it back is the fix."
        );
        failures += 1;
    }

    // The logo, which the menu asks for and tolerates the absence of.
    if !dir.join("logo.png").is_file() {
        notes.push(
            "logo.png is absent — the menu's `console --picture` tolerates that and falls \
             back to the text console, so this is cosmetic"
                .to_string(),
        );
    }

    let (host, derived) = cfg.public_host();
    if derived {
        notes.push(format!(
            "RESCRIPTUM_PUBLIC_HOST is not set; generated scripts will name {host}"
        ));
    }

    println!(
        "  {} loader(s) the table can hand out",
        loaders::loaders().len()
    );
    for note in &notes {
        println!("  note: {note}");
    }

    if failures == 0 {
        println!("  ok — the loaders a snippet names are all here");
        ExitCode::SUCCESS
    } else {
        println!("  {failures} problem(s)");
        ExitCode::FAILURE
    }
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
