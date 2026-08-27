//! Boot media: the images machines install from, and the bits that carry them.
//!
//! The answer engine tells a machine *what* to install. This half is *where the
//! installer itself comes from* — the kernel, the initrd and the image, served over
//! HTTP from a catalogue discovered the same way answers are.
//!
//! Nothing here decides anything about answers, and `select.rs` knows nothing about
//! this. The one seam between them is `stanza`: a generator that writes an `.ipxe`
//! answer document naming a catalogue entry, which then goes through the existing
//! selection, layering and templating unchanged. The server does not become clever
//! about booting; it gains a generator.

pub mod catalog;
pub mod cpio;
pub mod iso;
pub mod media;
pub mod probe;
pub mod sha256;
pub mod stanza;
