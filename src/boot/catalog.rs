//! The media catalogue: what images this server holds, discovered rather than declared.
//!
//! The same machinery the answers directory already uses — read the directory, cache
//! the listing, invalidate on mtime with a backstop behind it. Drop an ISO in and it
//! appears; no restart, no registration, no database. That instinct is the one the
//! answer set is built on and there is no reason for media to differ.
//!
//! **What `media add` writes is a sidecar, and nothing else.** The image is never
//! modified, never moved, never copied. A `.media` file beside it records what was
//! learned at ingest — the digest above all, since hashing 1.5 GB is a minute the
//! server must never spend inside a request. An image with no sidecar is still listed
//! and still served; it just has no digest to re-check and was probed on sight.

use super::probe::{self, Arch, Family, Probed};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Editing a file's *contents* moves no directory mtime, and a filesystem may round the
/// timestamp it does move. Same reasoning, same value as the answer listing's.
const RELOAD_BACKSTOP: Duration = Duration::from_secs(1);

/// What counts as an image. Deliberately short: a media directory is also where an
/// operator's notes, checksums and licence files end up, and none of those is bootable.
pub const IMAGE_EXTENSIONS: &[&str] = &["iso", "img"];

/// The sidecar `media add` writes.
pub const SIDECAR_EXTENSION: &str = "media";

/// Paths the listener answers itself. `valid_id` accepts dots, so `netboot.xyz` is a
/// *valid* identifier — which would let an entry shadow a fixed root. They are refused
/// at `media add` rather than resolved at request time, because a shadowed route fails
/// as a mysterious 404 rather than as an error anybody can act on.
pub const RESERVED_IDS: &[&str] = &["boot", "ipxe", "netboot.xyz", "health", "media"];

/// One image, as the listener and the menu need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    /// The image itself. **Always taken from here, never built from a request.**
    pub path: PathBuf,
    pub size: u64,
    /// Recorded at ingest by `media add`. `None` means nobody has pinned this image.
    pub digest: Option<String>,
    pub probed: Probed,
    /// Where the kernel and initrd are, when they sit beside the image rather than
    /// inside it — `prepare-iso --pxe` output, which is a directory of three files.
    pub beside: Option<PathBuf>,
}

impl Entry {
    pub fn family(&self) -> Family {
        self.probed.family.unwrap_or(Family::Unknown)
    }

    pub fn arch(&self) -> Option<Arch> {
        self.probed.arch
    }

    /// A short human label for a listing: the version if a vendor left one, else the id.
    pub fn describe(&self) -> String {
        self.probed
            .version
            .clone()
            .unwrap_or_else(|| self.id.clone())
    }

    /// Whether this entry can offer a kernel and an initrd at all. An image that cannot
    /// is still served whole — `sanboot` and virtual media both take one.
    pub fn bootable(&self) -> bool {
        self.probed.kernel.is_some() && self.probed.initrd.is_some()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// Everything wrong that is not worth refusing to serve over. A fleet must never be
    /// unable to install because one image is odd.
    pub problems: Vec<String>,
}

impl Listing {
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

struct Cached {
    version: Option<String>,
    loaded_at: Instant,
    listing: Arc<Listing>,
}

pub struct Catalog {
    dir: PathBuf,
    cache: Mutex<Option<Cached>>,
}

impl Catalog {
    pub fn new(dir: impl Into<PathBuf>) -> Catalog {
        Catalog {
            dir: dir.into(),
            cache: Mutex::new(None),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn describe(&self) -> String {
        format!("media in {}", self.dir.display())
    }

    /// The catalogue as it currently stands, from cache when nothing has moved.
    pub fn listing(&self) -> io::Result<Arc<Listing>> {
        let version = self.version();
        // A poisoned lock means another request panicked mid-refresh. The data is still
        // structurally sound, so carry on rather than failing an install over it.
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(cached) = guard.as_ref()
            && cached.version == version
            && version.is_some()
            && cached.loaded_at.elapsed() < RELOAD_BACKSTOP
        {
            return Ok(Arc::clone(&cached.listing));
        }

        let listing = Arc::new(self.build());
        *guard = Some(Cached {
            version,
            loaded_at: Instant::now(),
            listing: Arc::clone(&listing),
        });
        Ok(listing)
    }

    pub fn get(&self, id: &str) -> io::Result<Option<Entry>> {
        Ok(self.listing()?.get(id).cloned())
    }

    pub fn problems(&self) -> io::Result<Vec<String>> {
        Ok(self.listing()?.problems.clone())
    }

    /// One `stat`, standing in for the whole walk. A new file moves the directory's
    /// mtime; a rewritten one does not, which is what the backstop above is for.
    fn version(&self) -> Option<String> {
        let meta = std::fs::metadata(&self.dir).ok()?;
        let mtime = meta.modified().ok()?;
        let since = mtime.duration_since(SystemTime::UNIX_EPOCH).ok()?;
        Some(format!("{}.{}", since.as_secs(), since.subsec_nanos()))
    }

    fn build(&self) -> Listing {
        let mut listing = Listing::default();

        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) => {
                // Not fatal, on purpose: the directory may appear, or have its
                // permissions fixed, and this is re-read as it changes.
                listing.problems.push(format!(
                    "{} cannot be listed: {e} — no images will be served until that is fixed",
                    self.dir.display()
                ));
                return listing;
            }
        };

        let mut images: BTreeMap<String, PathBuf> = BTreeMap::new();
        let mut sidecars: BTreeMap<String, PathBuf> = BTreeMap::new();

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // A hidden file is never an image, and one kind is actively dangerous: a Mac
            // editing this share over SMB drops AppleDouble `._<name>` files beside every
            // real one. The answers listing already skips them for the same reason.
            if name.starts_with('.') {
                continue;
            }
            // `file_type` comes back free with the readdir on Unix; only a symlink needs
            // the extra stat to resolve.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let is_file = if kind.is_file() {
                true
            } else if kind.is_symlink() {
                entry.path().is_file()
            } else {
                false
            };
            if !is_file {
                continue;
            }

            let path = entry.path();
            let Some(extension) = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
            else {
                continue;
            };
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();

            if extension == SIDECAR_EXTENSION {
                sidecars.insert(stem, path);
            } else if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
                images.insert(stem, path);
            }
        }

        for (id, path) in images {
            match self.entry(&id, &path, sidecars.get(&id).map(PathBuf::as_path)) {
                Ok(entry) => listing.entries.push(entry),
                Err(problem) => listing.problems.push(problem),
            }
        }

        // A sidecar whose image is gone is a leftover, and a silent one: the entry
        // simply stops existing and the menu shrinks with no explanation.
        for (id, path) in &sidecars {
            if !listing.entries.iter().any(|e| &e.id == id) {
                listing.problems.push(format!(
                    "{}: no image named {id} — the sidecar describes something that is not here",
                    path.display()
                ));
            }
        }

        listing.entries.sort_by(|a, b| a.id.cmp(&b.id));
        listing
    }

    fn entry(&self, id: &str, path: &Path, sidecar: Option<&Path>) -> Result<Entry, String> {
        if !crate::store::valid_id(id) {
            return Err(format!(
                "{}: {id:?} is not a usable identifier — it becomes part of a URL",
                path.display()
            ));
        }
        if RESERVED_IDS.contains(&id) {
            return Err(format!(
                "{}: {id:?} is a reserved name — the listener answers that path itself",
                path.display()
            ));
        }

        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let recorded = sidecar.map(Sidecar::load).transpose()?.unwrap_or_default();

        // A sidecar that already carries the probe's answers saves opening the image.
        // Without one, probe now: it is a few kilobytes of reads, not a pass over the
        // file, and the listing is cached behind an mtime.
        let probed = match recorded.probed() {
            Some(probed) => probed,
            None => probe::probe(path).unwrap_or_else(|e| {
                // An image that will not parse is still an image somebody can `sanboot`
                // or write to a stick. Serve it; describe it as unknown.
                crate::log::server(&format!("warning: cannot probe {}: {e}", path.display()));
                Probed::default()
            }),
        };

        let beside = probed
            .external
            .then(|| path.parent().unwrap_or(Path::new(".")).to_path_buf());

        Ok(Entry {
            id: id.to_string(),
            path: path.to_path_buf(),
            size,
            digest: recorded.digest,
            probed,
            beside,
        })
    }
}

/// What `media add` recorded about an image, so the server never re-learns it.
///
/// Plain `key = value` lines, because this is a note beside a file rather than a
/// document anybody composes: no merging, no layering, no format negotiation. A key
/// nothing understands is ignored rather than refused — a newer rescriptum writing one
/// must not stop an older one from serving the image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sidecar {
    pub digest: Option<String>,
    pub family: Option<String>,
    pub version: Option<String>,
    pub arch: Option<String>,
    pub kernel: Option<String>,
    pub initrd: Option<String>,
    pub external: bool,
    pub zstd_initrd: bool,
}

impl Sidecar {
    pub fn load(path: &Path) -> Result<Sidecar, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("{}: cannot be read: {e}", path.display()))?;
        Ok(Sidecar::parse(&text))
    }

    pub fn parse(text: &str) -> Sidecar {
        let mut out = Sidecar::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().to_string();
            if value.is_empty() {
                continue;
            }
            match key.trim() {
                "sha256" => out.digest = Some(value),
                "family" => out.family = Some(value),
                "version" => out.version = Some(value),
                "arch" => out.arch = Some(value),
                "kernel" => out.kernel = Some(value),
                "initrd" => out.initrd = Some(value),
                "external" => out.external = value == "true",
                "zstd-initrd" => out.zstd_initrd = value == "true",
                _ => {}
            }
        }
        out
    }

    /// The probe's answers, when the sidecar carries enough of them to skip the image.
    /// A sidecar with only a digest does not: the family is what the menu needs.
    fn probed(&self) -> Option<Probed> {
        let family = self.family.as_deref().and_then(Family::parse)?;
        Some(Probed {
            family: Some(family),
            version: self.version.clone(),
            arch: self.arch.as_deref().and_then(Arch::parse),
            kernel: self.kernel.clone(),
            initrd: self.initrd.clone(),
            external: self.external,
            zstd_initrd: self.zstd_initrd,
        })
    }

    pub fn render(digest: &str, probed: &Probed) -> String {
        let mut out = String::from(
            "# rescriptum media entry — written by `media add`.\n\
             # Delete it and the image is probed again on sight; nothing is lost but the\n\
             # recorded digest, which is the one thing this server will not re-compute\n\
             # inside a request.\n",
        );
        out.push_str(&format!("sha256 = {digest}\n"));
        if let Some(family) = probed.family {
            out.push_str(&format!("family = {}\n", family.label()));
        }
        if let Some(version) = &probed.version {
            out.push_str(&format!("version = {version}\n"));
        }
        if let Some(arch) = probed.arch {
            out.push_str(&format!("arch = {}\n", arch.label()));
        }
        if let Some(kernel) = &probed.kernel {
            out.push_str(&format!("kernel = {kernel}\n"));
        }
        if let Some(initrd) = &probed.initrd {
            out.push_str(&format!("initrd = {initrd}\n"));
        }
        if probed.external {
            out.push_str("external = true\n");
        }
        if probed.zstd_initrd {
            out.push_str("zstd-initrd = true\n");
        }
        out
    }

    /// Where the sidecar for an image lives.
    pub fn path_for(image: &Path) -> PathBuf {
        image.with_extension(SIDECAR_EXTENSION)
    }
}

#[cfg(test)]
mod tests {
    use super::super::iso::build;
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let path = std::env::temp_dir()
                .join(format!("rescriptum-catalog-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Dir(path)
        }

        fn image(&self, name: &str, builder: &build::Builder) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, builder.build()).expect("write");
            path
        }

        fn write(&self, name: &str, body: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, body).expect("write");
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn bzimage() -> Vec<u8> {
        let mut k = vec![0u8; 0x400];
        k[0x202..0x206].copy_from_slice(b"HdrS");
        k
    }

    fn pve() -> build::Builder {
        build::Builder::new()
            .volume("PVE")
            .file("/boot/linux26", &bzimage())
            .file("/boot/initrd.img", &[0x1f, 0x8b, 0, 0])
    }

    #[test]
    fn an_image_dropped_in_the_directory_is_in_the_catalogue() {
        // Discovered, not declared: the whole point. No registration step, no restart.
        let dir = Dir::new("discovery");
        dir.image("pve-8.4.iso", &pve());

        let catalog = Catalog::new(&dir.0);
        let listing = catalog.listing().expect("lists");
        assert_eq!(listing.entries.len(), 1);
        let entry = &listing.entries[0];
        assert_eq!(entry.id, "pve-8.4");
        assert_eq!(entry.family(), Family::Proxmox);
        assert!(entry.bootable());
        assert_eq!(entry.digest, None, "nobody has pinned it");
    }

    #[test]
    fn a_new_image_appears_without_a_restart() {
        // The cache must not be a way to miss an image. One `Catalog`, as the cache
        // invalidation tests for answers already insist: a fresh one per call would
        // bypass the cache entirely and prove nothing.
        let dir = Dir::new("appears");
        let catalog = Catalog::new(&dir.0);
        assert_eq!(catalog.listing().expect("lists").entries.len(), 0);

        dir.image("pve-8.4.iso", &pve());
        std::thread::sleep(RELOAD_BACKSTOP + Duration::from_millis(50));
        assert_eq!(catalog.listing().expect("lists").entries.len(), 1);
    }

    #[test]
    fn hidden_and_appledouble_entries_are_skipped() {
        // A Mac editing this share over SMB drops `._pve-8.4.iso` beside the real file.
        // On the answers side that hijacked a machine's answer; here it would be a
        // second, broken catalogue entry for every image.
        let dir = Dir::new("appledouble");
        dir.image("pve-8.4.iso", &pve());
        dir.write("._pve-8.4.iso", b"AppleDouble junk");
        dir.write(".hidden.iso", b"junk");

        let listing = Catalog::new(&dir.0).listing().expect("lists");
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].id, "pve-8.4");
    }

    #[test]
    fn a_file_that_is_not_an_image_is_not_an_entry() {
        // A media directory is also where notes and checksum files end up.
        let dir = Dir::new("clutter");
        dir.image("pve-8.4.iso", &pve());
        dir.write("SHA256SUMS", b"9f86d0 pve-8.4.iso\n");
        dir.write("notes.txt", b"remember to update this");
        dir.write("vmlinuz", &bzimage());

        let listing = Catalog::new(&dir.0).listing().expect("lists");
        assert_eq!(listing.entries.len(), 1, "{:?}", listing.entries);
    }

    #[test]
    fn a_reserved_name_is_refused_rather_than_shadowing_a_route() {
        // `valid_id` accepts dots, so `netboot.xyz.iso` produces a *valid* identifier
        // that would shadow the listener's own root — and a shadowed route fails as a
        // mysterious 404 rather than as anything anybody can act on.
        let dir = Dir::new("reserved");
        dir.image("netboot.xyz.iso", &pve());
        dir.image("boot.iso", &pve());

        let listing = Catalog::new(&dir.0).listing().expect("lists");
        assert!(listing.entries.is_empty(), "{:?}", listing.entries);
        assert_eq!(listing.problems.len(), 2);
        assert!(
            listing.problems.iter().all(|p| p.contains("reserved")),
            "{:?}",
            listing.problems
        );
    }

    #[test]
    fn a_sidecar_supplies_the_digest_and_spares_the_probe() {
        let dir = Dir::new("sidecar");
        dir.image("pve-8.4.iso", &pve());
        dir.write(
            "pve-8.4.media",
            b"sha256 = 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\n\
              family = proxmox\nversion = Proxmox VE 8.4-1\narch = x86_64\n\
              kernel = /boot/linux26\ninitrd = /boot/initrd.img\n",
        );

        let listing = Catalog::new(&dir.0).listing().expect("lists");
        let entry = listing.get("pve-8.4").expect("present");
        assert_eq!(
            entry.digest.as_deref(),
            Some("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08")
        );
        assert_eq!(entry.describe(), "Proxmox VE 8.4-1");
        assert_eq!(entry.family(), Family::Proxmox);
    }

    #[test]
    fn a_sidecar_whose_image_is_gone_is_reported_rather_than_ignored() {
        // Otherwise the entry simply stops existing and the menu shrinks silently.
        let dir = Dir::new("orphan");
        dir.write("gone.media", b"sha256 = deadbeef\nfamily = proxmox\n");

        let listing = Catalog::new(&dir.0).listing().expect("lists");
        assert!(listing.entries.is_empty());
        assert_eq!(listing.problems.len(), 1);
        assert!(
            listing.problems[0].contains("gone"),
            "{:?}",
            listing.problems
        );
    }

    #[test]
    fn a_sidecar_round_trips_through_its_own_renderer() {
        let probed = Probed {
            family: Some(Family::Proxmox),
            version: Some("Proxmox VE 8.4-1".to_string()),
            arch: Some(Arch::X86_64),
            kernel: Some("/boot/linux26".to_string()),
            initrd: Some("/boot/initrd.img".to_string()),
            external: false,
            zstd_initrd: true,
        };
        let text = Sidecar::render("9f86d0", &probed);
        let back = Sidecar::parse(&text);
        assert_eq!(back.digest.as_deref(), Some("9f86d0"));
        assert_eq!(back.probed(), Some(probed));
    }

    #[test]
    fn an_unknown_sidecar_key_is_ignored_rather_than_refused() {
        // A newer rescriptum writing a key this one does not know must not stop it
        // serving the image.
        let back = Sidecar::parse("sha256 = abc\nfamily = proxmox\nfuture-thing = 42\n");
        assert_eq!(back.digest.as_deref(), Some("abc"));
        assert!(back.probed().is_some());
    }

    #[test]
    fn a_missing_directory_is_a_problem_and_not_an_error() {
        // A fleet must never be unable to install because the media directory is not
        // there yet — the answer endpoint is untouched by any of this.
        let catalog = Catalog::new("/nonexistent/rescriptum/media");
        let listing = catalog.listing().expect("still lists");
        assert!(listing.entries.is_empty());
        assert_eq!(listing.problems.len(), 1);
    }

    #[test]
    fn an_image_no_probe_places_is_still_an_entry() {
        let dir = Dir::new("mystery");
        dir.write("mystery.iso", &vec![0u8; 64 * 1024]);

        let listing = Catalog::new(&dir.0).listing().expect("lists");
        let entry = listing.get("mystery").expect("still listed");
        assert_eq!(entry.family(), Family::Unknown);
        assert!(!entry.bootable());
        assert_eq!(entry.describe(), "mystery");
    }
}
