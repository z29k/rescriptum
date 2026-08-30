//! Placing an image: which installer family it is, and where its kernel and initrd sit.
//!
//! A table of markers, read in order, first match wins. It costs a few kilobytes of
//! reads — a volume descriptor and a directory extent or two — never a pass over the
//! file, because ISO9660 directories are extents you can seek to.
//!
//! **Every row was written from documentation and must be pinned against a real image
//! before it is trusted.** The table is a table precisely so that verifying it is cheap.
//! An image no row claims is `Unknown` and still served: not describable is not the
//! same as not usable.

use super::iso::Iso;
use std::collections::BTreeMap;
use std::path::Path;

/// The families whose boot arguments `stanza` knows how to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    Proxmox,
    Debian,
    Ubuntu,
    Rhel,
    Suse,
    CoreOs,
    Unknown,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Proxmox => "proxmox",
            Family::Debian => "debian",
            Family::Ubuntu => "ubuntu",
            Family::Rhel => "rhel",
            Family::Suse => "suse",
            Family::CoreOs => "coreos",
            Family::Unknown => "unknown",
        }
    }

    pub fn parse(text: &str) -> Option<Family> {
        Some(match text {
            "proxmox" => Family::Proxmox,
            "debian" => Family::Debian,
            "ubuntu" => Family::Ubuntu,
            "rhel" => Family::Rhel,
            "suse" => Family::Suse,
            "coreos" => Family::CoreOs,
            "unknown" => Family::Unknown,
            _ => return None,
        })
    }
}

/// What the menu gates entries on. An ARM64 image offered to an x86 client is a menu
/// entry that boots the wrong kernel, so this is part of the probe rather than a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Arch {
    X86_64,
    Arm64,
}

impl Arch {
    pub fn label(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        }
    }

    /// iPXE's `${buildarch}`, which is what a generated menu compares against.
    pub fn buildarch(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        }
    }

    pub fn parse(text: &str) -> Option<Arch> {
        Some(match text {
            "x86_64" | "amd64" => Arch::X86_64,
            "arm64" | "aarch64" => Arch::Arm64,
            _ => return None,
        })
    }
}

/// Everything the probe could establish. Any of it may be absent; none of it is fatal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Probed {
    pub family: Option<Family>,
    pub version: Option<String>,
    pub arch: Option<Arch>,
    /// Where the kernel is **inside the image**.
    pub kernel: Option<String>,
    pub initrd: Option<String>,
    /// The kernel and initrd are files *beside* the image, not inside it — which is
    /// what `prepare-iso --pxe` leaves behind, since it strips `/boot` from the ISO it
    /// emits and writes `vmlinuz` and `initrd.img` next to it.
    pub external: bool,
    /// A stock Proxmox initrd is zstd-compressed. The assistant recompresses it to gzip
    /// when it splits, saying "iPXE does not support a zstd-compressed initrd" — so an
    /// image carrying the original is worth a word from `media check`.
    pub zstd_initrd: bool,
}

/// One row of the table: a marker that claims an image, and what it implies.
struct Row {
    marker: &'static str,
    family: Family,
    kernel: &'static str,
    initrd: &'static str,
    arch: Option<Arch>,
}

/// **Ordered, and the order carries meaning.** CoreOS sits above the RHEL family whose
/// on-disk skeleton it borrows; a plain RHEL row first would claim every CoreOS image.
const TABLE: &[Row] = &[
    Row {
        marker: "/boot/linux26",
        family: Family::Proxmox,
        kernel: "/boot/linux26",
        initrd: "/boot/initrd.img",
        arch: None,
    },
    Row {
        marker: "/casper/vmlinuz",
        family: Family::Ubuntu,
        kernel: "/casper/vmlinuz",
        initrd: "/casper/initrd",
        arch: None,
    },
    Row {
        marker: "/install.amd/vmlinuz",
        family: Family::Debian,
        kernel: "/install.amd/vmlinuz",
        initrd: "/install.amd/initrd.gz",
        arch: Some(Arch::X86_64),
    },
    Row {
        marker: "/install.a64/vmlinuz",
        family: Family::Debian,
        kernel: "/install.a64/vmlinuz",
        initrd: "/install.a64/initrd.gz",
        arch: Some(Arch::Arm64),
    },
    // Before the RHEL row: Fedora CoreOS lays its files out the same way and is told
    // apart by the live rootfs the RHEL installer does not have.
    Row {
        marker: "/images/pxeboot/rootfs.img",
        family: Family::CoreOs,
        kernel: "/images/pxeboot/vmlinuz",
        initrd: "/images/pxeboot/initrd.img",
        arch: None,
    },
    Row {
        marker: "/images/pxeboot/vmlinuz",
        family: Family::Rhel,
        kernel: "/images/pxeboot/vmlinuz",
        initrd: "/images/pxeboot/initrd.img",
        arch: None,
    },
    Row {
        marker: "/boot/x86_64/loader/linux",
        family: Family::Suse,
        kernel: "/boot/x86_64/loader/linux",
        initrd: "/boot/x86_64/loader/initrd",
        arch: Some(Arch::X86_64),
    },
    Row {
        marker: "/boot/aarch64/loader/linux",
        family: Family::Suse,
        kernel: "/boot/aarch64/loader/linux",
        initrd: "/boot/aarch64/loader/initrd",
        arch: Some(Arch::Arm64),
    },
];

/// Read enough of an image to place it.
pub fn probe(path: &Path) -> std::io::Result<Probed> {
    let mut iso = Iso::open(path)?;
    let mut found = Probed::default();

    // `/.disk/info` first, and Proxmox is why. `prepare-iso --pxe` **strips `/boot`**
    // from the ISO it emits — about 100 MiB — so the `/boot/linux26` marker misses
    // exactly the Proxmox image most likely to be dropped into a media directory. This
    // file survives, and identifying an installer by it is upstream's own method:
    // `proxmox-auto-install-assistant inspect-iso` reads this file and checks that
    // PRODUCTLONG starts with "Proxmox".
    let disk_info = iso
        .read("/.disk/info", 8 * 1024)
        .ok()
        .flatten()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());

    if let Some(info) = &disk_info {
        let fields = parse_disk_info(info);
        let product = fields.get("PRODUCTLONG").map(String::as_str).unwrap_or("");
        if product.starts_with("Proxmox") {
            found.family = Some(Family::Proxmox);
            found.version = Some(proxmox_version(&fields, product));
            // ISOs predating the ARCH key are always amd64, as the assistant records.
            found.arch = fields
                .get("ARCH")
                .and_then(|a| Arch::parse(a))
                .or(Some(Arch::X86_64));
        } else if fields.is_empty() {
            // Debian and Ubuntu write a single descriptive line here instead.
            found.version = info.lines().next().map(|l| l.trim().to_string());
        }
    }

    for row in TABLE {
        if !iso.has(row.marker) {
            continue;
        }
        // A family established from `/.disk/info` is not overridden by a marker; it was
        // read from the vendor's own identification, which is the stronger evidence.
        if found.family.is_none() {
            found.family = Some(row.family);
        }
        if found.family == Some(row.family) {
            found.kernel = Some(row.kernel.to_string());
            found.initrd = Some(row.initrd.to_string());
            if found.arch.is_none() {
                found.arch = row.arch;
            }
        }
        break;
    }

    // A Proxmox image the assistant has already split: the family is known from
    // `/.disk/info`, and the kernel and initrd it needs are the files beside it.
    if found.family == Some(Family::Proxmox) && found.kernel.is_none() {
        let beside = path.parent().unwrap_or(Path::new("."));
        if beside.join("vmlinuz").is_file() && beside.join("initrd.img").is_file() {
            found.kernel = Some("vmlinuz".to_string());
            found.initrd = Some("initrd.img".to_string());
            found.external = true;
        }
    }

    if found.version.is_none() {
        found.version = version_from(&mut iso, found.family);
    }
    if found.arch.is_none() {
        found.arch = arch_from_kernel(&mut iso, found.kernel.as_deref());
    }
    // The compression an initrd actually carries, which decides whether a loader that
    // only speaks gzip can use it.
    if let Some(initrd) = &found.initrd
        && !found.external
        && let Ok(Some(head)) = iso.read(initrd, 4)
    {
        found.zstd_initrd = head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]);
    }
    if found.version.is_none() && !iso.volume_id.is_empty() {
        found.version = Some(iso.volume_id.clone());
    }

    Ok(found)
}

/// `/.disk/info` in a Proxmox image is shell-env style: `KEY='value'` a line at a time.
/// Debian and Ubuntu write one descriptive sentence instead, which parses to nothing
/// here — and that emptiness is what tells the two apart.
fn parse_disk_info(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // A key with a space in it is prose, not a field.
        if key.trim().contains(char::is_whitespace) {
            continue;
        }
        out.insert(
            key.trim().to_string(),
            value.trim().trim_matches(['\'', '"']).to_string(),
        );
    }
    out
}

fn proxmox_version(fields: &BTreeMap<String, String>, product: &str) -> String {
    match (fields.get("RELEASE"), fields.get("ISORELEASE")) {
        (Some(release), Some(iso)) => format!("{product} {release}-{iso}"),
        (Some(release), None) => format!("{product} {release}"),
        _ => product.to_string(),
    }
}

/// Where a vendor left a version string, take it. Nobody has to, and the volume
/// identifier is the fallback.
fn version_from(iso: &mut Iso, family: Option<Family>) -> Option<String> {
    match family {
        Some(Family::Rhel) | Some(Family::CoreOs) => {
            let text = iso.read("/.treeinfo", 16 * 1024).ok().flatten()?;
            let text = String::from_utf8_lossy(&text).into_owned();
            let mut name = None;
            let mut version = None;
            for line in text.lines() {
                let line = line.trim();
                if let Some(v) = line.strip_prefix("version") {
                    version = v.trim_start_matches([' ', '=']).trim().to_string().into();
                } else if let Some(v) = line.strip_prefix("name") {
                    name = v.trim_start_matches([' ', '=']).trim().to_string().into();
                }
            }
            match (name, version) {
                (Some(n), _) => Some(n),
                (None, Some(v)) => Some(v),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A kernel says what it is in its first few dozen bytes. Cheaper and more honest than
/// guessing from a filename, and it is the only signal for a family whose layout is not
/// per-architecture.
fn arch_from_kernel(iso: &mut Iso, kernel: Option<&str>) -> Option<Arch> {
    let kernel = kernel?;
    let head = iso.read(kernel, 0x400).ok().flatten()?;
    // x86 bzImage: "HdrS" at 0x202.
    if head.len() > 0x206 && &head[0x202..0x206] == b"HdrS" {
        return Some(Arch::X86_64);
    }
    // arm64 Image: the magic at offset 56.
    if head.len() > 60 && &head[56..60] == b"ARM\x64" {
        return Some(Arch::Arm64);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::iso::build;
    use super::*;

    fn probe_of(name: &str, builder: &build::Builder) -> Probed {
        let dir = std::env::temp_dir().join(format!("rescriptum-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{name}.iso"));
        std::fs::write(&path, builder.build()).expect("write");
        probe(&path).expect("probes")
    }

    /// A kernel the arch sniffer will recognise, so a fixture can be honest about it.
    fn bzimage() -> Vec<u8> {
        let mut k = vec![0u8; 0x400];
        k[0x202..0x206].copy_from_slice(b"HdrS");
        k
    }

    fn arm64_image() -> Vec<u8> {
        let mut k = vec![0u8; 0x400];
        k[56..60].copy_from_slice(b"ARM\x64");
        k
    }

    #[test]
    fn each_family_is_claimed_by_its_own_marker() {
        // The table is the behaviour, so the test walks it the way a real image would.
        let cases: &[(&str, &str, Family)] = &[
            ("pve", "/boot/linux26", Family::Proxmox),
            ("ubuntu", "/casper/vmlinuz", Family::Ubuntu),
            ("debian", "/install.amd/vmlinuz", Family::Debian),
            ("rhel", "/images/pxeboot/vmlinuz", Family::Rhel),
            ("suse", "/boot/x86_64/loader/linux", Family::Suse),
        ];
        for (name, marker, family) in cases {
            let found = probe_of(name, &build::Builder::new().file(marker, &bzimage()));
            assert_eq!(found.family, Some(*family), "{marker}");
            assert_eq!(found.kernel.as_deref(), Some(*marker), "{marker}");
            assert!(found.initrd.is_some(), "{marker}");
        }
    }

    #[test]
    fn coreos_is_told_apart_from_the_rhel_skeleton_it_borrows() {
        // Both lay their files out in /images/pxeboot. Ordering the table wrongly makes
        // every CoreOS image a RHEL one, and the boot arguments are entirely different.
        let coreos = probe_of(
            "coreos",
            &build::Builder::new()
                .file("/images/pxeboot/vmlinuz", &bzimage())
                .file("/images/pxeboot/rootfs.img", b"live root"),
        );
        assert_eq!(coreos.family, Some(Family::CoreOs));

        let rhel = probe_of(
            "rhel-only",
            &build::Builder::new().file("/images/pxeboot/vmlinuz", &bzimage()),
        );
        assert_eq!(rhel.family, Some(Family::Rhel));
    }

    #[test]
    fn a_proxmox_image_the_assistant_trimmed_is_still_recognised() {
        // `prepare-iso --pxe` strips /boot — about 100 MiB — so the marker the plain
        // table would use is gone from exactly the image most likely to be dropped into
        // a media directory. `/.disk/info` survives, and reading it is how
        // `proxmox-auto-install-assistant inspect-iso` identifies an ISO itself.
        let found = probe_of(
            "pve-trimmed",
            &build::Builder::new().file(
                "/.disk/info",
                b"PRODUCTLONG='Proxmox Virtual Environment'\nRELEASE='8.4'\nISORELEASE='1'\nARCH='amd64'\n",
            ),
        );
        assert_eq!(found.family, Some(Family::Proxmox));
        assert_eq!(
            found.version.as_deref(),
            Some("Proxmox Virtual Environment 8.4-1")
        );
        assert_eq!(found.arch, Some(Arch::X86_64));
        // Nothing beside it, so it declares no kernel rather than inventing one.
        assert_eq!(found.kernel, None);
        assert!(!found.external);
    }

    #[test]
    fn a_trimmed_image_finds_the_kernel_the_assistant_left_beside_it() {
        // `--pxe` writes vmlinuz and initrd.img into the same output directory. Finding
        // them there is what makes an assistant-prepared directory work as-is.
        let dir = std::env::temp_dir().join(format!("rescriptum-pxe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let image = dir.join("pve.iso");
        std::fs::write(
            &image,
            build::Builder::new()
                .file("/.disk/info", b"PRODUCTLONG='Proxmox VE'\nRELEASE='8.4'\n")
                .build(),
        )
        .expect("write");
        std::fs::write(dir.join("vmlinuz"), bzimage()).expect("write");
        std::fs::write(dir.join("initrd.img"), b"\x1f\x8b gzip").expect("write");

        let found = probe(&image).expect("probes");
        assert_eq!(found.family, Some(Family::Proxmox));
        assert!(found.external, "the kernel is beside the image, not inside");
        assert_eq!(found.kernel.as_deref(), Some("vmlinuz"));
        assert_eq!(found.initrd.as_deref(), Some("initrd.img"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stock_proxmox_initrd_is_noticed_to_be_zstd() {
        // The assistant recompresses it to gzip when it splits, on the grounds that
        // "iPXE does not support a zstd-compressed initrd". Whether that binds through
        // our chain is a bench question; noticing it is not.
        let found = probe_of(
            "pve-zstd",
            &build::Builder::new()
                .file("/boot/linux26", &bzimage())
                .file("/boot/initrd.img", &[0x28, 0xB5, 0x2F, 0xFD, 0, 0, 0, 0]),
        );
        assert_eq!(found.family, Some(Family::Proxmox));
        assert!(found.zstd_initrd);

        let gzipped = probe_of(
            "pve-gzip",
            &build::Builder::new()
                .file("/boot/linux26", &bzimage())
                .file("/boot/initrd.img", &[0x1f, 0x8b, 0x08, 0x00]),
        );
        assert!(!gzipped.zstd_initrd);
    }

    #[test]
    fn architecture_comes_from_the_path_where_the_layout_says_it() {
        let debian = probe_of(
            "debian-arm",
            &build::Builder::new().file("/install.a64/vmlinuz", b"not a recognisable kernel"),
        );
        assert_eq!(debian.arch, Some(Arch::Arm64));

        let suse = probe_of(
            "suse-arm",
            &build::Builder::new().file("/boot/aarch64/loader/linux", b"opaque"),
        );
        assert_eq!(suse.arch, Some(Arch::Arm64));
    }

    #[test]
    fn architecture_falls_back_to_the_kernel_image_itself() {
        // Ubuntu's layout is the same on both architectures, so the only honest source
        // is the kernel. An ARM64 image offered to an x86 client is a menu entry that
        // boots the wrong thing.
        let x86 = probe_of(
            "ubuntu-x86",
            &build::Builder::new().file("/casper/vmlinuz", &bzimage()),
        );
        assert_eq!(x86.arch, Some(Arch::X86_64));

        let arm = probe_of(
            "ubuntu-arm",
            &build::Builder::new().file("/casper/vmlinuz", &arm64_image()),
        );
        assert_eq!(arm.arch, Some(Arch::Arm64));
    }

    #[test]
    fn an_image_nothing_claims_is_unknown_and_still_described() {
        // Not describable is not the same as not usable: it is still served, and the
        // volume identifier is the name it gets.
        let found = probe_of(
            "mystery",
            &build::Builder::new()
                .volume("SOME LIVE CD")
                .file("/readme.txt", b"hello"),
        );
        assert_eq!(found.family, None);
        assert_eq!(found.version.as_deref(), Some("SOME LIVE CD"));
        assert_eq!(found.kernel, None);
    }

    #[test]
    fn a_debian_style_disk_info_is_not_mistaken_for_proxmox_fields() {
        // Debian and Ubuntu write one sentence where Proxmox writes KEY='value'. The
        // parser must not turn that sentence into a field.
        let fields =
            parse_disk_info("Ubuntu-Server 24.04.1 LTS \"Noble Numbat\" - Release amd64\n");
        assert!(fields.is_empty(), "{fields:?}");

        let found = probe_of(
            "ubuntu-info",
            &build::Builder::new()
                .file(
                    "/.disk/info",
                    b"Ubuntu-Server 24.04.1 LTS - Release amd64\n",
                )
                .file("/casper/vmlinuz", &bzimage()),
        );
        assert_eq!(found.family, Some(Family::Ubuntu));
        assert_eq!(
            found.version.as_deref(),
            Some("Ubuntu-Server 24.04.1 LTS - Release amd64")
        );
    }
}
