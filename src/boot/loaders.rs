//! What firmware announces in DHCP option 93, and which loader it gets.
//!
//! A small alias table in the spirit of `format::endpoint_formats` — auditable, not
//! clever. **One table, two consumers**: the TFTP server serves from it and
//! `boot dhcp-snippet` generates from it, so the configuration an operator pastes into
//! their DHCP server and the files this one actually hands out cannot drift apart. A
//! snippet naming a loader that is not on disk fails *silently, at the ROM*, which is
//! the most common way this goes wrong and the least diagnosable.
//!
//! What it serves is **our own branded iPXE build, always.** A stock netboot.xyz binary
//! embeds a script that chains to the public `boot.netboot.xyz`, which is exactly the
//! failure the whole entry-point design exists to prevent. netboot.xyz stays what it is
//! here: menus served over HTTP, never the loader TFTP hands out.
//!
//! The values come from IANA's "Processor Architecture Types" registry **plus one
//! recorded exception**: RFC 4578 defined `0x0009` as "EFI x86-64", and the registry —
//! rewritten by RFC 5970 — lists it as "EBC". Real x64 firmware announces `0x0007` or
//! `0x0009`, so both map to x64, exactly as every deployed dhcpd example does. A table
//! generated from the registry alone would hand x64 firmware nothing.

/// How a client reaches us at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// The loader arrives over TFTP, named by DHCP options 66/67.
    Tftp,
    /// Firmware fetches it over HTTP itself — option 60 `HTTPClient` plus a URL in 67.
    /// The shortest chain there is, and it skips TFTP entirely.
    Http,
}

/// One row: what the firmware said it is, and what it gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Client {
    /// The option 93 value, as the ROM sends it.
    pub arch: u16,
    pub label: &'static str,
    /// The loader, or `None` when we have nothing to give it.
    pub loader: Option<&'static str>,
    pub transport: Transport,
    /// Said out loud when `loader` is `None`. A refusal that names its reason is a
    /// bug report; a silent one is a machine that hangs at power-on.
    pub refusal: &'static str,
}

const UNBUILT: &str = "not in the build matrix — no machine that needs it has been seen, and a loader \
     nobody has booted is worse than an honest refusal";

/// Every value the registry defines that a PXE ROM might plausibly send.
pub const TABLE: &[Client] = &[
    Client {
        arch: 0x0000,
        label: "BIOS PXE",
        // UNDI is upstream's own chainloading recommendation. `ipxe.kpxe` (native
        // drivers) is the one to reach for when a NIC's UNDI stack misbehaves.
        loader: Some("ipxe-undionly.kpxe"),
        transport: Transport::Tftp,
        refusal: "",
    },
    Client {
        arch: 0x0006,
        label: "UEFI IA32",
        loader: None,
        transport: Transport::Tftp,
        refusal: UNBUILT,
    },
    Client {
        arch: 0x0007,
        label: "UEFI x86-64",
        loader: Some("ipxe-x86_64.efi"),
        transport: Transport::Tftp,
        refusal: "",
    },
    Client {
        // The recorded exception: IANA calls this EBC, RFC 4578 called it x86-64, and
        // real firmware sends it meaning x86-64.
        arch: 0x0009,
        label: "UEFI x86-64 (announced as EBC; see RFC 4578)",
        loader: Some("ipxe-x86_64.efi"),
        transport: Transport::Tftp,
        refusal: "",
    },
    Client {
        arch: 0x000a,
        label: "UEFI ARM32",
        loader: None,
        transport: Transport::Tftp,
        refusal: UNBUILT,
    },
    Client {
        arch: 0x000b,
        label: "UEFI ARM64",
        loader: Some("ipxe-arm64.efi"),
        transport: Transport::Tftp,
        refusal: "",
    },
    Client {
        arch: 0x000f,
        label: "UEFI IA32, HTTP boot",
        loader: None,
        transport: Transport::Http,
        // HTTP transport does not add a loader the build matrix lacks.
        refusal: UNBUILT,
    },
    Client {
        arch: 0x0010,
        label: "UEFI x86-64, HTTP boot",
        loader: Some("ipxe-x86_64.efi"),
        transport: Transport::Http,
        refusal: "",
    },
    Client {
        arch: 0x0011,
        label: "EBC, HTTP boot",
        loader: None,
        transport: Transport::Http,
        refusal: UNBUILT,
    },
    Client {
        arch: 0x0012,
        label: "UEFI ARM32, HTTP boot",
        loader: None,
        transport: Transport::Http,
        refusal: UNBUILT,
    },
    Client {
        arch: 0x0013,
        label: "UEFI ARM64, HTTP boot",
        loader: Some("ipxe-arm64.efi"),
        transport: Transport::Http,
        refusal: "",
    },
    Client {
        arch: 0x0014,
        label: "BIOS, HTTP boot",
        loader: None,
        transport: Transport::Http,
        refusal: UNBUILT,
    },
    Client {
        arch: 0x0015,
        label: "ARM32 U-Boot",
        loader: None,
        transport: Transport::Tftp,
        refusal: UNBUILT,
    },
    Client {
        arch: 0x0016,
        label: "ARM64 U-Boot",
        loader: None,
        transport: Transport::Tftp,
        refusal: UNBUILT,
    },
];

pub fn for_arch(arch: u16) -> Option<&'static Client> {
    TABLE.iter().find(|c| c.arch == arch)
}

/// Every distinct loader filename the table can hand out — what `boot check` looks for
/// on disk, and what a release has to publish.
pub fn loaders() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = TABLE.iter().filter_map(|c| c.loader).collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The loader a ROM that announces nothing at all should get.
///
/// Every architecture line in a generated snippet is tag-matched, so a client matching
/// no tag would get no boot file and simply stop. The only clients that old are BIOS,
/// and a DHCP client that is not netbooting ignores boot options entirely — so an
/// untagged default costs nothing and covers the case.
pub const FALLBACK: &str = "ipxe-undionly.kpxe";

/// `snp` uses UEFI's Simple Network Protocol; `snponly` uses the firmware's own NIC
/// driver and is the one to reach for when the plain build cannot see the network.
///
/// **Serve all of them and let the table pick** — this is precisely the knowledge an
/// operator should not have to acquire. The variants are alternatives for one row
/// rather than rows of their own, because option 93 cannot tell them apart: nothing in
/// the protocol says "my UNDI stack is broken".
pub fn variants(loader: &str) -> Vec<String> {
    match loader.strip_suffix(".efi") {
        Some(stem) => vec![
            loader.to_string(),
            format!("{stem}-snp.efi"),
            format!("{stem}-snponly.efi"),
        ],
        None => vec![loader.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_architectures_that_matter_get_a_loader() {
        for (arch, expected) in [
            (0x0000u16, "ipxe-undionly.kpxe"),
            (0x0007, "ipxe-x86_64.efi"),
            (0x0009, "ipxe-x86_64.efi"),
            (0x000b, "ipxe-arm64.efi"),
        ] {
            let client = for_arch(arch).unwrap_or_else(|| panic!("{arch:#06x} is in the table"));
            assert_eq!(client.loader, Some(expected), "{arch:#06x}");
        }
    }

    #[test]
    fn the_registry_and_the_rfc_disagree_about_0x0009_and_both_map_to_x64() {
        // IANA (via RFC 5970) calls it EBC; RFC 4578 defined it as EFI x86-64; real x64
        // firmware sends either. A table generated from the registry alone would hand
        // x64 firmware nothing at all.
        assert_eq!(
            for_arch(0x0007).and_then(|c| c.loader),
            Some("ipxe-x86_64.efi")
        );
        assert_eq!(
            for_arch(0x0009).and_then(|c| c.loader),
            Some("ipxe-x86_64.efi")
        );
        assert!(for_arch(0x0009).unwrap().label.contains("RFC 4578"));
    }

    #[test]
    fn http_boot_is_a_transport_rather_than_a_second_architecture() {
        // 0x0010 and 0x0013 are the same silicon as 0x0007 and 0x000b, fetching over
        // HTTP instead of TFTP — so they get the same loader and skip TFTP entirely.
        for (tftp, http) in [(0x0007u16, 0x0010u16), (0x000b, 0x0013)] {
            let a = for_arch(tftp).expect("in the table");
            let b = for_arch(http).expect("in the table");
            assert_eq!(a.loader, b.loader, "{tftp:#06x} vs {http:#06x}");
            assert_eq!(a.transport, Transport::Tftp);
            assert_eq!(b.transport, Transport::Http);
        }
    }

    #[test]
    fn a_thirty_two_bit_client_is_refused_over_both_transports() {
        // An earlier draft of the table served 0x000f — IA32 over HTTP — while refusing
        // 0x0006, the same architecture over TFTP, with no IA32 loader anywhere in the
        // build matrix. HTTP transport does not conjure a loader.
        for arch in [0x0006u16, 0x000f, 0x000a, 0x0012] {
            let client = for_arch(arch).unwrap_or_else(|| panic!("{arch:#06x} is in the table"));
            assert_eq!(client.loader, None, "{arch:#06x}");
            assert!(!client.refusal.is_empty(), "{arch:#06x} must say why");
        }
    }

    #[test]
    fn every_refusal_names_a_reason() {
        // A refusal that names its reason is a bug report; a silent one is a machine
        // that hangs at power-on with nothing on the console.
        for client in TABLE {
            assert_eq!(
                client.loader.is_none(),
                !client.refusal.is_empty(),
                "{:#06x} must either serve something or say why not",
                client.arch
            );
        }
    }

    #[test]
    fn the_loaders_a_release_must_publish_are_derivable_from_the_table() {
        // `boot check` looks for exactly these on disk, and a release publishes exactly
        // these. Both read the table rather than a second list that could drift.
        assert_eq!(
            loaders(),
            vec!["ipxe-arm64.efi", "ipxe-undionly.kpxe", "ipxe-x86_64.efi"]
        );
    }

    #[test]
    fn an_efi_loader_has_snp_variants_and_a_bios_one_does_not() {
        assert_eq!(
            variants("ipxe-x86_64.efi"),
            vec![
                "ipxe-x86_64.efi",
                "ipxe-x86_64-snp.efi",
                "ipxe-x86_64-snponly.efi"
            ]
        );
        assert_eq!(variants("ipxe-undionly.kpxe"), vec!["ipxe-undionly.kpxe"]);
    }

    #[test]
    fn an_architecture_nobody_registered_is_simply_unknown() {
        assert!(for_arch(0x00ff).is_none());
        // And the fallback is what a ROM announcing nothing gets.
        assert_eq!(FALLBACK, "ipxe-undionly.kpxe");
        assert!(loaders().contains(&FALLBACK));
    }
}
