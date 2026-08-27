---
title: Netbooting a machine
description: TFTP, the loader, the menu — the whole chain from power-on to an unattended install, with two options added to a DHCP server you already run.
sidebar:
  label: Netbooting
  order: 9
---

# Netbooting a machine

A machine powers on. Four links later it is installing itself the way somebody decided —
or, if nobody has decided anything about it yet, sitting in a menu where a human can.

```
 power on
    │
(1) ├── DHCP says where to boot from ......... THEIRS. Two options, and we
    │   generate the snippet that sets them.
    ▼
(2) ├── TFTP hands over a loader ............. OURS
    │   arch-matched iPXE, chaining through ${next-server}
    ▼
(3) ├── iPXE asks what to do ................. OURS
    │   known machine  → its own unattended answer
    │   unknown machine → the menu
    ▼
(4) └── the bits arrive ..................... OURS
        kernel, initrd, the image itself — HTTP with ranges
```

**Link 1 is somebody else's and stays that way.** rescriptum speaks no DHCP at all — not
as a server, not as a proxy, not behind a flag. Sites that deploy this already run one,
and pointing it at a boot server is a solved problem with thirty years of tooling.

## Turning it on

```console
$ export RESCRIPTUM_MEDIA_DIR=/srv/media     # the images
$ export RESCRIPTUM_BOOT_DIR=/srv/boot       # the loaders
$ export RESCRIPTUM_PUBLIC_HOST=192.0.2.10   # what generated scripts will name
```

`RESCRIPTUM_BOOT_DIR` says where the loaders are: unset, there is no TFTP listener and
nothing at `/boot/…`. Naming it starts TFTP on `0.0.0.0:69` unless you say otherwise.

Port 69 is privileged, and it is the *only* privileged port this server ever wants —
with no DHCP responder there is nothing after 67 or 4011. Four ways to deal with it, all
portable:

```console
$ export RESCRIPTUM_USER=rescriptum          # start as root, bind, then drop
$ setcap cap_net_bind_service=+ep rescriptum # or grant just that one capability
$ export RESCRIPTUM_TFTP_ADDR=0.0.0.0:6969   # or move it, if their DHCP can say so
$ export RESCRIPTUM_TFTP_ADDR=off            # or have no listener at all
```

**`off` is a value, not an absence** — it is how you say another daemon on this host
hands the loader over while rescriptum serves the rest of the chain. The loaders stay
served over HTTP at `/boot/…` and stay checked by `boot check`; only the listener is gone.
It is a deployment workaround for somebody who wants it, **never how anything here
ships**: rescriptum *is* the TFTP server, and a build that turned it off by default would
have traded away the thing it is for. The [Synology package](./synology.md) binds port 69
with a `setcap`.

**A TFTP port that cannot be bound does not stop the server**, and that is the one place
this project's "a listener that cannot bind is fatal" rule inverts. Port 69 is the only
privileged port in the design, so it is the only bind that can fail for something nobody
configured — a capability an upgrade quietly dropped, most often. Answers are the product;
dying here would fail every install in flight to report that a second port could not be
opened. So it warns, keeps serving, and `boot check` exits non-zero:

```console
$ rescriptum boot check
  BROKEN nothing answers on 0.0.0.0:69 and it cannot be bound either: Permission denied.
  Port 69 is privileged: run as root and set RESCRIPTUM_USER to drop afterwards, or grant
  the binary cap_net_bind_service with setcap — the server still answers and still serves
  media, but a machine sent here by DHCP asks for a loader and gets nothing
```

It asks the port for a real loader rather than trying to bind it, because binding proves
the opposite of what it looks like: a bind that *succeeds* means nothing is listening, and
a bind that fails cannot tell this server apart from another daemon squatting the port.

**Binding happens first and dropping second**, always. The other order works in testing
as root and fails on deployment, at a reboot, which is the one moment nobody is watching.

## Their DHCP server's two lines

```console
$ rescriptum boot dhcp-snippet --format dnsmasq
# rescriptum 0.2.0 - boot handoff for 192.0.2.10
# Architecture values are IANA option 93 codes; see docs/guide/boot/dhcp.
# Generated from the same table the TFTP server serves from.
dhcp-match=set:bios,option:client-arch,0
dhcp-match=set:efi64,option:client-arch,7
dhcp-match=set:efi64,option:client-arch,9
dhcp-match=set:efiarm64,option:client-arch,11
…
```

`--format` covers `dnsmasq`, `isc`, `kea`, `powershell`, `pfsense` and `mikrotik`;
`--one-loader` emits the single-line form for a fleet that is all one architecture.

**The snippet and the TFTP server are generated from one table**, so what you paste in
and what the server hands out cannot drift apart. What they *can* do is name a loader
nobody has downloaded yet, and that fails silently at the ROM — the machine asks, gets
nothing, and stops with no message on any console. One command catches it:

```console
$ rescriptum boot check
checking boot assets in /srv/boot
  ok   ipxe-arm64.efi (1.0M)
  MISSING ipxe-undionly.kpxe — every machine the snippet sends here will ask for it,
  get nothing, and stop
```

Its exit code is a contract, like `check`'s. Put it in the same place.

### Four details the generated snippet gets right

Each is a way this fails quietly on somebody else's network, and none is obvious:

- **Both the BOOTP `file` field and option 67.** Some ROMs read only one, and which is
  not predictable from the vendor.
- **An untagged default at the end.** Every architecture line is tag-matched, so a ROM
  that sends no option 93 would match nothing and get no boot file at all.
- **`HTTPClient` echoed back in option 60** for UEFI HTTP Boot clients. The firmware
  *filters offers* on it: a reply carrying only the URL is discarded, silently, which is
  indistinguishable from having no DHCP server.
- **A next-server for those clients too**, even though they fetch over HTTP. Without one
  the loader's embedded script reads an empty `${next-server}` and chains into nowhere.

::: tip Windows Server
A DHCP policy **cannot condition on option 93** — the condition types are vendor class,
user class, MAC, client id, FQDN and relay information. The architecture reaches a policy
only inside the option 60 string, so the generated PowerShell defines vendor classes on
`PXEClient:Arch:00007*` and hangs the policies off those. Same outcome, different
mechanism, and it is exactly the sort of thing that gets half-remembered.
:::

## The loader

TFTP hands over **one file**, and the rule is written into the code:

> **TFTP hands over the loader. Everything after that is HTTP.**

At 1468 bytes a round-trip, TFTP moves about 1.4 MB/s on a millisecond of latency. The
loader is a megabyte — two seconds. A 1.5 GB image would be the better part of twenty
minutes, against fifteen seconds over HTTP on the same wire.

Which loader depends on what the firmware announced:

| Option 93 | Client | Served |
|---|---|---|
| `0x0000` | BIOS PXE | `ipxe-undionly.kpxe` |
| `0x0007`, `0x0009` | UEFI x86-64 | `ipxe-x86_64.efi`, plus `-snp` / `-snponly` |
| `0x000b` | UEFI ARM64 | `ipxe-arm64.efi` |
| `0x0010`, `0x0013` | UEFI HTTP Boot | the same files, over HTTP, no TFTP at all |
| everything else | 32-bit UEFI, EBC, U-Boot | refused, with the reason |

`0x0009` needs a word. RFC 4578 defined it as "EFI x86-64"; IANA's registry, rewritten by
RFC 5970, lists it as "EBC". Real x64 firmware sends either, so both map to x64 — a table
generated from the registry alone would hand half a fleet nothing.

`snponly` exists because the plain UEFI build cannot always see the NIC. All the variants
are served and the table picks; this is precisely the knowledge an operator should not
have to acquire.

::: warning No release publishes the loaders yet
`packaging/ipxe/build.sh` builds all eight from a pinned upstream commit and has been run,
but nothing is published as a release artifact — so for now you build them yourself:

```console
$ packaging/ipxe/build.sh --out /srv/boot
$ rescriptum boot check
```

A loader from elsewhere works too, provided it chains to *this* server rather than to the
internet — see below for why a stock one does not.
:::

## How iPXE ends up talking to *us*

The question nobody expects to have to answer. Whatever delivers the loader:

- A plain `undionly.kpxe` from ipxe.org does DHCP, is told to load iPXE, and **loads
  itself forever** — iPXE's documented chainloading loop.
- A stock netboot.xyz binary has an embedded script that goes straight to the **public**
  `boot.netboot.xyz`. No loop, but your menu and your answers are never consulted.

The loaders rescriptum ships carry a three-line script that chains through
`${next-server}` — the value option 66 already set, which is how the loader arrived in
the first place. That makes one generic build work in every deployment, with no second
condition in a configuration file somebody else owns.

The script chains to **port 8001**, and that is a contract rather than a preference: it
is baked into the loader before any deployment exists and can read no configuration.
Moving `RESCRIPTUM_MEDIA_ADDR` is allowed and `boot check` warns about it.

## What a machine sees

Stage two puts the machine's identity in the query string, which is the one thing DHCP
cannot do — a DHCP option cannot carry `${net0/mac}`:

```console
$ rescriptum boot bootstrap
#!ipxe
chain http://192.0.2.10:8000/ipxe/boot?mac=${netX/mac}&uuid=${uuid}\
&serial=${serial:uristring}&asset=${asset:uristring}\
…
|| chain http://192.0.2.10:8001/ipxe/menu
```

Two details in there are load-bearing. **`netX`, not `net0`** — `net0` is merely the
first interface, so a server booting from its second port would identify as its unused
first. And **`:uristring`** on every SMBIOS string, because `${manufacturer}` expands to
`Dell Inc.` with the space and iPXE percent-encodes nothing on its own.

That final `||` is the whole of "a menu is the default answer": a machine something
claims gets its own unattended answer, and a machine nothing claims falls through to the
menu. It is `default.toml`'s job description word for word, applied to a different
format.

## The menu

```console
$ rescriptum boot menu
```

Rendered from the catalogue **at request time**, not kept in sync as a file: drop an ISO
in the media directory and it is in the menu on the next fetch.

- **`Boot from the local disk` is first, and the timeout falls through to it.** A machine
  that PXE-boots by accident, and that nothing claims, ends up on its own disk after
  fifteen seconds. It never sits waiting for a human who is not coming, and it never
  installs anything. Combined with the rule that an unclaimed machine gets a menu rather
  than an install, **the worst case of being wrong about which machines reach this server
  is a few seconds added to a boot.**
- Entries are **gated on the client's architecture**, so an ARM64 image is not offered to
  an x86 machine — that is an entry that boots the wrong kernel.
- An image no probe could place is still offered, as a CD.
- The diagnostics entries — a shell, `netinfo`, and one that boots a *different*
  rescriptum — are what every boot server ends up needing. The last is how you test a
  candidate server on site, from the running one, without touching DHCP or the loaders.

`RESCRIPTUM_BOOT_TIMEOUT_SECS` (default 15) sets the wait, and `RESCRIPTUM_BOOT_TITLE`
the title bar. The logo is fetched with `console --picture … ||`, which **tolerates its
own failure**: a serial console over IPMI has no framebuffer, and that is how half of all
datacenter installs are watched.

## What breaks when this server is down

Worth stating plainly, because "a boot server" sounds load-bearing and is not:

| | rescriptum down |
|---|---|
| DHCP addressing, DNS, routing | **unaffected** — it speaks none of those protocols |
| Machines already installed and running | **unaffected** |
| Machines rebooting | **unaffected** — they boot from disk |
| A machine that PXE-boots by accident | falls to its next boot device, as it would anyway |
| Starting a *new* installation | stops |

**Nothing rescriptum installs depends on rescriptum afterwards.** The answer endpoint is
consulted during an install and never again.

## Security

Boot traffic is unauthenticated, and necessarily — a PXE ROM has no credentials, the same
necessity that already governs the answer endpoint. So the controls are structural, and
one of them can say *not you*:

```console
$ export RESCRIPTUM_BOOT_ALLOW=10.0.0.0/8    # shared by TFTP and media
```

UDP is forgeable and TFTP is UDP, so the server **never answers a broadcast or multicast
destination** — amplification hygiene rather than politeness — caps concurrent transfers
in total and per peer, and logs every one. It is read-only: a write request is refused as
an access violation, because writing a loader over unauthenticated UDP would be a way to
change what every machine on the segment boots.

**A boot VLAN is the honest recommendation** and the one that actually works. See
[Security](./security.md).

::: tip Secure Boot
Our loaders are unsigned, and shim only loads what its distro's vendor key signed — so
serving a shim beside an unsigned iPXE is not Secure Boot support, it is a boot that
stops at a signature error. What does work: turn Secure Boot off, enrol a MOK, or let
firmware PXE-boot the target distro's *own* signed shim and GRUB, served from the media
listener like any other file. We sign nothing and strip nothing, and nothing here weakens
a machine that has Secure Boot on.
:::

## When their DHCP genuinely cannot be touched

None of this costs a line of code, and all three work:

- **UEFI HTTP Boot with a URL typed into firmware setup.** Modern server firmware lets
  you enter a boot URL directly. The chain then starts on the media listener with no DHCP
  option involved at all.
- **iPXE from IPMI virtual media, a USB stick, or the NIC's own ROM**, carrying this
  server's address. A one-megabyte image, mounted once per machine.
- **dnsmasq in proxy-DHCP mode**, for a site that truly has a DHCP server it cannot edit.
  It exists, it is mature, it is three lines of configuration, and it is not ours to
  rewrite. Naming it is the honest answer.
