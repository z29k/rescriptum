//! Adding one file to an ISO9660 image without writing 1.5 GB.
//!
//! The only genuinely hard part of this whole design, and it is needed by exactly one
//! installer. Proxmox reads `/auto-installer-mode.toml` from the mounted image to learn
//! where its answer lives; every other family takes a URL on the kernel command line.
//!
//! ## Why this is tractable at all
//!
//! **On the PXE path the image is never booted, only mounted.** Bootability is not at
//! stake — the requirement is *"still a readable ISO9660 filesystem exposing one more
//! file"*, which is a far weaker problem than the one xorriso solves. No boot catalog,
//! no hybrid MBR, no repacking.
//!
//! An ISO9660 file is a contiguous extent, so adding one is three small overwrites and
//! an append:
//!
//! 1. **Append the content** past the end of the image.
//! 2. **Write a directory record** into the slack at the end of the root directory's
//!    extent — directory extents are padded to 2048 bytes, and a root with a dozen
//!    entries typically leaves a kilobyte free.
//! 3. **Bump the volume space size** in the primary descriptor, and in the
//!    supplementary one if there is a Joliet tree.
//!
//! So this does not produce a file. It produces a **plan** — a list of `(offset, bytes)`
//! plus a tail — which the listener applies *while streaming*. What goes on the wire is
//! the source image with a few hundred bytes substituted and a few hundred appended:
//! no second copy on disk, the source never mutated so its published digest stays
//! verifiable, ranges still work because the arithmetic is trivial, and changing the
//! answer URL is a matter of recomputing 300 bytes.
//!
//! ## The trap that decides whether this works: Rock Ridge
//!
//! `auto-installer-mode.toml` is **not a legal ISO9660 identifier** — hyphens are not
//! d-characters, lower case is not allowed, and it exceeds 8.3. The record would be
//! called something like `AUTO_INS.TOM;1`, and **the installer would never find its
//! file**: it would be in the image and invisible to its only reader.
//!
//! Linux mounts iso9660 with Rock Ridge auto-detected and Rock Ridge names win, so the
//! record carries an `NM` entry with the real name. Where there is a Joliet tree the
//! record goes there too, in UCS-2 — because **which tree the mount reads is not ours
//! to decide**. Where there is neither, this refuses, and refusing is a *complete*
//! answer: the fallback is one command on any Debian box
//! (`proxmox-auto-install-assistant prepare-iso`), whose output this server is perfectly
//! happy to serve.

use super::iso::{Extent, Iso, SECTOR};
use std::path::{Path, PathBuf};

/// One overwrite inside the original image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overwrite {
    pub at: u64,
    pub bytes: Vec<u8>,
}

/// What to substitute and what to append, with the arithmetic already done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub source: PathBuf,
    /// The original image's length. Everything past it is `tail`.
    pub source_len: u64,
    pub overwrites: Vec<Overwrite>,
    pub tail: Vec<u8>,
    /// What was added, for a listing to report.
    pub added: String,
}

impl Plan {
    /// The length of the image this plan describes. Exact arithmetic, because a
    /// `Content-Length` computed any other way would be a guess.
    pub fn len(&self) -> u64 {
        self.source_len + self.tail.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.overwrites.is_empty() && self.tail.is_empty()
    }

    /// Fill `buffer` with the patched image's bytes starting at `offset`.
    ///
    /// This is what makes a range request work over a virtual file: the offsets are the
    /// patched image's, and every one of them resolves to either the source, an
    /// overwrite, or the tail.
    pub fn read_at(
        &self,
        source: &mut std::fs::File,
        offset: u64,
        buffer: &mut [u8],
    ) -> std::io::Result<usize> {
        use std::io::{Read, Seek, SeekFrom};

        let total = self.len();
        if offset >= total {
            return Ok(0);
        }
        let want = buffer.len().min((total - offset) as usize);
        let buffer = &mut buffer[..want];

        // The source's part of this window, then the tail's.
        let from_source = if offset < self.source_len {
            (self.source_len - offset).min(want as u64) as usize
        } else {
            0
        };
        if from_source > 0 {
            source.seek(SeekFrom::Start(offset))?;
            source.read_exact(&mut buffer[..from_source])?;
        }
        if from_source < want {
            // Where in the tail this window starts: at its beginning when the window
            // straddles the join, and further in when it starts past the source's end.
            let tail_start = offset.saturating_sub(self.source_len) as usize;
            let take = want - from_source;
            let end = (tail_start + take).min(self.tail.len());
            let slice = &self.tail[tail_start.min(self.tail.len())..end];
            buffer[from_source..from_source + slice.len()].copy_from_slice(slice);
            // Past the tail is zero, which only happens if the arithmetic above is
            // wrong; leaving it zero beats reading somebody else's memory.
            for byte in &mut buffer[from_source + slice.len()..] {
                *byte = 0;
            }
        }

        // Then substitute, which is what makes this a patch rather than a copy.
        for overwrite in &self.overwrites {
            let end = overwrite.at + overwrite.bytes.len() as u64;
            if end <= offset || overwrite.at >= offset + want as u64 {
                continue;
            }
            let from = overwrite.at.max(offset);
            let to = end.min(offset + want as u64);
            let in_buffer = (from - offset) as usize;
            let in_patch = (from - overwrite.at) as usize;
            let count = (to - from) as usize;
            buffer[in_buffer..in_buffer + count]
                .copy_from_slice(&overwrite.bytes[in_patch..in_patch + count]);
        }
        Ok(want)
    }

    /// Write the whole thing out, for a USB stick. **One code path with the streaming
    /// one**: `media export` materialises exactly what the listener would have served.
    pub fn materialise(&self, to: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut source = std::fs::File::open(&self.source)?;
        let mut out = std::fs::File::create(to)?;
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut at = 0u64;
        while at < self.len() {
            let n = self.read_at(&mut source, at, &mut buffer)?;
            if n == 0 {
                break;
            }
            out.write_all(&buffer[..n])?;
            at += n as u64;
        }
        out.flush()
    }
}

/// Plan the addition of one file at the root of an image.
pub fn add_file(path: &Path, name: &str, content: &[u8]) -> Result<Plan, String> {
    let mut iso = Iso::open(path).map_err(|e| format!("{}: {e}", path.display()))?;

    // A Windows ISO is UDF+ISO9660 and its large files exist only in the UDF tree.
    // Patching the ISO9660 tree of such an image produces something that looks right and
    // is not, which is the worst of the three outcomes.
    if iso.trees.udf {
        return Err(format!(
            "{}: the image carries a UDF filesystem, which this does not understand. \
             Patching only its ISO9660 tree would produce an image that looks right and \
             is not. Prepare it with the vendor's own tool instead.",
            path.display()
        ));
    }
    // Neither long-name tree: the record would be in the image under a mangled name and
    // invisible to the only reader that wants it.
    if !iso.trees.rock_ridge && !iso.trees.joliet {
        return Err(format!(
            "{}: the image has neither Rock Ridge nor Joliet, so {name} could only exist \
             under a mangled 8.3 name and the installer would never find it.",
            path.display()
        ));
    }

    let source_len = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    // Every extent is addressed in sectors, so an image that does not end on one has no
    // sector to put the new file in.
    if source_len == 0 || source_len % SECTOR != 0 {
        return Err(format!(
            "{}: the image is {source_len} bytes, which is not a whole number of 2048-byte \
             sectors — there is nowhere to append a file.",
            path.display()
        ));
    }
    if content.len() as u64 > SECTOR * 64 {
        // This exists to add a few hundred bytes of configuration. Anything larger is a
        // different job and should say so rather than half-working.
        return Err(format!(
            "{name} is {} bytes; this adds small files, not payloads.",
            content.len()
        ));
    }

    let lba = (source_len / SECTOR) as u32;
    let mut overwrites = Vec::new();

    // Read out of the descriptors before the borrow checker has an opinion about
    // holding one while reading through the other.
    let root = iso.root_extent();
    let joliet_root = iso.joliet_root_extent();
    let susp_skip = iso.susp_skip();

    // The ISO9660 tree, with a Rock Ridge `NM` entry carrying the real name.
    if iso.trees.rock_ridge {
        let record = iso9660_record(name, lba, content.len() as u32, true, susp_skip);
        let at = slack_in(&mut iso, root, record.len())?;
        overwrites.push(Overwrite { at, bytes: record });
    }

    // The Joliet tree, where names are UCS-2 and case-preserving. Both, when both exist:
    // which one the mount reads is not ours to decide.
    if let Some(joliet_root) = joliet_root {
        let record = joliet_record(name, lba, content.len() as u32);
        let at = slack_in(&mut iso, joliet_root, record.len())?;
        overwrites.push(Overwrite { at, bytes: record });
    }

    // The volume space size, in both descriptors. An image whose descriptor still claims
    // the old length has a file past its own end, and a mount will not read it.
    let added_sectors = (content.len() as u64).div_ceil(SECTOR) as u32;
    let blocks = (source_len / SECTOR) as u32 + added_sectors;
    overwrites.push(Overwrite {
        at: iso.pvd_at() + 80,
        bytes: both_endian32(blocks),
    });
    if let Some(svd_at) = iso.svd_at() {
        overwrites.push(Overwrite {
            at: svd_at + 80,
            bytes: both_endian32(blocks),
        });
    }

    let mut tail = content.to_vec();
    tail.resize(added_sectors as usize * SECTOR as usize, 0);

    Ok(Plan {
        source: path.to_path_buf(),
        source_len,
        overwrites,
        tail,
        added: name.to_string(),
    })
}

/// Find room for a record in a directory extent, and say where it goes.
///
/// **A directory record may not cross a sector boundary**, so this looks per sector
/// rather than at the extent as a whole: the space after the last record in sector three
/// is unusable if the record does not fit in it, even when sector four is empty.
fn slack_in(iso: &mut Iso, directory: Extent, need: usize) -> Result<u64, String> {
    if !directory.directory || directory.size == 0 {
        return Err("the root directory extent is not readable".to_string());
    }
    let extent = iso
        .read_raw(directory.offset, directory.size as usize)
        .map_err(|e| format!("cannot read the root directory: {e}"))?;

    let sector = SECTOR as usize;
    for (index, chunk) in extent.chunks(sector).enumerate() {
        let mut used = 0usize;
        while used + 33 <= chunk.len() {
            let length = chunk[used] as usize;
            if length == 0 {
                break;
            }
            if used + length > chunk.len() {
                // A record claiming to run past its own sector: refuse rather than
                // write into whatever follows.
                return Err("a directory record overruns its sector".to_string());
            }
            used += length;
        }
        if chunk.len() - used >= need {
            return Ok(directory.offset + (index * sector) as u64 + used as u64);
        }
    }

    Err(format!(
        "the root directory has no {need} bytes of slack in any of its sectors. \
         Relocating the extent would drag in the path tables, which is deliberately not \
         done here — prepare the image with `proxmox-auto-install-assistant prepare-iso` \
         instead, and this server will serve the result."
    ))
}

/// An ISO9660 directory record, optionally carrying the real name in a Rock Ridge `NM`.
fn iso9660_record(name: &str, lba: u32, size: u32, rock_ridge: bool, skip: usize) -> Vec<u8> {
    let identifier = mangle(name);
    let mut system: Vec<u8> = vec![0u8; skip];
    if rock_ridge {
        // NM: signature, length, version, flags, then the name. Flags zero means "this
        // is the whole name" — no CONTINUE, not a `.` or `..` marker.
        let mut nm = vec![b'N', b'M', (5 + name.len()) as u8, 1, 0];
        nm.extend_from_slice(name.as_bytes());
        system.extend_from_slice(&nm);
    }
    record(identifier.as_bytes(), lba, size, &system)
}

/// The same record in the Joliet tree, where the name is UCS-2 big-endian and needs no
/// mangling at all — which is why an image with only Joliet is still patchable.
fn joliet_record(name: &str, lba: u32, size: u32) -> Vec<u8> {
    let mut identifier = Vec::new();
    for unit in name.encode_utf16() {
        identifier.extend_from_slice(&unit.to_be_bytes());
    }
    record(&identifier, lba, size, &[])
}

fn record(identifier: &[u8], lba: u32, size: u32, system: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 33];
    out[2..10].copy_from_slice(&both_endian32(lba));
    out[10..18].copy_from_slice(&both_endian32(size));
    // Recording date and time: zeros. Every reader accepts it, and a fabricated
    // timestamp would be a lie about when somebody wrote the file.
    out[25] = 0; // flags: a plain file
    out[28..32].copy_from_slice(&both_endian16(1));
    out[32] = identifier.len() as u8;
    out.extend_from_slice(identifier);
    // A pad byte when the identifier length is even, so the system use area starts on an
    // even offset — which is where a reader looks for it.
    if identifier.len() % 2 == 0 {
        out.push(0);
    }
    out.extend_from_slice(system);
    // Records are even-length.
    if out.len() % 2 == 1 {
        out.push(0);
    }
    out[0] = out.len() as u8;
    out
}

/// The best legal ISO9660 identifier for a name that is not one.
///
/// Deliberately conservative — upper case, `A-Z0-9_`, 8.3 — because the *real* name
/// comes from Rock Ridge or Joliet and this only has to be legal and unlikely to
/// collide. A mastering tool would do the same thing.
fn mangle(name: &str) -> String {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem, extension),
        None => (name, ""),
    };
    let clean = |text: &str, limit: usize| -> String {
        text.to_ascii_uppercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .take(limit)
            .collect()
    };
    let stem = clean(stem, 8);
    let extension = clean(extension, 3);
    if extension.is_empty() {
        format!("{stem}.;1")
    } else {
        format!("{stem}.{extension};1")
    }
}

fn both_endian32(value: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&value.to_le_bytes());
    out.extend_from_slice(&value.to_be_bytes());
    out
}

fn both_endian16(value: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(4);
    out.extend_from_slice(&value.to_le_bytes());
    out.extend_from_slice(&value.to_be_bytes());
    out
}

/// The file Proxmox reads to learn where its answer lives.
///
/// **`AutoInstSettings` is `deny_unknown_fields`**, so one key this does not know about
/// is a *rejected file*, not a warning — and the machine boots the interactive installer
/// with nobody there. Only the five keys upstream defines are ever written.
pub fn mode_file(url: &str, fingerprint: Option<&str>, token: Option<&str>) -> String {
    let mut out = String::from(
        "# Written by rescriptum. The Proxmox installer reads this from the mounted image\n\
         # to learn where to POST its hardware inventory.\n\
         mode = \"http\"\n",
    );
    out.push_str("partition-label = \"proxmox-ais\"\n\n[http]\n");
    out.push_str(&format!("url = \"{}\"\n", escape(url)));
    if let Some(fingerprint) = fingerprint {
        out.push_str(&format!("cert-fingerprint = \"{}\"\n", escape(fingerprint)));
    }
    if let Some(token) = token {
        out.push_str(&format!("token = \"{}\"\n", escape(token)));
    }
    out
}

/// A quote or a backslash in a URL would end the string early, and the result would be
/// a mode file the installer refuses — which reads as this server being broken.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::super::iso::build;
    use super::*;

    fn image(name: &str, builder: &build::Builder) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rescriptum-patch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{name}.iso"));
        std::fs::write(&path, builder.build()).expect("write");
        path
    }

    const MODE: &str = "mode = \"http\"\n[http]\nurl = \"http://192.0.2.10:8000/proxmox\"\n";

    #[test]
    fn a_file_added_to_an_image_reads_back_under_its_real_name() {
        // **The whole point.** `auto-installer-mode.toml` is not a legal ISO9660 name, so
        // without a Rock Ridge `NM` entry the file would be in the image and invisible
        // to the only reader that wants it.
        let path = image(
            "basic",
            &build::Builder::new().file("/boot/linux26", b"kernel"),
        );
        let plan = add_file(&path, "auto-installer-mode.toml", MODE.as_bytes())
            .unwrap_or_else(|e| panic!("{e}"));

        let out = path.with_extension("patched.iso");
        plan.materialise(&out).expect("materialises");

        let mut patched = Iso::open(&out).expect("still an image");
        assert_eq!(
            patched
                .read("/auto-installer-mode.toml", 4096)
                .expect("readable"),
            Some(MODE.as_bytes().to_vec()),
            "the installer must find it under the name it looks for"
        );
    }

    #[test]
    fn everything_that_was_already_there_is_byte_identical() {
        // A corrupt image fails silently, on somebody's USB stick, weeks later.
        let original = build::Builder::new()
            .file("/boot/linux26", b"the kernel, verbatim")
            .file("/boot/initrd.img", b"the initrd, verbatim")
            .file("/.disk/info", b"PRODUCTLONG='Proxmox VE'\n");
        let path = image("preserved", &original);
        let before = std::fs::read(&path).expect("read");

        let plan = add_file(&path, "auto-installer-mode.toml", MODE.as_bytes()).expect("plans");
        let out = path.with_extension("patched.iso");
        plan.materialise(&out).expect("materialises");
        let after = std::fs::read(&out).expect("read");

        // **The first 32 KiB is untouched**, which is where a boot catalog and a hybrid
        // MBR live — the two things whose corruption would only surface on a stick.
        assert_eq!(&before[..32768], &after[..32768], "the system area moved");

        let mut patched = Iso::open(&out).expect("still an image");
        for (name, contents) in [
            ("/boot/linux26", &b"the kernel, verbatim"[..]),
            ("/boot/initrd.img", &b"the initrd, verbatim"[..]),
        ] {
            assert_eq!(
                patched.read(name, 4096).expect("readable"),
                Some(contents.to_vec()),
                "{name} changed"
            );
        }
        // And the source itself was never touched, so its published digest still holds.
        assert_eq!(std::fs::read(&path).expect("read"), before);
    }

    #[test]
    fn the_image_grows_by_exactly_one_sector_for_a_small_file() {
        let path = image("growth", &build::Builder::new().file("/x", b"y"));
        let before = std::fs::metadata(&path).expect("stat").len();
        let plan = add_file(&path, "auto-installer-mode.toml", MODE.as_bytes()).expect("plans");

        assert_eq!(plan.len(), before + SECTOR);
        // Exact arithmetic rather than a guess: this is a `Content-Length`.
        let out = path.with_extension("grown.iso");
        plan.materialise(&out).expect("materialises");
        assert_eq!(std::fs::metadata(&out).expect("stat").len(), plan.len());
    }

    #[test]
    fn a_range_over_the_patched_image_reads_the_same_bytes_as_the_whole_of_it() {
        // The listener serves ranges over this virtual file, so every window has to
        // resolve — through the source, an overwrite, or the tail.
        let path = image(
            "ranges",
            &build::Builder::new().file("/boot/linux26", b"kernel"),
        );
        let plan = add_file(&path, "auto-installer-mode.toml", MODE.as_bytes()).expect("plans");
        let out = path.with_extension("ranged.iso");
        plan.materialise(&out).expect("materialises");
        let whole = std::fs::read(&out).expect("read");

        let mut source = std::fs::File::open(&path).expect("open");
        for size in [1usize, 7, 2048, 4096, 100_000] {
            let mut rebuilt = Vec::new();
            let mut buffer = vec![0u8; size];
            let mut at = 0u64;
            loop {
                let n = plan.read_at(&mut source, at, &mut buffer).expect("reads");
                if n == 0 {
                    break;
                }
                rebuilt.extend_from_slice(&buffer[..n]);
                at += n as u64;
            }
            assert_eq!(rebuilt, whole, "reading in {size}-byte windows disagreed");
        }
    }

    #[test]
    fn an_image_with_only_joliet_is_patched_in_that_tree() {
        // Which tree a mount reads is not ours to decide, and Joliet needs no mangling
        // at all — so an image with only Joliet is still patchable.
        let path = image(
            "joliet",
            &build::Builder::new()
                .rock_ridge(false)
                .joliet(true)
                .file("/boot/linux26", b"kernel"),
        );
        let plan = add_file(&path, "auto-installer-mode.toml", MODE.as_bytes()).expect("plans");
        let out = path.with_extension("joliet-patched.iso");
        plan.materialise(&out).expect("materialises");

        let mut patched = Iso::open(&out).expect("still an image");
        assert!(
            patched
                .locate_joliet("/auto-installer-mode.toml")
                .expect("readable")
                .is_some(),
            "the Joliet tree must carry the readable name"
        );
    }

    #[test]
    fn an_image_with_both_trees_is_patched_in_both() {
        let path = image(
            "both",
            &build::Builder::new()
                .rock_ridge(true)
                .joliet(true)
                .file("/boot/linux26", b"kernel"),
        );
        let plan = add_file(&path, "auto-installer-mode.toml", MODE.as_bytes()).expect("plans");
        // Two records plus two descriptor updates: neither tree may be left behind.
        assert_eq!(plan.overwrites.len(), 4, "{:?}", plan.overwrites);

        let out = path.with_extension("both-patched.iso");
        plan.materialise(&out).expect("materialises");
        let mut patched = Iso::open(&out).expect("still an image");
        assert!(patched.has("/auto-installer-mode.toml"));
        assert!(
            patched
                .locate_joliet("/auto-installer-mode.toml")
                .expect("readable")
                .is_some()
        );
    }

    #[test]
    fn an_image_with_neither_long_name_tree_is_refused_with_the_way_out() {
        // Refusing is a *complete* answer, because the fallback is one command on any
        // Debian box and this server is perfectly happy to serve its output.
        let path = image(
            "mangled-only",
            &build::Builder::new()
                .rock_ridge(false)
                .file("/boot/linux26", b"kernel"),
        );
        let e =
            add_file(&path, "auto-installer-mode.toml", MODE.as_bytes()).expect_err("must refuse");
        assert!(e.contains("neither Rock Ridge nor Joliet"), "{e}");
        assert!(e.contains("would never find it"), "{e}");
    }

    #[test]
    fn a_name_that_is_not_legal_iso9660_is_mangled_the_way_a_mastering_tool_would() {
        // The mangled name is what the record is *called*; the real one lives in the NM
        // entry. Both have to exist, and neither is the other's substitute.
        assert_eq!(mangle("auto-installer-mode.toml"), "AUTO_INS.TOM;1");
        assert_eq!(mangle("readme"), "README.;1");
        assert_eq!(mangle("a.b"), "A.B;1");
    }

    #[test]
    fn the_mode_file_carries_only_the_keys_upstream_defines() {
        // `AutoInstSettings` is `deny_unknown_fields`: one key it does not know is a
        // rejected file, not a warning, and the machine boots the interactive installer
        // with nobody there to answer it.
        let text = mode_file("http://192.0.2.10:8000/proxmox", None, None);
        let keys: Vec<&str> = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && l.contains('='))
            .map(|l| l.split('=').next().unwrap_or("").trim())
            .collect();
        for key in &keys {
            assert!(
                [
                    "mode",
                    "partition-label",
                    "url",
                    "cert-fingerprint",
                    "token"
                ]
                .contains(key),
                "{key} is not a key the installer defines"
            );
        }
        assert!(text.contains("mode = \"http\""), "{text}");
        assert!(text.contains("[http]"), "{text}");
    }

    #[test]
    fn a_quote_in_the_url_cannot_end_the_string_early() {
        // The result would be a mode file the installer refuses, which reads as this
        // server being broken rather than as a bad URL.
        let text = mode_file("http://host/a\"b", None, Some("name:sec\\ret"));
        assert!(text.contains("\\\""), "{text}");
        assert!(text.contains("\\\\"), "{text}");
    }

    #[test]
    fn a_payload_rather_than_a_configuration_file_is_refused() {
        let path = image("payload", &build::Builder::new().file("/x", b"y"));
        let e = add_file(&path, "big.bin", &vec![0u8; 200 * 1024]).expect_err("must refuse");
        assert!(e.contains("small files, not payloads"), "{e}");
    }
}
