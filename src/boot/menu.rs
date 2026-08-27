//! The two scripts the server writes for iPXE: the bootstrap, and the menu.
//!
//! ## Why a bootstrap exists at all
//!
//! DHCP hands iPXE a URL, and **a DHCP option cannot carry `${net0/mac}`**. So the URL
//! DHCP names arrives with no query string — no MAC, no serial, no UUID. A `GET` has no
//! body either, so the haystack would be empty and every machine would match nothing
//! but the default. The whole selection engine would go dark at exactly the moment it
//! matters.
//!
//! Stage two is therefore one fixed `chain` that stage three is not. It is served by
//! the media listener rather than from the answer set, because **it has to work when
//! the answer set is empty** — which is the state every new install starts in.
//!
//! ## A menu is the default answer
//!
//! The bootstrap's `||` is the whole of it: a machine something claims gets its own
//! unattended answer, and a machine nothing claims falls through to the menu. That is
//! `default.toml`'s job description word for word, applied to a different format, and
//! it needs no new concept in `select.rs`.
//!
//! ## What iPXE's own parser allows here
//!
//! Both read out of the source rather than remembered:
//!
//! - **`;` separates commands only as a whole whitespace-delimited token**
//!   (`split_command` in `core/exec.c`), so a `;` inside an argument is safe.
//! - **A trailing `\` continues a line**, which is how a long URL stays readable.
//!
//! And two things that are ours to get right: **`${version}` is iPXE's own version**,
//! not ours, so everything the server knows is rendered as a literal; and the text
//! stays **ASCII**, because a BIOS text console is not UTF-8.

use super::catalog::Listing;
use super::probe::{Arch, Family};
use super::stanza::{self, Endpoints};

/// The three-line stage two, baked into no loader and configurable by nobody.
///
/// Two expansions carry a correction each. **`netX`, not `net0`**: `net0` is merely the
/// first interface, so a server booting from its second port would identify as its
/// unused first — `netX` is iPXE's virtual scope for the device that actually booted.
/// And **`:uristring` on every SMBIOS string**: `${manufacturer}` expands to
/// `Dell Inc.`, space included, and iPXE percent-encodes nothing on plain expansion, so
/// a space in a request line is a broken fetch.
pub fn bootstrap(endpoints: &Endpoints) -> String {
    let answer = endpoints.answer.trim_end_matches('/');
    let media = endpoints.media.trim_end_matches('/');
    format!(
        "#!ipxe\n\
         # Stage two. DHCP cannot carry a MAC, so this is what puts one in the query\n\
         # string — without it every machine would match nothing but the default.\n\
         chain {answer}/ipxe/boot?mac=${{netX/mac}}&uuid=${{uuid}}\\\n\
         &serial=${{serial:uristring}}&asset=${{asset:uristring}}\\\n\
         &manufacturer=${{manufacturer:uristring}}&product=${{product:uristring}}\\\n\
         &platform=${{platform}}&arch=${{buildarch}} \\\n\
         || chain {media}/ipxe/menu\n"
    )
}

/// The catalogue, rendered as an iPXE menu.
///
/// **Generated at request time, not built into a file.** netboot.xyz's menus are static
/// templates rendered by Ansible at build time; ours are a rendering pass over facts the
/// server already holds, so dropping an ISO in the media directory puts it in the menu
/// on the next fetch. The catalogue is the single source of truth — the same instinct as
/// answers being discovered rather than registered.
pub fn menu(listing: &Listing, endpoints: &Endpoints, style: &Style) -> String {
    let media = endpoints.media.trim_end_matches('/');
    let mut out = String::from("#!ipxe\n");

    // Rows 2 and 3 of the branding are a chain rather than alternatives: `console
    // --picture … ||` **tolerates its own failure**, so a client with no framebuffer —
    // a serial console over IPMI, which is how half of all datacenter installs are
    // actually watched — simply keeps the text console and gets the colours instead.
    // Write it as a chain and there is nothing to detect.
    out.push_str(&format!(
        "console --picture {media}/boot/logo.png --left 0 --right 0 --keep ||\n"
    ));
    out.push_str("colour --rgb 0x1c1b19 0 ||\n");
    out.push_str("colour --rgb 0xc8a15a 3 ||\n");
    out.push_str("cpair --foreground 3 1 ||\n");
    out.push('\n');

    out.push_str(&format!("menu {}\n", ascii(&style.title)));

    // `item local` first, and the timeout falls through to it. **A machine that
    // PXE-boots by accident, and that nothing claims, ends up on its own disk** — it
    // does not sit at a menu forever waiting for a human who is not coming, and it
    // never installs anything. The worst case of being wrong about which machines
    // reach us is a few seconds added to a boot.
    out.push_str("item --gap Default:\n");
    out.push_str("item local              Boot from the local disk\n");

    let bootable: Vec<_> = listing.entries.iter().filter(|e| e.bootable()).collect();
    if !bootable.is_empty() {
        out.push_str("item --gap Install:\n");
        for entry in &bootable {
            let line = format!(
                "item {:<20} {:<28} ({})\n",
                ascii(&entry.id),
                truncate(&entry.describe(), 28),
                entry.family().label()
            );
            out.push_str(&gate(entry.arch(), &line));
        }
    }

    // An image no probe placed cannot produce a stanza, but it can still be booted as a
    // CD — which is a normal thing to want from a live tool.
    let opaque: Vec<_> = listing.entries.iter().filter(|e| !e.bootable()).collect();
    if !opaque.is_empty() {
        out.push_str("item --gap Tools:\n");
        for entry in &opaque {
            out.push_str(&format!(
                "item {:<20} {:<28} (boot as CD)\n",
                ascii(&entry.id),
                truncate(&entry.describe(), 28)
            ));
        }
    }

    out.push_str("item --gap Diagnostics:\n");
    out.push_str("item shell              iPXE shell\n");
    out.push_str("item netinfo            Network card information\n");
    out.push_str("item retry              Ask this server again\n");
    out.push_str("item endpoints          Boot from another rescriptum\n");
    out.push_str("item reboot             Reboot\n");

    // The timeout is rendered **in milliseconds**, because that is what `choose`
    // counts. A seconds value passed through unconverted is a menu that flashes past
    // before a human has read its title — hence the `_SECS` suffix on the variable and
    // exactly one conversion, here.
    out.push_str(&format!(
        "choose --timeout {} --default local target || goto local\n",
        style.timeout_millis
    ));
    out.push_str("goto ${target} ||\n");
    out.push_str("goto local\n\n");

    // One label per entry. `sanboot` is the generic fallback for an image no probe
    // could place: how far it reaches on real firmware is a bench question, so the
    // entry tolerates its own failure and returns to the menu rather than hanging.
    for entry in &bootable {
        out.push_str(&format!(":{}\n", ascii(&entry.id)));
        match stanza::ipxe(entry, endpoints) {
            Ok(script) => {
                for line in script.lines().skip(1) {
                    if line.starts_with('#') {
                        continue;
                    }
                    out.push_str(line);
                    out.push('\n');
                }
            }
            // Unreachable for a bootable entry, but a menu that silently omitted a
            // label would `goto` into nothing.
            Err(why) => {
                out.push_str(&format!("echo {}\n", ascii(&why)));
                out.push_str("goto start\n");
            }
        }
        out.push_str("goto start\n\n");
    }
    for entry in &opaque {
        out.push_str(&format!(":{}\n", ascii(&entry.id)));
        out.push_str(&format!("sanboot {media}/{}/iso ||\n", ascii(&entry.id)));
        out.push_str("goto start\n\n");
    }

    out.push_str(":local\n");
    out.push_str("echo Booting from the local disk\n");
    // `exit` hands control back to the firmware, which moves to its next boot device.
    // `sanboot --no-describe --drive 0x80` is the BIOS-only version and fails on UEFI,
    // so this is the one that works on both.
    out.push_str("exit 0\n\n");

    out.push_str(":shell\n");
    out.push_str("echo Type exit to come back to the menu\n");
    out.push_str("shell ||\n");
    out.push_str("goto start\n\n");

    out.push_str(":netinfo\n");
    out.push_str("ifstat ||\n");
    out.push_str("echo MAC ${netX/mac}  IP ${netX/ip}  next-server ${next-server}\n");
    out.push_str("prompt Press any key to return\n");
    out.push_str("goto start\n\n");

    out.push_str(":retry\n");
    out.push_str(&format!("chain {media}/ipxe/bootstrap ||\n"));
    out.push_str("goto start\n\n");

    // The one borrow that is a feature rather than a pattern: `read` a URL, `chain` it.
    // It is how a candidate server is tested **on site, from the running one, without
    // touching DHCP or the loaders** — netboot.xyz runs its whole staged release
    // process through exactly this entry.
    out.push_str(":endpoints\n");
    out.push_str("echo Boot from another rescriptum, to try one before moving DHCP.\n");
    out.push_str(&format!("set endpoint {media}\n"));
    out.push_str("read endpoint ||\n");
    out.push_str("chain ${endpoint}/ipxe/bootstrap ||\n");
    out.push_str("goto start\n\n");

    out.push_str(":reboot\n");
    out.push_str("reboot\n\n");

    // `goto start` above needs somewhere to land, and re-fetching the menu is the
    // honest way back: the catalogue may have changed since it was rendered.
    out.push_str(":start\n");
    out.push_str(&format!("chain {media}/ipxe/menu ||\n"));
    out.push_str("goto local\n");

    out
}

/// What the menu is called and how long it waits.
pub struct Style {
    pub title: String,
    /// **Milliseconds**, converted once, by the caller that owns the seconds.
    pub timeout_millis: u64,
}

impl Style {
    /// The title a site has not overridden. `${next-server}` stays a variable because it
    /// is genuinely client-side; the version is a literal, because `${version}` in an
    /// iPXE script is *iPXE's* version and the title would advertise iPXE, not us.
    pub fn default_title() -> String {
        format!(
            "rescriptum {} - ${{next-server}}",
            env!("CARGO_PKG_VERSION")
        )
    }
}

/// Wrap a line in a client-side architecture guard, netboot.xyz's `menu_*` trick fed by
/// the catalogue instead of by Ansible. **An ARM64 image offered to an x86 client is a
/// menu entry that boots the wrong kernel**, and one menu has to serve every client.
fn gate(arch: Option<Arch>, line: &str) -> String {
    match arch {
        Some(arch) => format!(
            "iseq ${{buildarch}} {} && {}||\n",
            arch.buildarch(),
            line.trim_end()
        ),
        // An image whose architecture nobody could establish is offered to everybody:
        // hiding it would be a guess in the more damaging direction.
        None => line.to_string(),
    }
}

/// A BIOS text console is not UTF-8, and a menu title full of replacement characters is
/// worse than a plain one. Anything outside printable ASCII becomes a space.
fn ascii(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect()
}

fn truncate(text: &str, width: usize) -> String {
    let text = ascii(text);
    if text.len() <= width {
        return text;
    }
    format!("{}...", &text[..width.saturating_sub(3)])
}

/// The families whose entries the menu can render, for `boot check` to report.
pub fn describable() -> Vec<Family> {
    vec![
        Family::Proxmox,
        Family::Debian,
        Family::Ubuntu,
        Family::Rhel,
        Family::Suse,
        Family::CoreOs,
    ]
}

#[cfg(test)]
mod tests {
    use super::super::catalog::Entry;
    use super::super::probe::Probed;
    use super::*;
    use std::path::PathBuf;

    fn endpoints() -> Endpoints {
        Endpoints {
            media: "http://192.0.2.10:8001".to_string(),
            answer: "http://192.0.2.10:8000".to_string(),
        }
    }

    fn style() -> Style {
        Style {
            title: Style::default_title(),
            timeout_millis: 15000,
        }
    }

    fn entry(id: &str, family: Option<Family>, arch: Option<Arch>) -> Entry {
        Entry {
            id: id.to_string(),
            path: PathBuf::from(format!("/srv/media/{id}.iso")),
            size: 1024,
            digest: None,
            probed: Probed {
                family,
                version: Some(format!("{id} 1.0")),
                arch,
                kernel: family.map(|_| "/kernel".to_string()),
                initrd: family.map(|_| "/initrd".to_string()),
                external: false,
                zstd_initrd: false,
            },
            beside: None,
        }
    }

    /// An entry carrying a version string a vendor might really have written.
    fn described(id: &str, version: &str) -> Entry {
        let mut e = entry(id, Some(Family::Debian), None);
        e.probed.version = Some(version.to_string());
        e
    }

    fn listing(entries: Vec<Entry>) -> Listing {
        Listing {
            entries,
            problems: Vec::new(),
        }
    }

    // ---- the bootstrap ----------------------------------------------------

    #[test]
    fn the_bootstrap_puts_the_machines_identity_in_the_query_string() {
        // Without this the haystack is empty for every GET and the selection engine
        // goes dark at exactly the moment it matters.
        let script = bootstrap(&endpoints());
        assert!(script.starts_with("#!ipxe\n"));
        assert!(script.contains("mac=${netX/mac}"), "{script}");
        assert!(script.contains("uuid=${uuid}"), "{script}");
        assert!(script.contains("arch=${buildarch}"), "{script}");
    }

    #[test]
    fn the_bootstrap_names_the_booting_nic_rather_than_the_first_one() {
        // `net0` is merely the first interface. A server that PXE-boots from its second
        // port would identify as its unused first, and install the wrong machine.
        let script = bootstrap(&endpoints());
        assert!(script.contains("${netX/mac}"), "{script}");
        assert!(!script.contains("${net0/"), "{script}");
    }

    #[test]
    fn every_smbios_string_is_percent_encoded_at_expansion() {
        // `${manufacturer}` is `Dell Inc.` — space included — and iPXE encodes nothing
        // on plain expansion, so a space in the request line is a broken fetch.
        let script = bootstrap(&endpoints());
        for field in ["serial", "asset", "manufacturer", "product"] {
            assert!(
                script.contains(&format!("{field}=${{{field}:uristring}}")),
                "{field} must be uristring: {script}"
            );
        }
        // These cannot carry a reserved character, so they stay plain.
        assert!(script.contains("platform=${platform}"), "{script}");
    }

    #[test]
    fn an_unclaimed_machine_falls_through_to_the_menu() {
        // **A menu is what a machine gets when nobody has decided anything about it
        // yet** — `default.toml`'s job description, applied to a different format, and
        // implemented as one `||` rather than as a new concept in select.rs.
        let script = bootstrap(&endpoints());
        assert!(
            script.contains("|| chain http://192.0.2.10:8001/ipxe/menu"),
            "{script}"
        );
    }

    // ---- the menu ---------------------------------------------------------

    #[test]
    fn the_menu_renders_the_catalogue() {
        let script = menu(
            &listing(vec![
                entry("pve-8.4", Some(Family::Proxmox), Some(Arch::X86_64)),
                entry("ubuntu-24.04", Some(Family::Ubuntu), Some(Arch::X86_64)),
            ]),
            &endpoints(),
            &style(),
        );
        assert!(script.starts_with("#!ipxe\n"));
        assert!(script.contains("item pve-8.4"), "{script}");
        assert!(script.contains("item ubuntu-24.04"), "{script}");
        // And each has a label to `goto`, carrying its family's own stanza.
        assert!(script.contains(":pve-8.4\n"), "{script}");
        assert!(script.contains("proxmox-start-auto-installer"), "{script}");
    }

    #[test]
    fn local_boot_is_first_and_is_what_the_timeout_falls_through_to() {
        // **The safety behaviour that must not be lost.** A machine that PXE-boots by
        // accident, and that nothing claims, ends up on its own disk rather than
        // sitting at a menu forever waiting for a human who is not coming.
        let script = menu(&listing(vec![]), &endpoints(), &style());
        let items: Vec<&str> = script
            .lines()
            .filter(|l| l.starts_with("item ") && !l.starts_with("item --gap"))
            .collect();
        assert!(items[0].starts_with("item local"), "{items:?}");
        assert!(
            script.contains("choose --timeout 15000 --default local target || goto local"),
            "{script}"
        );
    }

    #[test]
    fn the_timeout_is_rendered_in_the_unit_choose_actually_counts() {
        // `choose --timeout` is milliseconds. A seconds value passed through
        // unconverted is a menu that flashes past before a human has read its title.
        let script = menu(
            &listing(vec![]),
            &endpoints(),
            &Style {
                title: "t".to_string(),
                timeout_millis: 30_000,
            },
        );
        assert!(script.contains("--timeout 30000"), "{script}");
    }

    #[test]
    fn an_architecture_specific_entry_is_gated_client_side() {
        // One menu serves every client, so an ARM64 image must not be offered to an x86
        // one — that is a menu entry that boots the wrong kernel.
        let script = menu(
            &listing(vec![
                entry("arm-image", Some(Family::Debian), Some(Arch::Arm64)),
                entry("x86-image", Some(Family::Debian), Some(Arch::X86_64)),
            ]),
            &endpoints(),
            &style(),
        );
        assert!(
            script.contains("iseq ${buildarch} arm64 && item arm-image"),
            "{script}"
        );
        assert!(
            script.contains("iseq ${buildarch} x86_64 && item x86-image"),
            "{script}"
        );
    }

    #[test]
    fn an_image_of_unknown_architecture_is_offered_to_everybody() {
        // Hiding it would be a guess in the more damaging direction: an entry nobody
        // can see is an image nobody can boot.
        let script = menu(
            &listing(vec![entry("mystery", Some(Family::Debian), None)]),
            &endpoints(),
            &style(),
        );
        assert!(script.contains("item mystery"), "{script}");
        assert!(!script.contains("iseq ${buildarch}"), "{script}");
    }

    #[test]
    fn an_image_no_probe_placed_is_offered_as_a_cd() {
        // Not describable is not the same as not usable.
        let script = menu(
            &listing(vec![entry("gparted", None, None)]),
            &endpoints(),
            &style(),
        );
        assert!(script.contains("item gparted"), "{script}");
        assert!(script.contains("boot as CD"), "{script}");
        assert!(
            script.contains("sanboot http://192.0.2.10:8001/gparted/iso ||"),
            "{script}"
        );
    }

    #[test]
    fn the_title_is_ours_and_the_version_is_a_literal() {
        // `${version}` in an iPXE script is *iPXE's* version — the title would have
        // advertised iPXE, not us.
        let script = menu(&listing(vec![]), &endpoints(), &style());
        assert!(script.contains(&format!("menu rescriptum {}", env!("CARGO_PKG_VERSION"))));
        assert!(!script.contains("${version}"), "{script}");
        // `${next-server}` stays a variable, because it is genuinely client-side.
        assert!(script.contains("${next-server}"), "{script}");
    }

    #[test]
    fn the_text_stays_ascii_because_a_bios_console_is_not_utf8() {
        let script = menu(
            // The realistic vector is vendor text read out of an image — a volume
            // identifier or a `/.disk/info` line can hold anything. An id cannot:
            // `valid_id` constrains it at the catalogue boundary. Both are filtered
            // anyway, so the menu does not rest on a guarantee made elsewhere.
            &listing(vec![described("live-cd", "Ubuntu 24.04 « Naïve Numbat »")]),
            &endpoints(),
            &Style {
                title: "rescriptum — naïve".to_string(),
                timeout_millis: 1000,
            },
        );
        assert!(script.is_ascii(), "a BIOS text console cannot render this");
    }

    #[test]
    fn the_logo_tolerates_its_own_failure() {
        // A serial console over IPMI has no framebuffer, and that is how half of all
        // datacenter installs are watched. Written as a chain, there is nothing to
        // detect: the picture fails, the text console stays, the colours still apply.
        let script = menu(&listing(vec![]), &endpoints(), &style());
        let line = script
            .lines()
            .find(|l| l.starts_with("console "))
            .expect("a console line");
        assert!(line.ends_with("||"), "{line}");
    }

    #[test]
    fn every_menu_target_has_a_label_to_land_on() {
        // A `goto` into nothing is a menu that hangs on a keypress, and it would only
        // show up on the machine.
        let script = menu(
            &listing(vec![
                entry("pve-8.4", Some(Family::Proxmox), Some(Arch::X86_64)),
                entry("gparted", None, None),
            ]),
            &endpoints(),
            &style(),
        );
        let labels: Vec<&str> = script.lines().filter_map(|l| l.strip_prefix(':')).collect();
        for line in script.lines() {
            let Some(rest) = line.strip_prefix("item ") else {
                continue;
            };
            if rest.starts_with("--gap") {
                continue;
            }
            let target = rest.split_whitespace().next().unwrap_or_default();
            assert!(labels.contains(&target), "no :{target} label in\n{script}");
        }
        // Including the one every other label returns to.
        assert!(labels.contains(&"start"), "{script}");
    }

    #[test]
    fn no_line_carries_a_bare_semicolon_token() {
        // `;` separates commands only as a whole whitespace-delimited token, so one
        // that appeared alone would split a line into two commands.
        let script = menu(
            &listing(vec![entry(
                "ubuntu",
                Some(Family::Ubuntu),
                Some(Arch::X86_64),
            )]),
            &endpoints(),
            &style(),
        );
        for line in script.lines() {
            assert!(
                !line.split_whitespace().any(|t| t == ";"),
                "a bare `;` would split this: {line}"
            );
        }
        // And the NoCloud argument, which contains one, is left alone.
        assert!(script.contains("ds=nocloud-net;s="), "{script}");
    }
}
