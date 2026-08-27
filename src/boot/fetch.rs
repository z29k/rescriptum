//! Getting an installer image onto the server, over the network.
//!
//! **No base image is ever in this repository, and none is in a release.** An ISO is
//! somebody else's copyrighted artefact, it is one to four gigabytes, and it changes on
//! its own schedule — three separate reasons it belongs on the deployment's disk rather
//! than in ours. So the server fetches it, once, into the directory it serves from.
//!
//! ## Why this shells out
//!
//! **There is no TLS in this binary.** rustls plus a root store is forty-odd crates and
//! over a megabyte on armv7 — for a job every host already has a tool for. So this runs
//! `curl`, or `wget` if that is what is installed, and says plainly when it finds
//! neither. The precedent is `check` calling `proxmox-auto-install-assistant`: an
//! external tool used when it is there, never depended upon. Without one, the answer is
//! the one that always worked — download it yourself and drop it in the directory.
//!
//! ## What "the base ISO is the archive" means
//!
//! The file this writes is **never modified afterwards**. Preparing an image produces a
//! sidecar and an injection applied on the wire, so the bytes on disk stay exactly what
//! the vendor published and their digest stays verifiable against the vendor's own
//! checksum file. The media directory is the archive; everything derived from it is a
//! few hundred bytes.

use std::path::{Path, PathBuf};
use std::process::Command;

/// How the image got here, for a caller to report.
#[derive(Debug)]
pub struct Fetched {
    pub path: PathBuf,
    pub bytes: u64,
    /// The tool that did it, so a log line says what to install if it is missing.
    pub via: &'static str,
}

/// Fetch `url` into `dir`, atomically, verifying it against `expected` before it counts.
///
/// The download lands on a `.part` name and is renamed only once the digest matches.
/// **A partial download must never become a catalogue entry**: the catalogue probes
/// whatever it finds, and a truncated ISO probes as an unknown image that a machine
/// would then try to boot.
pub fn fetch(
    url: &str,
    dir: &Path,
    name: Option<&str>,
    expected: Option<&str>,
) -> Result<Fetched, String> {
    let name = match name {
        Some(name) => name.to_string(),
        None => name_from(url)?,
    };
    let target = dir.join(&name);
    if target.exists() {
        return Err(format!(
            "{} already exists. Machines may be booting it right now, so this will not \
             overwrite it — remove it first, or use --as to fetch under another name.",
            target.display()
        ));
    }
    let partial = dir.join(format!("{name}.part"));

    let (program, args) = downloader(&partial, url).ok_or_else(|| {
        format!(
            "neither curl nor wget is installed, and there is no TLS in this binary — \
             40-odd crates and a megabyte on armv7 for a job the host already does. \
             Install one, or download {url} yourself and drop it in {}.",
            dir.display()
        )
    })?;

    eprintln!("fetching {url}");
    eprintln!("  with {program}, into {}", partial.display());
    let status = Command::new(program)
        .args(&args)
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        // **curl exits 33 when the server ignored the Range header**, which is what a
        // mirror without byte-range support does. Retrying would fail identically
        // forever, so the answer is to say the one thing that fixes it rather than
        // repeating "run it again".
        if code == 33 {
            return Err(format!(
                "{program} exited 33: this server does not support resuming, and {} is a \
                 partial download from an earlier attempt. Delete it and run this again \
                 to start from the beginning.",
                partial.display()
            ));
        }
        // Otherwise leave the partial file: a 1.5 GB download that failed at 90% is
        // worth resuming, and both tools resume onto it.
        return Err(format!(
            "{program} exited {code}. {} is left in place, and running this again resumes \
             it — delete it to start over.",
            partial.display()
        ));
    }

    let bytes = std::fs::metadata(&partial)
        .map_err(|e| format!("{}: {e}", partial.display()))?
        .len();
    if bytes == 0 {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("{url} produced an empty file"));
    }

    if let Some(expected) = expected {
        eprintln!("verifying {} …", human(bytes));
        let digest = crate::boot::sha256::file(&partial, |_, _| {})
            .map_err(|e| format!("{}: {e}", partial.display()))?;
        if !digest.eq_ignore_ascii_case(expected) {
            // **Loud, fatal, and the file is destroyed.** A mismatch is a corrupted
            // transfer, the wrong file, or a mirror that is not what it claims — and
            // leaving it on disk means somebody registers it tomorrow by hand.
            let _ = std::fs::remove_file(&partial);
            return Err(format!(
                "digest mismatch — {} was deleted\n  expected {expected}\n  found    {digest}",
                partial.display()
            ));
        }
    }

    // Only now does it become a file the catalogue can see. Rename within one directory
    // is atomic on POSIX, which is the same guarantee the file store already relies on.
    std::fs::rename(&partial, &target).map_err(|e| {
        format!(
            "cannot move {} to {}: {e}",
            partial.display(),
            target.display()
        )
    })?;

    Ok(Fetched {
        path: target,
        bytes,
        via: program,
    })
}

/// The tool to use, and how to ask it. Both are told to resume, to follow redirects and
/// to fail on an HTTP error rather than writing the error page to disk under a name
/// ending in `.iso`.
fn downloader(into: &Path, url: &str) -> Option<(&'static str, Vec<String>)> {
    let into = into.to_string_lossy().into_owned();
    if on_path("curl") {
        return Some((
            "curl",
            vec![
                // Without --fail a 404 is written out as a file, and the next command
                // registers an HTML page as an installer image.
                "--fail".into(),
                // Mirrors redirect constantly.
                "--location".into(),
                // Resume onto a partial file, which is what makes a failed 1.5 GB
                // download cost the remainder rather than the whole thing.
                "--continue-at".into(),
                "-".into(),
                "--progress-bar".into(),
                "--output".into(),
                into,
                url.into(),
            ],
        ));
    }
    if on_path("wget") {
        return Some((
            "wget",
            vec![
                "--continue".into(),
                "--progress=bar:force".into(),
                "--output-document".into(),
                into,
                url.into(),
            ],
        ));
    }
    None
}

fn on_path(program: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

/// The filename a URL implies, which is what the entry will be called.
///
/// Refused rather than guessed at when the URL does not carry one: an image named after
/// a query string is an identifier nobody can type, and it becomes part of a URL that a
/// machine has to fetch.
pub fn name_from(url: &str) -> Result<String, String> {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let last = without_query
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let decoded = percent_decode(last);
    if decoded.is_empty() || !decoded.contains('.') {
        return Err(format!(
            "cannot tell what to call the image from {url} — pass --as NAME.iso"
        ));
    }
    let stem = decoded.rsplit_once('.').map(|(s, _)| s).unwrap_or(&decoded);
    if !crate::store::valid_id(stem) {
        return Err(format!(
            "{url} implies the name {decoded:?}, whose stem is not a usable identifier — \
             it becomes part of a URL a machine has to fetch. Pass --as NAME.iso."
        ));
    }
    Ok(decoded)
}

/// Just enough to turn `%2B` back into `+`. A vendor's download path occasionally
/// carries one, and a file named `proxmox%2Dve.iso` is nobody's idea of an identifier.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
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
        format!("{bytes}B")
    } else {
        format!("{size:.1}{}", UNITS[unit])
    }
}

/// Whether an argument is a URL rather than a path, which is what decides whether
/// `media add` fetches or registers.
pub fn looks_like_a_url(argument: &str) -> bool {
    argument.starts_with("http://") || argument.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_is_told_from_a_path() {
        assert!(looks_like_a_url(
            "https://enterprise.proxmox.com/iso/pve.iso"
        ));
        assert!(looks_like_a_url("http://mirror/pve.iso"));
        assert!(!looks_like_a_url("/srv/media/pve.iso"));
        assert!(!looks_like_a_url("pve.iso"));
        // Deliberately not FTP or file: neither is a path this fetches, and treating one
        // as a URL would hand it to curl and produce a confusing failure.
        assert!(!looks_like_a_url("ftp://mirror/pve.iso"));
    }

    #[test]
    fn the_name_comes_from_the_last_path_segment() {
        assert_eq!(
            name_from("https://enterprise.proxmox.com/iso/proxmox-ve_8.4-1.iso").expect("named"),
            "proxmox-ve_8.4-1.iso"
        );
        // A query string is not part of the name; plenty of mirrors add one.
        assert_eq!(
            name_from("https://mirror/ubuntu-24.04.iso?mirror=de&x=1").expect("named"),
            "ubuntu-24.04.iso"
        );
        assert_eq!(
            name_from("https://mirror/rocky.iso#sha256").expect("named"),
            "rocky.iso"
        );
    }

    #[test]
    fn a_url_that_implies_no_usable_name_is_refused_rather_than_guessed_at() {
        // The name becomes part of a URL a machine has to fetch, so an unusable one is
        // worth stopping for rather than mangling into something nobody can type.
        for url in [
            "https://mirror/download?id=42",
            "https://mirror/",
            "https://mirror/iso/",
        ] {
            assert!(name_from(url).is_err(), "{url} should be refused");
        }
        let e = name_from("https://mirror/download?id=42").expect_err("refused");
        assert!(e.contains("--as"), "the way out has to be named: {e}");
    }

    #[test]
    fn a_percent_escape_in_the_path_is_decoded() {
        assert_eq!(percent_decode("proxmox%2Dve.iso"), "proxmox-ve.iso");
        assert_eq!(percent_decode("plain.iso"), "plain.iso");
        // A stray `%` is left alone rather than eating the next two characters.
        assert_eq!(percent_decode("100%.iso"), "100%.iso");
    }

    #[test]
    fn an_existing_image_is_never_overwritten() {
        // Machines may be booting it right now, and a half-written image is one they
        // would boot into something that does not work.
        let dir = std::env::temp_dir().join(format!("rescriptum-fetch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("pve.iso"), b"an image somebody is using").expect("write");

        let e = fetch("https://mirror/pve.iso", &dir, None, None).expect_err("must refuse");
        assert!(e.contains("already exists"), "{e}");
        assert!(e.contains("--as"), "{e}");
        // And it is untouched.
        assert_eq!(
            std::fs::read(dir.join("pve.iso")).expect("read"),
            b"an image somebody is using"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_downloader_refuses_to_write_an_error_page_to_disk() {
        // Without `--fail` a 404 is written out, and the next command registers an HTML
        // page as an installer image — which probes as unknown and boots as nothing.
        let Some((program, args)) = downloader(Path::new("/tmp/x.part"), "https://m/x.iso") else {
            // Neither tool installed: nothing to assert, and `fetch` says so itself.
            return;
        };
        match program {
            "curl" => {
                assert!(args.iter().any(|a| a == "--fail"), "{args:?}");
                assert!(args.iter().any(|a| a == "--location"), "{args:?}");
                assert!(args.iter().any(|a| a == "--continue-at"), "{args:?}");
            }
            "wget" => assert!(args.iter().any(|a| a == "--continue"), "{args:?}"),
            other => panic!("unexpected downloader {other}"),
        }
    }
}
