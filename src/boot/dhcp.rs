//! Generating somebody else's DHCP configuration.
//!
//! **There is no DHCP code in this project** — not a server, not a proxy, not behind a
//! flag. rescriptum deploys into an infrastructure that already has a DHCP server, and
//! pointing that server at a boot server is one of the oldest and most standard things
//! it does. Writing a responder to work around problems that are not DHCP problems is
//! how a provisioning server acquires a reputation for breaking networks.
//!
//! So the handoff is two options on a server that already exists, and our job is to
//! make setting them trivial. **The operator copies rather than composes**, and every
//! line comes from the same table `tftp` serves from — so a snippet naming a loader
//! that is not on disk cannot happen by drift. (It can still happen by nobody running
//! `boot sync`, which is what `boot check` is for. That failure is silent at the ROM,
//! and it is the most common way this goes wrong.)
//!
//! Five details are ours to get right in what we emit, and each is a way this fails
//! quietly on somebody else's network:
//!
//! 1. **Both the BOOTP `file` field and option 67.** Some ROMs read only one, and which
//!    one is not predictable from the vendor.
//! 2. **The option 93 table comes from IANA plus one recorded exception** — see
//!    `loaders`.
//! 3. **A next-server for the `HTTPClient` classes too.** An HTTP-boot deployment that
//!    sets only option 67 leaves `${next-server}` empty inside the loader, and the
//!    bootstrap's primary path with it.
//! 4. **Echo `HTTPClient` back in option 60 for those classes.** UEFI firmware filters
//!    offers on it; a reply carrying only the URL is discarded, silently.
//! 5. **An untagged default at the end.** Every architecture line is tag-matched, so a
//!    ROM that sends no option 93 would match nothing and get no boot file at all. The
//!    only clients that old are BIOS, and a DHCP client that is not netbooting ignores
//!    boot options entirely.

use super::loaders::{self, Transport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Dnsmasq,
    Isc,
    Kea,
    /// Windows Server's `DhcpServer` module. **Not `netsh`**: branching there takes
    /// policies, which `netsh dhcp` predates.
    PowerShell,
    PfSense,
    Mikrotik,
}

impl Format {
    pub fn parse(name: &str) -> Option<Format> {
        Some(match name.to_ascii_lowercase().as_str() {
            "dnsmasq" => Format::Dnsmasq,
            "isc" | "dhcpd" => Format::Isc,
            "kea" => Format::Kea,
            "powershell" | "windows" => Format::PowerShell,
            "pfsense" | "opnsense" => Format::PfSense,
            "mikrotik" | "routeros" => Format::Mikrotik,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Dnsmasq => "dnsmasq",
            Format::Isc => "isc",
            Format::Kea => "kea",
            Format::PowerShell => "powershell",
            Format::PfSense => "pfsense",
            Format::Mikrotik => "mikrotik",
        }
    }

    pub const ALL: &'static [Format] = &[
        Format::Dnsmasq,
        Format::Isc,
        Format::Kea,
        Format::PowerShell,
        Format::PfSense,
        Format::Mikrotik,
    ];
}

pub struct Handoff {
    /// The address a ROM is sent to, as it must appear on that network.
    pub host: String,
    /// The media listener's URL, for the clients that fetch their loader over HTTP.
    pub media: String,
    pub version: &'static str,
    /// One line for a homogeneous fleet, rather than four for a mixed one.
    pub one_loader: bool,
}

/// The vendor-class string firmware announces in option 60, which is the only place the
/// architecture reaches a Windows DHCP policy.
fn vendor_class(arch: u16, transport: Transport) -> String {
    let prefix = match transport {
        Transport::Tftp => "PXEClient",
        Transport::Http => "HTTPClient",
    };
    format!("{prefix}:Arch:{arch:05}")
}

/// A short tag name for a row, for the formats that use one.
fn tag(arch: u16, transport: Transport) -> String {
    let base = match arch {
        0x0000 => "bios",
        0x0007 | 0x0009 | 0x0010 => "efi64",
        0x000b | 0x0013 => "efiarm64",
        _ => "other",
    };
    match transport {
        Transport::Tftp => base.to_string(),
        Transport::Http => format!("http{base}"),
    }
}

/// Every row that actually serves something, which is what a snippet is made of.
fn served() -> Vec<(&'static loaders::Client, &'static str)> {
    loaders::TABLE
        .iter()
        .filter_map(|c| c.loader.map(|l| (c, l)))
        .collect()
}

pub fn snippet(format: Format, handoff: &Handoff) -> String {
    let mut out = String::new();
    // Kea's configuration is JSON, and although its own parser accepts `#` comments
    // locally, upstream says plainly that "most JSON tools detect them as errors" and
    // recommends `user-context` instead. So the provenance goes *inside* the document
    // there, and the output stays pasteable into anything that reads JSON.
    if format != Format::Kea {
        out.push_str(&format!(
            "# rescriptum {} - boot handoff for {}\n\
             # Architecture values are IANA option 93 codes; see docs/guide/boot/dhcp.\n\
             # Generated from the same table the TFTP server serves from.\n",
            handoff.version, handoff.host
        ));
    }

    if handoff.one_loader {
        if format != Format::Kea {
            out.push_str("# --one-loader: a homogeneous BIOS fleet, so no branching at all.\n");
        }
        out.push_str(&one_loader(format, handoff));
        return out;
    }

    out.push_str(&match format {
        Format::Dnsmasq => dnsmasq(handoff),
        Format::Isc => isc(handoff),
        Format::Kea => kea(handoff),
        Format::PowerShell => powershell(handoff),
        Format::PfSense => pfsense(handoff),
        Format::Mikrotik => mikrotik(handoff),
    });
    out
}

fn one_loader(format: Format, handoff: &Handoff) -> String {
    let host = &handoff.host;
    let loader = loaders::FALLBACK;
    match format {
        Format::Dnsmasq => format!("dhcp-boot={loader},,{host}\n"),
        Format::Isc => format!("next-server {host};\nfilename \"{loader}\";\n"),
        Format::Kea => format!(
            "{{\n  \"user-context\": {{ \"comment\": \"rescriptum {} boot handoff\" }},\n  \
             \"next-server\": \"{host}\",\n  \"boot-file-name\": \"{loader}\"\n}}\n",
            handoff.version
        ),
        Format::PowerShell => format!(
            "Set-DhcpServerv4OptionValue -OptionId 66 -Value '{host}'\n\
             Set-DhcpServerv4OptionValue -OptionId 67 -Value '{loader}'\n"
        ),
        Format::PfSense => format!(
            "Services > DHCP Server > Network Booting\n  \
             Next Server: {host}\n  Default BIOS file name: {loader}\n"
        ),
        Format::Mikrotik => format!(
            "/ip dhcp-server network set [find] next-server={host} boot-file-name={loader}\n"
        ),
    }
}

fn dnsmasq(handoff: &Handoff) -> String {
    let host = &handoff.host;
    let media = handoff.media.trim_end_matches('/');
    let mut out = String::new();

    for (client, _) in served() {
        match client.transport {
            // dnsmasq can match option 93 directly, which is the clean way.
            Transport::Tftp => out.push_str(&format!(
                "dhcp-match=set:{},option:client-arch,{}\n",
                tag(client.arch, client.transport),
                client.arch
            )),
            // HTTP-boot clients are told apart by their vendor class, because that is
            // also what has to be echoed back at them.
            Transport::Http => out.push_str(&format!(
                "dhcp-vendorclass=set:{},{}\n",
                tag(client.arch, client.transport),
                vendor_class(client.arch, client.transport)
            )),
        }
    }
    out.push('\n');

    for (client, loader) in served() {
        let tag = tag(client.arch, client.transport);
        match client.transport {
            Transport::Tftp => {
                out.push_str(&format!("dhcp-boot=tag:{tag},{loader},,{host}\n"));
            }
            Transport::Http => {
                // The firmware filters offers on option 60 being HTTPClient. A reply
                // carrying only the URL is discarded, silently, at the firmware.
                out.push_str(&format!(
                    "dhcp-option-force=tag:{tag},60,HTTPClient\n\
                     dhcp-boot=tag:{tag},{media}/boot/{loader},,{host}\n"
                ));
            }
        }
    }

    out.push_str(&format!(
        "\n# A ROM that sends no option 93 matches no tag above and would get nothing.\n\
         # The only clients that old are BIOS, and a DHCP client that is not netbooting\n\
         # ignores boot options entirely.\n\
         dhcp-boot={},,{host}\n",
        loaders::FALLBACK
    ));
    out
}

fn isc(handoff: &Handoff) -> String {
    let host = &handoff.host;
    let media = handoff.media.trim_end_matches('/');
    let mut out = String::from(
        "option arch code 93 = unsigned integer 16;\n\
         option vendor-class code 60 = string;\n\n",
    );
    out.push_str(&format!("next-server {host};\n"));
    // Both the BOOTP `file` field and option 67: some ROMs read only one, and which is
    // not predictable from the vendor. `filename` sets the former; dhcpd copies it into
    // the latter for a client that asked for it.
    out.push_str(&format!("filename \"{}\";\n\n", loaders::FALLBACK));

    let mut first = true;
    for (client, loader) in served() {
        let keyword = if first { "if" } else { "} elsif" };
        first = false;
        match client.transport {
            Transport::Tftp => out.push_str(&format!(
                "{keyword} option arch = {:02x}:{:02x} {{\n  filename \"{loader}\";\n",
                client.arch >> 8,
                client.arch & 0xff
            )),
            Transport::Http => out.push_str(&format!(
                "{keyword} option arch = {:02x}:{:02x} {{\n  \
                 option vendor-class \"HTTPClient\";\n  \
                 filename \"{media}/boot/{loader}\";\n",
                client.arch >> 8,
                client.arch & 0xff
            )),
        }
    }
    if !first {
        out.push_str("}\n");
    }
    out
}

fn kea(handoff: &Handoff) -> String {
    let host = &handoff.host;
    let media = handoff.media.trim_end_matches('/');
    let mut classes: Vec<String> = Vec::new();

    for (client, loader) in served() {
        let name = format!(
            "{}-{:#06x}",
            tag(client.arch, client.transport),
            client.arch
        );
        let file = match client.transport {
            Transport::Tftp => loader.to_string(),
            Transport::Http => format!("{media}/boot/{loader}"),
        };
        let mut body = format!(
            "    {{\n      \"name\": \"{name}\",\n      \
             \"test\": \"option[93].hex == {:#06x}\",\n      \
             \"next-server\": \"{host}\",\n      \
             \"boot-file-name\": \"{file}\"",
            client.arch
        );
        if client.transport == Transport::Http {
            body.push_str(
                ",\n      \"option-data\": [ { \"name\": \"vendor-class-identifier\", \
                 \"data\": \"HTTPClient\" } ]",
            );
        }
        body.push_str("\n    }");
        classes.push(body);
    }

    format!(
        "{{\n  \"Dhcp4\": {{\n    \"user-context\": {{\n      \
         \"comment\": \"rescriptum {} boot handoff for {host}; option 93 values are IANA \
         architecture types, generated from the same table the TFTP server serves from\"\n    \
         }},\n    \"client-classes\": [\n{}\n    ],\n    \
         \"next-server\": \"{host}\",\n    \"boot-file-name\": \"{}\"\n  }}\n}}\n",
        handoff.version,
        classes.join(",\n"),
        loaders::FALLBACK
    )
}

fn powershell(handoff: &Handoff) -> String {
    let host = &handoff.host;
    let media = handoff.media.trim_end_matches('/');
    let mut out = String::from(
        "# Windows Server DHCP. **A policy cannot condition on option 93** — the policy\n\
         # condition types are vendor class, user class, MAC, client id, FQDN and relay\n\
         # information, and the architecture reaches a policy only inside the option 60\n\
         # string. Hence a vendor class per architecture, matched with a trailing\n\
         # wildcard because the real string continues (`PXEClient:Arch:00007:UNDI:003016`).\n\
         # Run per scope: pass -ScopeId, or omit it for the server-level policy.\n\n",
    );

    for (client, loader) in served() {
        let class = vendor_class(client.arch, client.transport);
        let name = format!("rescriptum-{}", class.replace(':', "-"));
        let file = match client.transport {
            Transport::Tftp => loader.to_string(),
            Transport::Http => format!("{media}/boot/{loader}"),
        };
        out.push_str(&format!(
            "Add-DhcpServerv4Class -Name '{name}' -Type Vendor -Data '{class}'\n\
             Add-DhcpServerv4Policy -Name '{name}' -Condition OR -VendorClass EQ,'{class}*'\n\
             Set-DhcpServerv4OptionValue -PolicyName '{name}' -OptionId 66 -Value '{host}'\n\
             Set-DhcpServerv4OptionValue -PolicyName '{name}' -OptionId 67 -Value '{file}'\n"
        ));
        if client.transport == Transport::Http {
            out.push_str(&format!(
                "Set-DhcpServerv4OptionValue -PolicyName '{name}' -OptionId 60 -Value 'HTTPClient'\n"
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "# The untagged default, for a ROM that announces no architecture.\n\
         Set-DhcpServerv4OptionValue -OptionId 66 -Value '{host}'\n\
         Set-DhcpServerv4OptionValue -OptionId 67 -Value '{}'\n",
        loaders::FALLBACK
    ));
    out
}

fn pfsense(handoff: &Handoff) -> String {
    let host = &handoff.host;
    let mut out = String::from(
        "# pfSense and OPNsense configure this through the web interface, so this is\n\
         # what to type rather than a file to paste.\n\n\
         Services > DHCP Server > (interface) > Network Booting\n",
    );
    out.push_str(&format!("  Next Server:            {host}\n"));
    out.push_str(&format!(
        "  Default BIOS file name: {}\n",
        loaders::FALLBACK
    ));
    for (client, loader) in served() {
        if client.transport == Transport::Http {
            continue;
        }
        if client.arch == 0x0007 {
            out.push_str(&format!("  UEFI 64-bit file name:  {loader}\n"));
        }
    }
    out.push_str(
        "\n# Architectures the built-in fields do not cover — ARM64, and HTTP boot —\n\
         # go in Advanced > Additional BOOTP/DHCP Options, or in the ISC snippet:\n\
         #   rescriptum boot dhcp-snippet --format isc\n",
    );
    out
}

fn mikrotik(handoff: &Handoff) -> String {
    let host = &handoff.host;
    let mut out = String::from(
        "# RouterOS. Branching on option 93 needs matchers, which older versions do not\n\
         # have; the single-loader form below works everywhere and is right for a fleet\n\
         # that is all one architecture. For a mixed fleet, run dnsmasq beside it or use\n\
         # a DHCP server that can branch — see the guide.\n\n",
    );
    out.push_str(&format!(
        "/ip dhcp-server network set [find] next-server={host} boot-file-name={}\n",
        loaders::FALLBACK
    ));
    out.push_str("\n# RouterOS 7 with matchers, for a mixed fleet:\n");
    for (client, loader) in served() {
        if client.transport == Transport::Http {
            continue;
        }
        out.push_str(&format!(
            "# /ip dhcp-server matcher add server=[find] code=93 value=0x{:04x} \
             address-pool=static-only boot-file-name={loader}\n",
            client.arch
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handoff() -> Handoff {
        Handoff {
            host: "192.0.2.10".to_string(),
            media: "http://192.0.2.10:8001".to_string(),
            version: "0.3.0",
            one_loader: false,
        }
    }

    #[test]
    fn every_format_names_every_loader_the_table_serves() {
        // **The two cannot drift**: what an operator pastes into their DHCP server and
        // what this one hands out come from one table. A snippet naming a loader that
        // is not there fails silently at the ROM, which is the least diagnosable
        // failure in the whole chain.
        for format in Format::ALL {
            let text = snippet(*format, &handoff());
            for loader in loaders::loaders() {
                // pfSense and Mikrotik are interfaces rather than files, and both say
                // in the output which architectures they cannot express.
                if matches!(format, Format::PfSense | Format::Mikrotik) && loader.contains("arm64")
                {
                    continue;
                }
                assert!(
                    text.contains(loader),
                    "{} does not name {loader}:\n{text}",
                    format.label()
                );
            }
        }
    }

    #[test]
    fn every_format_names_the_server() {
        for format in Format::ALL {
            let text = snippet(*format, &handoff());
            assert!(
                text.contains("192.0.2.10"),
                "{} names no server:\n{text}",
                format.label()
            );
        }
    }

    #[test]
    fn a_rom_that_announces_no_architecture_still_gets_a_loader() {
        // Every architecture line is tag-matched, so without an untagged default a ROM
        // sending no option 93 matches nothing and simply stops.
        for format in [
            Format::Dnsmasq,
            Format::Isc,
            Format::Kea,
            Format::PowerShell,
        ] {
            let text = snippet(format, &handoff());
            assert!(
                text.contains(loaders::FALLBACK),
                "{} has no default:\n{text}",
                format.label()
            );
        }
        // dnsmasq's is the untagged `dhcp-boot` at the end.
        let text = snippet(Format::Dnsmasq, &handoff());
        assert!(
            text.contains(&format!("dhcp-boot={},,192.0.2.10", loaders::FALLBACK)),
            "{text}"
        );
    }

    #[test]
    fn http_boot_clients_are_told_to_expect_an_http_client_offer() {
        // UEFI firmware filters offers on option 60 being HTTPClient. A reply carrying
        // only the URL in option 67 is discarded, silently, at the firmware — which is
        // indistinguishable from no DHCP server at all.
        let text = snippet(Format::Dnsmasq, &handoff());
        assert!(text.contains("60,HTTPClient"), "{text}");

        let text = snippet(Format::PowerShell, &handoff());
        assert!(text.contains("-OptionId 60 -Value 'HTTPClient'"), "{text}");

        let text = snippet(Format::Kea, &handoff());
        assert!(text.contains("\"data\": \"HTTPClient\""), "{text}");
    }

    #[test]
    fn http_boot_clients_are_still_given_a_next_server() {
        // An HTTP-boot deployment names no next-server of its own — option 67 carries a
        // URL there — so the bootstrap's primary path would read an empty
        // `${next-server}` and chain into nowhere.
        let text = snippet(Format::Dnsmasq, &handoff());
        for line in text.lines().filter(|l| l.contains("http://")) {
            assert!(
                line.ends_with(",,192.0.2.10"),
                "an HTTP-boot line must still carry a next-server: {line}"
            );
        }
    }

    #[test]
    fn a_windows_policy_conditions_on_the_vendor_class_not_on_option_93() {
        // A policy cannot condition on option 93 at all: the condition types are vendor
        // class, user class, MAC, client id, FQDN and relay information. The
        // architecture reaches a policy only inside the option 60 string, and the real
        // string continues past the arch — hence the trailing wildcard.
        let text = snippet(Format::PowerShell, &handoff());
        assert!(
            text.contains("-VendorClass EQ,'PXEClient:Arch:00007*'"),
            "{text}"
        );
        assert!(text.contains("Add-DhcpServerv4Class"), "{text}");
        assert!(!text.contains("netsh"), "{text}");
        assert!(text.contains("cannot condition on option 93"), "{text}");
    }

    #[test]
    fn the_vendor_class_strings_are_the_ones_firmware_actually_sends() {
        assert_eq!(
            vendor_class(0x0000, Transport::Tftp),
            "PXEClient:Arch:00000"
        );
        assert_eq!(
            vendor_class(0x0007, Transport::Tftp),
            "PXEClient:Arch:00007"
        );
        assert_eq!(
            vendor_class(0x000b, Transport::Tftp),
            "PXEClient:Arch:00011"
        );
        // 0x0010 is sixteen, and it is an HTTP client rather than a PXE one.
        assert_eq!(
            vendor_class(0x0010, Transport::Http),
            "HTTPClient:Arch:00016"
        );
        assert_eq!(
            vendor_class(0x0013, Transport::Http),
            "HTTPClient:Arch:00019"
        );
    }

    #[test]
    fn the_recorded_exception_gets_its_own_line() {
        // Real x64 firmware announces 0x0007 or 0x0009, so a snippet that covered only
        // the one the registry blesses would leave half a fleet unbootable.
        let text = snippet(Format::Dnsmasq, &handoff());
        assert!(text.contains("option:client-arch,7"), "{text}");
        assert!(text.contains("option:client-arch,9"), "{text}");
    }

    #[test]
    fn one_loader_is_a_single_line_for_a_homogeneous_fleet() {
        let mut handoff = handoff();
        handoff.one_loader = true;
        let text = snippet(Format::Dnsmasq, &handoff);
        assert!(
            text.contains("dhcp-boot=ipxe-undionly.kpxe,,192.0.2.10"),
            "{text}"
        );
        assert!(!text.contains("dhcp-match"), "no branching at all:\n{text}");
    }

    #[test]
    fn a_format_name_nobody_uses_is_refused_rather_than_defaulted() {
        assert_eq!(Format::parse("dnsmasq"), Some(Format::Dnsmasq));
        assert_eq!(Format::parse("DHCPD"), Some(Format::Isc));
        assert_eq!(Format::parse("opnsense"), Some(Format::PfSense));
        // `netsh` is deliberately not an alias: it cannot express this.
        assert_eq!(Format::parse("netsh"), None);
        assert_eq!(Format::parse("bind"), None);
    }

    #[test]
    fn the_kea_snippet_is_valid_json() {
        // It is the one format that is a data document rather than a directive list, so
        // it can be checked here rather than only by the real parser.
        let text = snippet(Format::Kea, &handoff());
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("Kea output must parse as JSON: {e}\n{text}"));
        let classes = parsed["Dhcp4"]["client-classes"]
            .as_array()
            .expect("client-classes");
        assert_eq!(classes.len(), served().len());
    }
}
