//! Where installer images can be *fetched from* — as opposed to `catalog`, which is what
//! is already held.
//!
//! ## Why this is a list of indexes rather than a list of ISOs
//!
//! The obvious shape for "offer the usual images" is a table of URLs with their digests
//! baked in. It is also wrong, and would be wrong the day it shipped: Proxmox prunes old
//! ISOs from its CDN, Debian and Ubuntu publish point releases every few weeks, and a
//! release cut in August would still be offering June's images — some of them 404. Worse,
//! this project *requires* a digest for a URL (`media add`), because that decision is what
//! every machine ends up installing. A baked-in table would need re-cutting on somebody
//! else's schedule, forever.
//!
//! So nothing about a specific image is stored here. Each entry names the **checksum index
//! the vendor already publishes beside its own images**, and both the filenames and their
//! digests are read from it at the moment somebody asks. The list is current because it is
//! the vendor's, and the digest requirement is satisfied by the vendor's own file — which
//! is exactly what the documentation already tells people to do by hand.
//!
//! ## What that is worth, honestly
//!
//! Taking the digest from the same host that serves the image is **not** the same as
//! verifying a signature you already trusted. Over HTTPS it authenticates the vendor's
//! domain and it catches a truncated download, a corrupted mirror and a file that quietly
//! changed underneath — which is most of what goes wrong. It is *not* protection against a
//! vendor's CDN being compromised, and this module does not pretend otherwise. Somebody
//! who wants more pastes a digest they obtained out of band into `media add URL --sha256`,
//! which stays the stronger path and is never removed.
//!
//! ## The two formats, both measured
//!
//! Every index checked is one of two shapes, so the parser handles exactly two and refuses
//! to guess at a third:
//!
//! - **coreutils** — `<digest>  <name>`, with an optional `*` marking binary mode.
//!   Proxmox, Debian and Ubuntu.
//! - **BSD tag** — `SHA256 (<name>) = <digest>`. AlmaLinux and Rocky, inside a
//!   PGP-clearsigned document whose surrounding lines simply do not match and are skipped.

use super::sha256;

/// One vendor's published index.
///
/// **There is no separate base URL**, deliberately: it is the index's own directory. A
/// second field could drift from the first, and the whole point of reading the vendor's
/// index is that the two cannot disagree.
pub struct Source {
    /// What `media add --from` takes. Becomes part of no URL, but reads like an id.
    pub id: &'static str,
    pub label: &'static str,
    /// The checksum index, beside the images it describes.
    pub index: &'static str,
    /// Kept when the index lists more than this project should offer — Proxmox's one file
    /// covers Backup Server and Mail Gateway too, and offering those under "Proxmox VE"
    /// would be a lie. Empty means no filtering.
    pub keep: &'static str,
    /// A word for what these images install, so a list is readable without knowing the
    /// project. Not the `Family` enum: this is documentation, and `probe` decides the
    /// real family once an image is on disk.
    pub about: &'static str,
}

/// The indexes shipped by default.
///
/// **Every URL here was fetched before being written down**, and so was one image URL
/// derived from each — a table of plausible-looking 404s would be worse than no table.
/// Adding one is a two-line change; an operator who needs a local mirror uses
/// `media add URL --sha256` and needs nothing from this list.
pub const SOURCES: &[Source] = &[
    Source {
        id: "proxmox-ve",
        label: "Proxmox VE",
        index: "https://enterprise.proxmox.com/iso/SHA256SUMS",
        // The same index lists proxmox-backup-server and proxmox-mail-gateway.
        keep: "proxmox-ve_",
        about: "the founding case — answers come from a file injected into the image",
    },
    Source {
        id: "debian",
        label: "Debian",
        index: "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/SHA256SUMS",
        keep: "",
        about: "netinst images; the answer is a preseed on the kernel command line",
    },
    Source {
        id: "ubuntu",
        label: "Ubuntu LTS",
        index: "https://releases.ubuntu.com/noble/SHA256SUMS",
        // The index lists a .wsl alongside the images; `is_image` drops it.
        keep: "",
        about: "autoinstall, via a cloud-init datasource on the kernel command line",
    },
    Source {
        id: "almalinux",
        label: "AlmaLinux 9",
        index: "https://repo.almalinux.org/almalinux/9/isos/x86_64/CHECKSUM",
        keep: "",
        about: "kickstart, named on the kernel command line",
    },
    Source {
        id: "rocky",
        label: "Rocky Linux 9",
        index: "https://download.rockylinux.org/pub/rocky/9/isos/x86_64/CHECKSUM",
        keep: "",
        about: "kickstart, named on the kernel command line",
    },
];

/// Look one up by id.
pub fn source(id: &str) -> Option<&'static Source> {
    SOURCES.iter().find(|s| s.id == id)
}

/// One image a source offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub name: String,
    pub digest: String,
    pub url: String,
}

impl Source {
    /// The directory the index lives in, which is where its images live too.
    pub fn base(&self) -> &str {
        match self.index.rfind('/') {
            Some(at) => &self.index[..=at],
            None => self.index,
        }
    }

    /// Everything this index offers that is an installer image, newest first.
    pub fn offers(&self, index_text: &str) -> Vec<Available> {
        let mut out: Vec<Available> = parse_index(index_text)
            .into_iter()
            .filter(|(name, _)| is_image(name) && name.starts_with(self.keep))
            .map(|(name, digest)| Available {
                url: format!("{}{name}", self.base()),
                name,
                digest,
            })
            .collect();
        // **Newest first, because the first row is the one that gets clicked.** Sorted by
        // the version-ish parts of the name rather than the whole string: plain
        // lexicographic ordering puts `9.10` before `9.9`, which would offer a rack an
        // older installer than the one it asked for.
        out.sort_by(|a, b| natural(&b.name).cmp(&natural(&a.name)));
        out
    }
}

/// Whether a name from an index is an image this server could ever serve.
///
/// Ubuntu's index lists a `.wsl` beside its ISOs; Debian's lists `.jigdo` in some
/// directories. Neither is bootable here, and offering one is a click that ends in a
/// puzzle.
fn is_image(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".iso") || lower.ends_with(".img")
}

/// Split a name into text and number runs, so `9.9` sorts before `9.10`.
fn natural(name: &str) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut chars = name.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut n: u64 = 0;
            while let Some(&d) = chars.peek() {
                if !d.is_ascii_digit() {
                    break;
                }
                // Saturating rather than wrapping: a 30-digit run in a filename is not a
                // version, and it must not silently become a small number.
                n = n.saturating_mul(10).saturating_add(d as u64 - '0' as u64);
                chars.next();
            }
            out.push(Chunk::Number(n));
        } else {
            let mut s = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    break;
                }
                s.push(d.to_ascii_lowercase());
                chars.next();
            }
            out.push(Chunk::Text(s));
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Chunk {
    // Text before Number, so `debian-13` and `debian-9` compare on the number.
    Text(String),
    Number(u64),
}

/// Read a checksum index in either of the two formats vendors actually publish.
///
/// Anything that is not a digest line is skipped rather than refused — an AlmaLinux
/// `CHECKSUM` is a PGP-clearsigned document with a header, a comment per file and a
/// signature block, and none of that is an error.
pub fn parse_index(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(pair) = bsd_tag(line).or_else(|| coreutils(line)) {
            out.push(pair);
        }
    }
    out
}

/// `SHA256 (name) = digest` — AlmaLinux, Rocky.
fn bsd_tag(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("SHA256")?.trim_start();
    let rest = rest.strip_prefix('(')?;
    let (name, rest) = rest.split_once(')')?;
    let digest = rest.trim_start().strip_prefix('=')?.trim();
    sha256::is_digest(digest).then(|| (name.trim().to_string(), digest.to_ascii_lowercase()))
}

/// `digest  name`, with `*` marking binary mode — Proxmox, Debian, Ubuntu.
fn coreutils(line: &str) -> Option<(String, String)> {
    let (digest, name) = line.split_once(char::is_whitespace)?;
    if !sha256::is_digest(digest) {
        return None;
    }
    let name = name.trim_start();
    let name = name.strip_prefix('*').unwrap_or(name);
    let name = name.trim();
    (!name.is_empty()).then(|| (name.to_string(), digest.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from the real indexes, on 2026-08-28, rather than invented — the point of
    // a fixture here is that it is the shape the vendor actually publishes.
    const PROXMOX: &str = "\
d237d70ca48a9f6eb47f95fd4fd337722c3f69f8106393844d027d28c26523d8  proxmox-ve_8.4-1.iso
6d8f5afc78c0c66812d7272cde7c8b98be7eb54401ceb045400db05eb5ae6d22  proxmox-ve_9.1-1.iso
4e88fe416df9b527624a175f24c9aa07c714d3332afb1ee3dbf3879573ef2c6c  proxmox-ve_9.2-1.iso
721e21a88ae93dba73ca3e4a494b438190acadb99993ec755a19e721a86f0395  proxmox-backup-server_2.4-1.iso
";

    const UBUNTU: &str = "\
faabcf33ae53976d2b8207a001ff32f4e5daae013505ac7188c9ea63988f8328 *ubuntu-24.04.3-live-server-amd64.iso
c74833a55e525b1e99e1541509c566bb3e32bdb53bf27ea3347174364a57f47c *ubuntu-24.04.3-wsl-amd64.wsl
e907d92eeec9df64163a7e454cbc8d7755e8ddc7ed42f99dbc80c40f1a138433 *ubuntu-24.04.4-live-server-amd64.iso
";

    const ALMA: &str = "\
-----BEGIN PGP SIGNED MESSAGE-----
Hash: SHA256

# AlmaLinux-9.8-x86_64-boot.iso: 1519271936 bytes
SHA256 (AlmaLinux-9.8-x86_64-boot.iso) = 445f99e24399bbe98aab86111d60751c142eda049d2444fd76da5eb03472e4ab
SHA256 (AlmaLinux-9.8-x86_64-dvd.iso) = 7a392bdc879afd159b30da39a356b7b26c1ddf618b01549164da9aadbc40d814
-----BEGIN PGP SIGNATURE-----
iQIzBAEBCAAdFiEE
-----END PGP SIGNATURE-----
";

    #[test]
    fn the_coreutils_format_parses_with_and_without_the_binary_marker() {
        let got = parse_index(PROXMOX);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].0, "proxmox-ve_8.4-1.iso");
        assert_eq!(
            got[0].1,
            "d237d70ca48a9f6eb47f95fd4fd337722c3f69f8106393844d027d28c26523d8"
        );

        // Ubuntu marks binary mode with `*`, which is part of the format and not part of
        // the filename. Left in, every fetch would ask for a file that does not exist.
        let got = parse_index(UBUNTU);
        assert_eq!(got[0].0, "ubuntu-24.04.3-live-server-amd64.iso");
    }

    #[test]
    fn a_pgp_signed_bsd_tag_index_parses_and_its_wrapper_is_not_an_error() {
        let got = parse_index(ALMA);
        assert_eq!(
            got.len(),
            2,
            "the signature and header are skipped, not refused"
        );
        assert_eq!(got[0].0, "AlmaLinux-9.8-x86_64-boot.iso");
        assert_eq!(
            got[0].1,
            "445f99e24399bbe98aab86111d60751c142eda049d2444fd76da5eb03472e4ab"
        );
    }

    #[test]
    fn nothing_that_is_not_a_digest_line_is_taken_for_one() {
        // Each of these has the *shape* of an entry and is not one. A parser that accepted
        // any of them would offer a fetch that cannot resolve.
        let text = "\
not-a-digest  something.iso
SHA1 (old.iso) = da39a3ee5e6b4b0d3255bfef95601890afd80709
SHA256 (truncated.iso) = abc123
# 445f99e24399bbe98aab86111d60751c142eda049d2444fd76da5eb03472e4ab  commented.iso
445f99e24399bbe98aab86111d60751c142eda049d2444fd76da5eb03472e4ab
";
        assert!(parse_index(text).is_empty(), "{:?}", parse_index(text));
    }

    #[test]
    fn an_index_offers_only_images_and_only_what_the_source_claims() {
        let proxmox = source("proxmox-ve").expect("shipped");
        let offers = proxmox.offers(PROXMOX);
        // The same index carries Backup Server, and offering it under "Proxmox VE" would
        // be a lie the catalogue told.
        assert!(
            offers.iter().all(|o| o.name.starts_with("proxmox-ve_")),
            "{offers:?}"
        );
        assert_eq!(offers.len(), 3);

        // Ubuntu's `.wsl` is in the index and is not something this server can serve.
        let ubuntu = source("ubuntu").expect("shipped");
        let offers = ubuntu.offers(UBUNTU);
        assert!(
            offers.iter().all(|o| o.name.ends_with(".iso")),
            "{offers:?}"
        );
        assert_eq!(offers.len(), 2);
    }

    #[test]
    fn the_newest_is_offered_first_and_ten_beats_nine() {
        let proxmox = source("proxmox-ve").expect("shipped");
        assert_eq!(proxmox.offers(PROXMOX)[0].name, "proxmox-ve_9.2-1.iso");

        // **The reason sorting is not lexicographic.** The first row is the one that gets
        // clicked, and plain string order puts 9.10 behind 9.9 — offering a rack an older
        // installer than the one it asked for.
        let text = "\
1111111111111111111111111111111111111111111111111111111111111111  x_9.9-1.iso
2222222222222222222222222222222222222222222222222222222222222222  x_9.10-1.iso
";
        let s = Source {
            id: "x",
            label: "x",
            index: "https://example.invalid/d/SHA256SUMS",
            keep: "",
            about: "",
        };
        assert_eq!(s.offers(text)[0].name, "x_9.10-1.iso");
    }

    #[test]
    fn an_images_url_is_the_indexs_own_directory() {
        // Not a second field, so the two cannot drift apart — which is the whole reason
        // for reading the vendor's index in the first place.
        let proxmox = source("proxmox-ve").expect("shipped");
        assert_eq!(proxmox.base(), "https://enterprise.proxmox.com/iso/");
        assert_eq!(
            proxmox.offers(PROXMOX)[0].url,
            "https://enterprise.proxmox.com/iso/proxmox-ve_9.2-1.iso"
        );
    }

    #[test]
    fn every_shipped_source_is_usable_as_written() {
        // Cheap, and it catches the paste error that a network test would blame on the
        // network. The URLs themselves were fetched by hand before being written down;
        // this asserts the shape they have to keep.
        for s in SOURCES {
            assert!(!s.id.is_empty() && !s.label.is_empty(), "{}", s.id);
            assert!(
                s.index.starts_with("https://"),
                "{} must be https — the digest is the point",
                s.id
            );
            assert!(
                s.base().ends_with('/') && s.base().len() < s.index.len(),
                "{} has no directory to hang images off",
                s.id
            );
            assert!(source(s.id).is_some(), "{} is not findable", s.id);
        }
        // Ids are what `media add --from` takes, so a duplicate would silently shadow.
        let mut ids: Vec<&str> = SOURCES.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "duplicate source id");
    }
}
