//! Reading ISO9660, far enough to find a file and say where it is.
//!
//! The property this is built on: **a file in an ISO9660 image is one contiguous
//! extent.** So "extract the kernel" is not an extraction at all — it is an offset and
//! a length, and the media listener streams those bytes straight out of the image with
//! a seek. Nothing is unpacked, nothing is copied, and a 1.5 GB image costs the same
//! few kilobytes of reads whether we want its `/boot/linux26` or its `/.disk/info`.
//!
//! Only the read half lives here. Writing — adding `auto-installer-mode.toml` to a
//! Proxmox image without rewriting 1.5 GB — is Phase 4, and it will build on the same
//! parsing.
//!
//! Three name spaces can coexist in one image and this matters more than it sounds:
//!
//! - **ISO9660** proper, whose identifiers are upper-case, dot-bearing and suffixed
//!   with `;1`. `auto-installer-mode.toml` is not expressible in it at all.
//! - **Rock Ridge** (SUSP `NM` entries), which is what Linux actually shows when it
//!   mounts one, and therefore what an installer looking for its own file will see.
//! - **Joliet**, a second directory tree entirely, with UCS-2 names.
//!
//! A lookup here tries the Rock Ridge name first and the ISO9660 identifier second, so
//! a marker path written the way a human writes it resolves either way.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// ISO9660's logical sector. Not configurable in practice — the field exists in the
/// descriptor, and every image ever written puts 2048 in it.
pub const SECTOR: u64 = 2048;
/// The first sixteen sectors are the system area, reserved for boot code. The primary
/// volume descriptor is what follows.
const FIRST_DESCRIPTOR: u64 = 16 * SECTOR;
/// A directory record is at least this long before its name.
const RECORD_HEADER: usize = 33;
/// Guard against a malformed image sending the walk around forever.
const MAX_DEPTH: usize = 16;
/// A directory extent larger than this is not a directory, it is a corrupt field.
const MAX_DIRECTORY: u64 = 16 * 1024 * 1024;

/// Where a file's bytes are, which is all the listener needs to serve it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    /// Byte offset into the image.
    pub offset: u64,
    pub size: u64,
    pub directory: bool,
}

/// Which name spaces an image carries. Phase 1 reports it; Phase 4 refuses on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Trees {
    /// SUSP is present, and with it Rock Ridge's long names.
    pub rock_ridge: bool,
    /// A supplementary descriptor with a Joliet escape sequence.
    pub joliet: bool,
}

/// An opened image: the descriptor facts, and a handle to read extents from.
pub struct Iso {
    file: File,
    path: PathBuf,
    root: Extent,
    /// The Joliet tree's root, when there is one.
    joliet_root: Option<Extent>,
    pub trees: Trees,
    /// The volume identifier, trimmed. The fallback name for an image no probe places.
    pub volume_id: String,
    /// Blocks the descriptor claims, times the sector size. A file extent past this is
    /// a truncated download, and saying so beats serving zeros.
    pub declared_size: u64,
    /// Bytes at the start of every system use area that belong to somebody else, as the
    /// root's `SP` entry declares. Read once at open: recomputing it per record would
    /// re-parse the root directory for every entry in the image.
    susp_skip: usize,
}

impl Iso {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Iso> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;

        let mut primary: Option<[u8; SECTOR as usize]> = None;
        let mut supplementary: Option<[u8; SECTOR as usize]> = None;

        // Volume descriptors run from sector 16 until a terminator. A handful of images
        // carry a dozen; none carries hundreds, so the cap is a guard, not a policy.
        for index in 0..32u64 {
            let mut sector = [0u8; SECTOR as usize];
            file.seek(SeekFrom::Start(FIRST_DESCRIPTOR + index * SECTOR))?;
            if file.read_exact(&mut sector).is_err() {
                break;
            }
            if &sector[1..6] != b"CD001" {
                // Not a descriptor at all. If we have not even found the primary yet,
                // this is not an ISO9660 image and saying so is the whole answer.
                break;
            }
            match sector[0] {
                1 => primary = Some(sector),
                2 if supplementary.is_none() && is_joliet(&sector) => supplementary = Some(sector),
                255 => break,
                _ => {}
            }
        }

        let Some(pvd) = primary else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} has no ISO9660 primary volume descriptor — it is not an ISO image",
                    path.display()
                ),
            ));
        };

        // The root directory record sits inside the descriptor, all 34 bytes of it.
        let root = record(&pvd[156..190], 0).map(|r| r.extent).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: the root directory record is malformed", path.display()),
            )
        })?;

        let joliet_root = supplementary
            .as_ref()
            .and_then(|svd| record(&svd[156..190], 0))
            .map(|r| r.extent);

        let declared_size = both_endian32(&pvd[80..88]).unwrap_or(0) as u64 * SECTOR;
        let volume_id = strip(&pvd[40..72]);

        let mut iso = Iso {
            file,
            path,
            root,
            joliet_root,
            trees: Trees {
                rock_ridge: false,
                joliet: supplementary.is_some(),
            },
            volume_id,
            declared_size,
            susp_skip: 0,
        };
        iso.trees.rock_ridge = iso.detect_rock_ridge();
        Ok(iso)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolve an absolute path inside the image.
    ///
    /// **This is the path-traversal guard, and it is structural rather than a check.**
    /// An ISO9660 image is its own root: `../../../etc/shadow` resolves to nothing here
    /// because no such record exists in its directory tree. `..` is refused all the
    /// same — belt and braces cost one line — but the property does not depend on it.
    pub fn locate(&mut self, path: &str) -> io::Result<Option<Extent>> {
        let root = self.root;
        self.walk_in(root, path, false)
    }

    /// The same, in the Joliet tree, for an image that has one.
    pub fn locate_joliet(&mut self, path: &str) -> io::Result<Option<Extent>> {
        let Some(root) = self.joliet_root else {
            return Ok(None);
        };
        self.walk_in(root, path, true)
    }

    fn walk_in(&mut self, root: Extent, path: &str, joliet: bool) -> io::Result<Option<Extent>> {
        let segments: Vec<&str> = path
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect();
        if segments.len() > MAX_DEPTH || segments.contains(&"..") {
            return Ok(None);
        }

        let mut current = root;
        for segment in segments {
            // A path continued past a plain file matches nothing.
            if !current.directory {
                return Ok(None);
            }
            match self.entry_in(current, segment, joliet)? {
                Some(found) => current = found,
                None => return Ok(None),
            }
        }
        Ok(Some(current))
    }

    /// One directory's worth of records, matched by name.
    fn entry_in(
        &mut self,
        directory: Extent,
        name: &str,
        joliet: bool,
    ) -> io::Result<Option<Extent>> {
        let wanted = name.to_ascii_lowercase();
        for entry in self.entries(directory)? {
            let matches = if joliet {
                entry.joliet_name.as_deref() == Some(wanted.as_str())
            } else {
                entry.rock_ridge_name.as_deref() == Some(wanted.as_str()) || entry.name == wanted
            };
            if matches {
                return Ok(Some(entry.extent));
            }
        }
        Ok(None)
    }

    /// Every record in a directory extent. Used by lookups and by the probe.
    pub fn entries(&mut self, directory: Extent) -> io::Result<Vec<Entry>> {
        if !directory.directory || directory.size == 0 || directory.size > MAX_DIRECTORY {
            return Ok(Vec::new());
        }
        let mut buffer = vec![0u8; directory.size as usize];
        self.file.seek(SeekFrom::Start(directory.offset))?;
        self.file.read_exact(&mut buffer)?;

        let mut entries = Vec::new();
        let mut at = 0usize;
        while at + RECORD_HEADER <= buffer.len() {
            let length = buffer[at] as usize;
            if length == 0 {
                // A zero length pads to the end of the sector; records resume in the
                // next one. This is normal, not the end of the directory.
                at = (at / SECTOR as usize + 1) * SECTOR as usize;
                continue;
            }
            if length < RECORD_HEADER || at + length > buffer.len() {
                break;
            }
            if let Some(entry) = record(&buffer[at..at + length], self.skip_length()) {
                // `.` and `..` are records with one-byte identifiers 0x00 and 0x01, and
                // no caller here wants them.
                if !entry.special {
                    entries.push(entry);
                }
            }
            at += length;
        }
        Ok(entries)
    }

    /// Read a whole file out of the image, for the small ones — a version string, a
    /// mode file. Anything image-sized is streamed by the listener instead.
    pub fn read(&mut self, path: &str, limit: u64) -> io::Result<Option<Vec<u8>>> {
        let Some(extent) = self.locate(path)? else {
            return Ok(None);
        };
        if extent.directory {
            return Ok(None);
        }
        let size = extent.size.min(limit) as usize;
        let mut buffer = vec![0u8; size];
        self.file.seek(SeekFrom::Start(extent.offset))?;
        self.file.read_exact(&mut buffer)?;
        Ok(Some(buffer))
    }

    /// Whether a path resolves at all, in either tree. The probe's whole vocabulary.
    pub fn has(&mut self, path: &str) -> bool {
        matches!(self.locate(path), Ok(Some(e)) if !e.directory)
            || matches!(self.locate_joliet(path), Ok(Some(e)) if !e.directory)
    }

    fn skip_length(&self) -> usize {
        self.susp_skip
    }

    /// SUSP announces itself in the root directory's `.` record, and declares how many
    /// bytes of each system use area belong to somebody else.
    fn detect_rock_ridge(&mut self) -> bool {
        // The `.` record of the root directory carries the SP entry, which is what says
        // SUSP — and therefore Rock Ridge's `NM` names — is in use at all.
        let root = self.root;
        let Ok(mut buffer) = self.read_extent_head(root, SECTOR as usize) else {
            return false;
        };
        buffer.truncate(root.size.min(SECTOR) as usize);
        if buffer.len() < RECORD_HEADER {
            return false;
        }
        let length = buffer[0] as usize;
        if length < RECORD_HEADER || length > buffer.len() {
            return false;
        }
        let name_len = buffer[32] as usize;
        let mut at = RECORD_HEADER + name_len;
        if name_len % 2 == 0 {
            at += 1;
        }
        let system = &buffer[at.min(length)..length];
        // SP is "SP", length 7, version 1, then 0xBE 0xEF, then the skip length.
        let mut found = false;
        for entry in susp_entries(system, 0) {
            if entry.signature == *b"SP" && entry.data.len() >= 3 {
                found = true;
                self.susp_skip = entry.data[2] as usize;
            }
        }
        found
    }

    fn read_extent_head(&mut self, extent: Extent, want: usize) -> io::Result<Vec<u8>> {
        let size = (extent.size as usize).min(want);
        let mut buffer = vec![0u8; size];
        self.file.seek(SeekFrom::Start(extent.offset))?;
        self.file.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    /// Hand out the file so the listener can stream an extent without reopening.
    pub fn into_file(self) -> File {
        self.file
    }
}

/// One directory record, with every name it answers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The ISO9660 identifier, lower-cased, with the `;1` version and a trailing dot
    /// removed — the form a human would have written.
    pub name: String,
    /// The Rock Ridge `NM` name, when the record carries one.
    pub rock_ridge_name: Option<String>,
    /// The Joliet name, when this record came from that tree.
    pub joliet_name: Option<String>,
    pub extent: Extent,
    /// `.` or `..`.
    pub special: bool,
}

fn record(bytes: &[u8], susp_skip: usize) -> Option<Entry> {
    if bytes.len() < RECORD_HEADER {
        return None;
    }
    let name_len = bytes[32] as usize;
    if RECORD_HEADER + name_len > bytes.len() {
        return None;
    }
    let lba = both_endian32(&bytes[2..10])?;
    let size = both_endian32(&bytes[10..18])?;
    let directory = bytes[25] & 0x02 != 0;
    let raw = &bytes[RECORD_HEADER..RECORD_HEADER + name_len];

    let special = name_len == 1 && (raw[0] == 0 || raw[0] == 1);
    let extent = Extent {
        offset: lba as u64 * SECTOR,
        size: size as u64,
        directory,
    };

    // The system use area begins after the identifier, plus a pad byte when the
    // identifier length is even.
    let mut at = RECORD_HEADER + name_len;
    if name_len % 2 == 0 {
        at += 1;
    }
    let system = bytes.get(at..).unwrap_or(&[]);

    Some(Entry {
        name: iso_name(raw),
        rock_ridge_name: rock_ridge_name(system, susp_skip),
        joliet_name: ucs2_name(raw),
        extent,
        special,
    })
}

/// `LINUX26.;1` and `BOOT` and `README.TXT;1` all become what a person would type.
fn iso_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = text.split(';').next().unwrap_or("");
    text.trim_end_matches('.').to_ascii_lowercase()
}

/// Joliet identifiers are UCS-2, big-endian. Anything outside the basic plane is not
/// something a boot file is named, so a lossy decode is the honest one.
fn ucs2_name(raw: &[u8]) -> Option<String> {
    if raw.len() < 2 || raw.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    let text = text.split(';').next().unwrap_or("");
    Some(text.trim_end_matches('.').to_ascii_lowercase())
}

/// The `NM` entries of a system use area, concatenated — a long name can be split
/// across several with a CONTINUE flag.
fn rock_ridge_name(system: &[u8], skip: usize) -> Option<String> {
    let mut name = String::new();
    for entry in susp_entries(system, skip) {
        if entry.signature != *b"NM" || entry.data.is_empty() {
            continue;
        }
        let flags = entry.data[0];
        // Bits 1 and 2 mean "current" and "parent"; those are not names.
        if flags & 0b0000_0110 != 0 {
            return None;
        }
        name.push_str(&String::from_utf8_lossy(&entry.data[1..]));
        if flags & 0b0000_0001 == 0 {
            break;
        }
    }
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

struct Susp<'a> {
    signature: [u8; 2],
    data: &'a [u8],
}

/// Walk a system use area. Entries are `signature[2] len version data…`.
fn susp_entries(system: &[u8], skip: usize) -> Vec<Susp<'_>> {
    let mut out = Vec::new();
    let mut at = skip.min(system.len());
    while at + 4 <= system.len() {
        let length = system[at + 2] as usize;
        if length < 4 || at + length > system.len() {
            break;
        }
        out.push(Susp {
            signature: [system[at], system[at + 1]],
            data: &system[at + 4..at + length],
        });
        at += length;
    }
    out
}

/// ISO9660 records every number twice, little-endian then big-endian. Read the little
/// half and check the other agrees — a disagreement is a corrupt image, and trusting
/// either half of it would serve garbage.
fn both_endian32(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 8 {
        return None;
    }
    let little = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let big = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    (little == big).then_some(little)
}

fn is_joliet(sector: &[u8]) -> bool {
    // The escape sequences at offset 88: %/@, %/C, %/E — UCS-2 at three levels.
    let escapes = &sector[88..120];
    escapes
        .windows(3)
        .any(|w| w == b"%/@" || w == b"%/C" || w == b"%/E")
}

fn strip(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

/// Building an ISO9660 image, for tests and for nothing else.
///
/// **No binary fixture in the repository.** A checked-in ISO is a blob nobody can
/// review, that nobody can vary, and that grows a repository by megabytes. This builds
/// exactly the image a test needs, in memory, and doubles as the case generator for the
/// Rock Ridge, Joliet and refusal paths Phase 4 will need.
#[cfg(test)]
pub mod build {
    use super::*;
    use std::collections::BTreeMap;

    /// What to put in the image, and which name spaces to describe it in.
    pub struct Builder {
        files: BTreeMap<String, Vec<u8>>,
        pub volume_id: String,
        pub rock_ridge: bool,
        pub joliet: bool,
    }

    impl Default for Builder {
        fn default() -> Self {
            Builder {
                files: BTreeMap::new(),
                volume_id: "TEST VOLUME".to_string(),
                rock_ridge: true,
                joliet: false,
            }
        }
    }

    impl Builder {
        pub fn new() -> Builder {
            Builder::default()
        }

        /// One file, at `/name` or `/dir/name`. One level of directories is all any
        /// marker path in the probe table needs.
        pub fn file(mut self, path: &str, content: &[u8]) -> Builder {
            self.files
                .insert(path.trim_start_matches('/').to_string(), content.to_vec());
            self
        }

        pub fn volume(mut self, id: &str) -> Builder {
            self.volume_id = id.to_string();
            self
        }

        pub fn rock_ridge(mut self, on: bool) -> Builder {
            self.rock_ridge = on;
            self
        }

        pub fn joliet(mut self, on: bool) -> Builder {
            self.joliet = on;
            self
        }

        pub fn build(&self) -> Vec<u8> {
            // Directories, then their files, each on its own sector boundary.
            let mut dirs: BTreeMap<String, Vec<String>> = BTreeMap::new();
            dirs.insert(String::new(), Vec::new());
            for path in self.files.keys() {
                match path.rsplit_once('/') {
                    Some((dir, _)) => {
                        dirs.entry(dir.to_string()).or_default().push(path.clone());
                        // Every ancestor has to exist as a directory of its own —
                        // `/images/pxeboot/vmlinuz` needs `/images` even though no file
                        // sits directly in it.
                        let mut ancestor = dir;
                        while let Some((parent, _)) = ancestor.rsplit_once('/') {
                            dirs.entry(parent.to_string()).or_default();
                            ancestor = parent;
                        }
                    }
                    None => dirs.entry(String::new()).or_default().push(path.clone()),
                }
            }

            // Sector 16 is the PVD, 17 the SVD when there is one, then the terminator.
            // Data starts at 20 — fixed, so a failing test's offsets are readable.
            let mut next = 20u32;
            let mut dir_lba: BTreeMap<String, u32> = BTreeMap::new();
            for dir in dirs.keys() {
                dir_lba.insert(dir.clone(), next);
                next += 1;
            }
            // The Joliet tree is a second set of directory extents over the same data.
            let mut joliet_lba: BTreeMap<String, u32> = BTreeMap::new();
            if self.joliet {
                for dir in dirs.keys() {
                    joliet_lba.insert(dir.clone(), next);
                    next += 1;
                }
            }
            let mut file_lba: BTreeMap<String, (u32, u32)> = BTreeMap::new();
            for (path, content) in &self.files {
                let sectors = (content.len() as u64).div_ceil(SECTOR).max(1) as u32;
                file_lba.insert(path.clone(), (next, content.len() as u32));
                next += sectors;
            }
            let total = next;

            let mut image = vec![0u8; total as usize * SECTOR as usize];
            let put = |image: &mut Vec<u8>, lba: u32, bytes: &[u8]| {
                let at = lba as usize * SECTOR as usize;
                image[at..at + bytes.len()].copy_from_slice(bytes);
            };

            // Descriptors.
            let root_lba = dir_lba[""];
            put(
                &mut image,
                16,
                &self.descriptor(1, root_lba, self.dir_size(&dirs, "", false), total),
            );
            let terminator_lba = if self.joliet {
                let joliet_root = joliet_lba[""];
                put(
                    &mut image,
                    17,
                    &self.descriptor(2, joliet_root, self.dir_size(&dirs, "", true), total),
                );
                18
            } else {
                17
            };
            let mut terminator = vec![0u8; SECTOR as usize];
            terminator[0] = 255;
            terminator[1..6].copy_from_slice(b"CD001");
            terminator[6] = 1;
            put(&mut image, terminator_lba, &terminator);

            // Directory extents, in both trees.
            for (dir, children) in &dirs {
                let extent = self.directory(dir, children, &dirs, &dir_lba, &file_lba, false);
                put(&mut image, dir_lba[dir], &extent);
                if self.joliet {
                    let extent = self.directory(dir, children, &dirs, &joliet_lba, &file_lba, true);
                    put(&mut image, joliet_lba[dir], &extent);
                }
            }

            // File data.
            for (path, content) in &self.files {
                put(&mut image, file_lba[path].0, content);
            }
            image
        }

        fn dir_size(&self, dirs: &BTreeMap<String, Vec<String>>, dir: &str, _joliet: bool) -> u32 {
            let _ = dirs;
            let _ = dir;
            SECTOR as u32
        }

        fn descriptor(&self, kind: u8, root_lba: u32, root_size: u32, total: u32) -> Vec<u8> {
            let mut d = vec![0u8; SECTOR as usize];
            d[0] = kind;
            d[1..6].copy_from_slice(b"CD001");
            d[6] = 1;
            d[8..40].fill(b' ');
            d[40..72].fill(b' ');
            let id = self.volume_id.as_bytes();
            let n = id.len().min(32);
            d[40..40 + n].copy_from_slice(&id[..n]);
            d[80..88].copy_from_slice(&both32(total));
            if kind == 2 {
                // The escape sequence is what makes a supplementary descriptor Joliet.
                d[88..91].copy_from_slice(b"%/E");
            }
            d[120..124].copy_from_slice(&both16(1));
            d[124..128].copy_from_slice(&both16(1));
            d[128..132].copy_from_slice(&both16(SECTOR as u16));
            let root = self.record(&[0u8], root_lba, root_size, true, kind == 2, true);
            d[156..156 + root.len()].copy_from_slice(&root);
            d
        }

        fn directory(
            &self,
            dir: &str,
            children: &[String],
            dirs: &BTreeMap<String, Vec<String>>,
            dir_lba: &BTreeMap<String, u32>,
            file_lba: &BTreeMap<String, (u32, u32)>,
            joliet: bool,
        ) -> Vec<u8> {
            let mut out = Vec::new();
            // `.` carries the SP entry that announces SUSP, in the root only.
            out.extend_from_slice(&self.record(
                &[0u8],
                dir_lba[dir],
                SECTOR as u32,
                true,
                joliet,
                dir.is_empty(),
            ));
            let parent = dir.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
            out.extend_from_slice(&self.record(
                &[1u8],
                dir_lba[parent],
                SECTOR as u32,
                true,
                joliet,
                false,
            ));

            // Subdirectories of this one.
            for other in dirs.keys() {
                if other.is_empty() || other == dir {
                    continue;
                }
                let (its_parent, base) = other.rsplit_once('/').unwrap_or(("", other.as_str()));
                if its_parent != dir {
                    continue;
                }
                let name = self.identifier(base, true, joliet);
                out.extend_from_slice(&self.record(
                    &name,
                    dir_lba[other],
                    SECTOR as u32,
                    true,
                    joliet,
                    false,
                ));
            }

            for path in children {
                let base = path.rsplit_once('/').map(|(_, b)| b).unwrap_or(path);
                let name = self.identifier(base, false, joliet);
                let (lba, size) = file_lba[path];
                out.extend_from_slice(&self.record(&name, lba, size, false, joliet, false));
            }
            assert!(
                out.len() <= SECTOR as usize,
                "test directory outgrew a sector"
            );
            out
        }

        /// ISO9660 identifiers are upper-case and version-suffixed; Joliet's are UCS-2.
        /// A name that cannot be expressed in ISO9660 — a hyphen, lower case — is
        /// deliberately mangled here exactly as a real mastering tool would mangle it,
        /// because that mangling is the trap Rock Ridge exists to undo.
        fn identifier(&self, base: &str, directory: bool, joliet: bool) -> Vec<u8> {
            if joliet {
                let mut out = Vec::new();
                for unit in base.encode_utf16() {
                    out.extend_from_slice(&unit.to_be_bytes());
                }
                return out;
            }
            let mangled: String = base
                .to_ascii_uppercase()
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '.' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            if directory {
                mangled.into_bytes()
            } else {
                format!("{mangled};1").into_bytes()
            }
        }

        fn record(
            &self,
            name: &[u8],
            lba: u32,
            size: u32,
            directory: bool,
            joliet: bool,
            with_sp: bool,
        ) -> Vec<u8> {
            let mut system: Vec<u8> = Vec::new();
            if self.rock_ridge && !joliet {
                if with_sp {
                    system.extend_from_slice(&[b'S', b'P', 7, 1, 0xBE, 0xEF, 0]);
                }
                // A `.`/`..` record gets no NM: its name is structural.
                if !(name.len() == 1 && (name[0] == 0 || name[0] == 1)) {
                    // The real name, the one a mounted image shows.
                    let real = self.rock_ridge_name_for(name);
                    let mut nm = vec![b'N', b'M', (5 + real.len()) as u8, 1, 0];
                    nm.extend_from_slice(real.as_bytes());
                    system.extend_from_slice(&nm);
                }
            }

            let mut r = vec![0u8; RECORD_HEADER];
            r[2..10].copy_from_slice(&both32(lba));
            r[10..18].copy_from_slice(&both32(size));
            r[25] = if directory { 0x02 } else { 0 };
            r[28..32].copy_from_slice(&both16(1));
            r[32] = name.len() as u8;
            r.extend_from_slice(name);
            if name.len() % 2 == 0 {
                r.push(0);
            }
            r.extend_from_slice(&system);
            if r.len() % 2 == 1 {
                r.push(0);
            }
            r[0] = r.len() as u8;
            r
        }

        /// The builder mangles ISO9660 identifiers, so the Rock Ridge name has to be
        /// recovered from what the caller asked for. Tests set it through `file`, and
        /// the mangling is reversible enough for the fixture's purposes: the NM name is
        /// the original base name, which is exactly what a mastering tool records.
        fn rock_ridge_name_for(&self, mangled: &[u8]) -> String {
            let text = String::from_utf8_lossy(mangled);
            let stem = text.split(';').next().unwrap_or("").to_ascii_lowercase();
            // Find the original spelling among the paths we were given.
            for path in self.files.keys() {
                let base = path.rsplit_once('/').map(|(_, b)| b).unwrap_or(path);
                if mangle_matches(base, &stem) {
                    return base.to_string();
                }
                if let Some((dir, _)) = path.split_once('/')
                    && mangle_matches(dir, &stem)
                {
                    return dir.to_string();
                }
            }
            stem
        }
    }

    fn mangle_matches(original: &str, mangled: &str) -> bool {
        let expected: String = original
            .to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        expected == mangled
    }

    fn both32(value: u32) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[..4].copy_from_slice(&value.to_le_bytes());
        out[4..].copy_from_slice(&value.to_be_bytes());
        out
    }

    fn both16(value: u16) -> [u8; 4] {
        let mut out = [0u8; 4];
        out[..2].copy_from_slice(&value.to_le_bytes());
        out[2..].copy_from_slice(&value.to_be_bytes());
        out
    }

    /// Write an image to a temporary file and open it, which is what every test wants.
    pub fn open(name: &str, builder: &Builder) -> (std::path::PathBuf, Iso) {
        let dir = std::env::temp_dir().join(format!("rescriptum-iso-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{name}.iso"));
        std::fs::write(&path, builder.build()).expect("write image");
        let iso = Iso::open(&path).expect("opens");
        (path, iso)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_resolves_to_an_offset_and_a_length() {
        // The property the whole listener rests on: a file is a contiguous extent, so
        // serving it is a seek and a length rather than an extraction.
        let (_path, mut iso) = build::open(
            "basic",
            &build::Builder::new().file("/boot/linux26", b"kernel bytes"),
        );

        let extent = iso
            .locate("/boot/linux26")
            .expect("readable")
            .expect("present");
        assert_eq!(extent.size, 12);
        assert_eq!(extent.offset % SECTOR, 0, "extents are sector-aligned");
        assert!(!extent.directory);

        assert_eq!(
            iso.read("/boot/linux26", 1024).expect("readable"),
            Some(b"kernel bytes".to_vec())
        );
    }

    #[test]
    fn a_missing_path_is_absence_rather_than_an_error() {
        let (_path, mut iso) =
            build::open("missing", &build::Builder::new().file("/present", b"x"));
        assert_eq!(iso.locate("/absent").expect("readable"), None);
        assert_eq!(iso.locate("/absent/deeper").expect("readable"), None);
        // A path continued past a plain file resolves to nothing rather than to the file.
        assert_eq!(iso.locate("/present/deeper").expect("readable"), None);
    }

    #[test]
    fn traversal_out_of_the_image_resolves_to_nothing() {
        // The guard is structural — no such record exists in the directory tree — and
        // `..` is refused outright on top of that.
        let (_path, mut iso) = build::open(
            "traversal",
            &build::Builder::new().file("/boot/linux26", b"k"),
        );
        for path in [
            "/../etc/shadow",
            "/boot/../../etc/shadow",
            "../../../etc/passwd",
        ] {
            assert_eq!(iso.locate(path).expect("readable"), None, "{path}");
        }
    }

    #[test]
    fn a_rock_ridge_name_wins_over_the_mangled_identifier() {
        // `auto-installer-mode.toml` cannot be spelled in ISO9660 — hyphens are not
        // allowed, lower case is not allowed, and 8.3 does not stretch that far. The
        // record is called `AUTO_INSTALLER_MODE.TOM;1` and only the Rock Ridge `NM`
        // entry carries the name the installer actually looks for. Getting this wrong
        // is the trap that puts the file in the image and hides it from its reader.
        let (_path, mut iso) = build::open(
            "rockridge",
            &build::Builder::new().file("/auto-installer-mode.toml", b"mode = \"http\"\n"),
        );
        assert!(iso.trees.rock_ridge, "SP announces SUSP");
        assert!(iso.has("/auto-installer-mode.toml"));
        // The mangled identifier still resolves, because it is genuinely in the image.
        assert!(iso.has("/auto_installer_mode.toml"));
    }

    #[test]
    fn an_image_without_rock_ridge_only_answers_to_the_mangled_name() {
        let (_path, mut iso) = build::open(
            "plain",
            &build::Builder::new()
                .rock_ridge(false)
                .file("/auto-installer-mode.toml", b"x"),
        );
        assert!(!iso.trees.rock_ridge);
        assert!(
            !iso.has("/auto-installer-mode.toml"),
            "the hyphen is not in the image"
        );
        assert!(iso.has("/auto_installer_mode.toml"));
    }

    #[test]
    fn a_joliet_tree_is_detected_and_searchable() {
        // Which tree a mount reads is not ours to decide, so both have to be visible.
        let (_path, mut iso) = build::open(
            "joliet",
            &build::Builder::new()
                .rock_ridge(false)
                .joliet(true)
                .file("/boot/linux26", b"kernel"),
        );
        assert!(iso.trees.joliet);
        assert!(
            iso.locate_joliet("/boot/linux26")
                .expect("readable")
                .is_some(),
            "the Joliet tree carries the readable name"
        );
        assert!(iso.has("/boot/linux26"));
    }

    #[test]
    fn the_volume_identifier_is_the_fallback_name() {
        let (_path, iso) = build::open(
            "volume",
            &build::Builder::new().volume("PVE 8.4-1").file("/x", b"y"),
        );
        assert_eq!(iso.volume_id, "PVE 8.4-1");
        assert!(iso.declared_size > 0);
    }

    #[test]
    fn something_that_is_not_an_iso_is_refused_by_name() {
        let dir = std::env::temp_dir().join(format!("rescriptum-iso-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("not-an-iso.iso");
        std::fs::write(&path, vec![0u8; 100 * 1024]).expect("write");

        // `Iso` holds an open file rather than deriving `Debug`, so unwrap by hand.
        let Err(e) = Iso::open(&path) else {
            panic!("a file of zeros must not open as an image");
        };
        assert!(e.to_string().contains("not an ISO image"), "{e}");
    }

    #[test]
    fn a_number_written_two_ways_must_agree() {
        // ISO9660 records every integer twice. A disagreement is a corrupt image, and
        // trusting either half would serve garbage from a plausible-looking offset.
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&7u32.to_le_bytes());
        bytes[4..].copy_from_slice(&7u32.to_be_bytes());
        assert_eq!(both_endian32(&bytes), Some(7));

        bytes[4..].copy_from_slice(&9u32.to_be_bytes());
        assert_eq!(both_endian32(&bytes), None);
    }
}
