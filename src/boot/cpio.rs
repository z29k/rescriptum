//! A cpio writer, for exactly one job: appending an ISO to an initrd as a named member.
//!
//! Proxmox over PXE wants its image visible at `/proxmox.iso` inside the initramfs.
//! Modern iPXE does that itself (`initrd <uri> proxmox.iso`); older loaders cannot, and
//! the community answer has always been to build a 1.5 GB initrd with the image cpio'd
//! into it. We synthesise the same bytes **on the wire** instead of storing them, which
//! is why this is a header generator rather than an archiver: nothing here ever holds a
//! file, it only says what bytes go around one.
//!
//! Two properties of the kernel's initramfs loader make it work with no compressor:
//! concatenated archives are all unpacked, and an **uncompressed** segment among
//! compressed ones is fine.
//!
//! The format is "newc" (SVR4, no CRC): a 110-byte header of ASCII hex fields, the NUL
//! terminated name, then the data, each padded to a four-byte boundary.

/// `c_filesize` is eight hex digits. An image at or past 4 GiB cannot be described, and
/// saying so beats emitting a header that wraps.
pub const MAX_MEMBER: u64 = 0xffff_ffff;

const MAGIC: &[u8; 6] = b"070701";
const HEADER: usize = 110;
/// A regular file, mode 0644.
const MODE_FILE: u32 = 0o100_644;

/// What surrounds one member's data, with the arithmetic already done — the media
/// listener needs an exact `Content-Length` before it has read a byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Header, name, and the padding that aligns the data that follows.
    pub prefix: Vec<u8>,
    /// Zero bytes after the data, aligning whatever comes next.
    pub padding: usize,
}

impl Member {
    /// Total bytes this member contributes, data included.
    pub fn len(&self, data: u64) -> u64 {
        self.prefix.len() as u64 + data + self.padding as u64
    }
}

/// Describe a regular file member. `size` is the data that will follow the prefix.
///
/// The inode number is a caller's choice only because it must be unique within the
/// archive; a single appended member can safely be any non-zero value.
pub fn member(name: &str, size: u64, ino: u32) -> Result<Member, String> {
    if size > MAX_MEMBER {
        return Err(format!(
            "{name} is {size} bytes; a cpio member cannot exceed {MAX_MEMBER} (4 GiB). \
             Serve the image directly and let the loader name it instead."
        ));
    }
    // A leading slash would make it an absolute path inside the archive, which the
    // kernel's unpacker refuses; the members it creates all sit at the root.
    if name.is_empty() || name.starts_with('/') || name.contains('\0') {
        return Err(format!("{name:?} is not a usable member name"));
    }

    let mut prefix = Vec::with_capacity(HEADER + name.len() + 8);
    prefix.extend_from_slice(MAGIC);
    let fields = [
        ino,
        MODE_FILE,
        0, // uid: root, because an initramfs member has no other sensible owner
        0, // gid
        1, // nlink
        0, // mtime: fixed, so the same request twice produces the same bytes
        size as u32,
        0, // devmajor
        0, // devminor
        0, // rdevmajor
        0, // rdevminor
        (name.len() + 1) as u32,
        0, // check: unused in newc, and zero is what everyone writes
    ];
    for field in fields {
        prefix.extend_from_slice(hex8(field).as_bytes());
    }
    prefix.extend_from_slice(name.as_bytes());
    prefix.push(0);
    // The name is padded so that the data begins on a four-byte boundary.
    prefix.resize(align4(prefix.len()), 0);

    Ok(Member {
        prefix,
        padding: align4(size as usize) - size as usize,
    })
}

/// The end-of-archive marker. Every reader stops here, so anything appended after it is
/// a separate archive — which is exactly how concatenation works.
pub fn trailer() -> Vec<u8> {
    let mut end = member("TRAILER!!!", 0, 0)
        .expect("the trailer name is fixed and valid")
        .prefix;
    // Archives are padded to 512 bytes at the end. The kernel does not require it, but
    // every other cpio reader expects it and it costs a few hundred zeros.
    let padded = end.len().div_ceil(512) * 512;
    end.resize(padded, 0);
    end
}

fn align4(n: usize) -> usize {
    n.div_ceil(4) * 4
}

fn hex8(value: u32) -> String {
    format!("{value:08X}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assemble what the listener would stream, so the assertions below are about real
    /// archive bytes rather than about the generator's internals.
    fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, (name, data)) in members.iter().enumerate() {
            let m = member(name, data.len() as u64, i as u32 + 1).expect("describable");
            assert_eq!(
                m.len(data.len() as u64) as usize,
                m.prefix.len() + data.len() + m.padding
            );
            out.extend_from_slice(&m.prefix);
            out.extend_from_slice(data);
            out.resize(out.len() + m.padding, 0);
        }
        out.extend_from_slice(&trailer());
        out
    }

    #[test]
    fn a_member_header_is_the_shape_the_format_specifies() {
        let m = member("proxmox.iso", 0x1234_5678, 1).expect("describable");
        assert_eq!(&m.prefix[..6], MAGIC);
        // Every field is eight upper-case hex digits, so the header is fixed width.
        let fields = std::str::from_utf8(&m.prefix[6..HEADER]).expect("ascii");
        assert_eq!(fields.len(), 104, "13 fields of 8");
        assert!(
            fields
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)),
            "{fields}"
        );
        // c_filesize is the seventh field, and it carries the size we were given.
        assert_eq!(&fields[48..56], "12345678");
        // The name follows, NUL-terminated, and the data begins aligned.
        assert!(m.prefix[HEADER..].starts_with(b"proxmox.iso\0"));
        assert_eq!(m.prefix.len() % 4, 0);
    }

    #[test]
    fn data_and_the_member_after_it_both_start_aligned() {
        // The alignment is the whole reason the padding exists: an unaligned member is
        // the failure that unpacks as garbage rather than as an error.
        for size in [0usize, 1, 2, 3, 4, 5, 1023, 1024, 1025] {
            let m = member("x", size as u64, 1).expect("describable");
            assert_eq!(m.prefix.len() % 4, 0, "size {size}");
            assert_eq!((m.prefix.len() + size + m.padding) % 4, 0, "size {size}");
            assert!(m.padding < 4);
        }
    }

    #[test]
    fn an_archive_reads_back_as_the_members_that_went_in() {
        // A miniature reader, because asserting on bytes proves the generator agrees
        // with itself and nothing more.
        let built = archive(&[("proxmox.iso", b"an image, pretend"), ("second", b"!!")]);

        let mut at = 0usize;
        let mut found: Vec<(String, Vec<u8>)> = Vec::new();
        loop {
            assert_eq!(&built[at..at + 6], MAGIC, "member at {at}");
            let field = |n: usize| -> usize {
                let text = std::str::from_utf8(&built[at + 6 + n * 8..at + 6 + n * 8 + 8]).unwrap();
                usize::from_str_radix(text, 16).unwrap()
            };
            let size = field(6);
            let namesize = field(11);
            let name = String::from_utf8(built[at + HEADER..at + HEADER + namesize - 1].to_vec())
                .expect("utf8");
            if name == "TRAILER!!!" {
                break;
            }
            let data_at = align4(at + HEADER + namesize);
            found.push((name, built[data_at..data_at + size].to_vec()));
            at = align4(data_at + size);
        }

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, "proxmox.iso");
        assert_eq!(found[0].1, b"an image, pretend");
        assert_eq!(found[1].0, "second");
        assert_eq!(found[1].1, b"!!");
    }

    #[test]
    fn an_image_too_large_to_describe_is_refused_by_name() {
        // Eight hex digits cannot hold it, and a header that silently wrapped would
        // produce an initrd the kernel unpacks as garbage.
        let e = member("proxmox.iso", MAX_MEMBER + 1, 1).expect_err("must refuse");
        assert!(e.contains("4 GiB"), "{e}");
        assert!(member("proxmox.iso", MAX_MEMBER, 1).is_ok());
    }

    #[test]
    fn a_name_the_kernel_would_refuse_is_refused_here() {
        assert!(member("/proxmox.iso", 1, 1).is_err(), "absolute");
        assert!(member("", 1, 1).is_err(), "empty");
        assert!(member("pro\0mox", 1, 1).is_err(), "embedded NUL");
    }

    #[test]
    fn the_trailer_ends_the_archive_on_a_block_boundary() {
        let end = trailer();
        assert!(end.starts_with(MAGIC));
        assert!(end[HEADER..].starts_with(b"TRAILER!!!\0"));
        assert_eq!(end.len() % 512, 0);
    }
}
