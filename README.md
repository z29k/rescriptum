<p align="center">
  <img src="https://raw.githubusercontent.com/z29k/rescriptum/main/assets/rescriptum-logo.jpg" width="150" alt="rescriptum — logo" />
</p>

<h1 align="center">rescriptum</h1>

<p align="center">
  <strong>An HTTP server that compiles and renders the configuration files for automated OS installs.</strong>
</p>

<p align="center">
  <a href="https://github.com/z29k/rescriptum/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/z29k/rescriptum/actions/workflows/ci.yml/badge.svg" /></a>
  <a href="https://github.com/z29k/rescriptum/releases"><img alt="Release" src="https://img.shields.io/github/v/release/z29k/rescriptum?color=3da638" /></a>
  <a href="./LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-3da638" /></a>
</p>

<p align="center"><strong>English</strong> · <a href="https://github.com/z29k/rescriptum/blob/main/README.fr.md">Français</a> · <a href="https://z29k.github.io/rescriptum/">📖 Documentation</a> · <a href="https://z29k.github.io/rescriptum/guide/quickstart">Quick start</a> · <a href="https://z29k.github.io/rescriptum/development/">Development</a></p>

---

**Provision a fleet without writing a file per machine - one config, and only the
differences.** It answers any unattended installer: Proxmox, Debian, RHEL, Ubuntu, Flatcar,
SUSE, Windows. It recognises the machine from its MAC address, serial number or hardware
inventory, stacks the layers that apply to it, and returns the result. Whatever a machine is
about to receive, you can read it before you power the machine on.

Installers ask in one of two shapes, and rescriptum answers both, on any path:

- **They POST what they found.** Proxmox VE, since 8.2, sends a JSON inventory — NICs and
  their MAC addresses, disks, DMI — and expects the answer file in the response body.
- **They GET with their identity in the query string.** Kickstart, preseed, Ubuntu
  autoinstall, Ignition, AutoYaST: iPXE substitutes the MAC or the serial into the URL
  before fetching it.

```console
$ RESCRIPTUM_ANSWERS_DIR=/srv/answers rescriptum
2026-08-24T08:43:36Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=files:/srv/answers workers=8 max_conn=2048 timeout=10s
2026-08-24T08:43:37Z 10.0.0.42:51234 POST /answer body=1876 200 format=toml machine=98fa9b50d810 group=rack-a bytes=431
```

- **Any installer that fetches its config.** Proxmox `answer.toml`, Ubuntu autoinstall,
  kickstart, preseed, Ignition, AutoYaST, Windows `unattend.xml`, iPXE scripts. The
  extension is the format, the URL decides which may answer, and the structured ones
  really merge.
- **One small static binary.** No runtime, no interpreter, no container — as happy on a
  512 MB ARM NAS as on a datacenter host absorbing a provisioning burst.
- **Configuration is files.** A directory of documents — greppable, diffable, in git if
  you like. Or SQLite, when tooling manages it rather than a person.
- **Configuration composes.** A rack shares one group file; a machine that differs
  carries only its difference.
- **Built to be leaned on.** Async, bounded concurrency, timeouts on every stage, and
  answers you can validate before they are served.

## Thirty seconds

```console
$ mkdir -p answers/groups
$ cat > answers/groups/rack-a.toml <<'TOML'
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11"]

[global]
keyboard = "fr"
timezone = "Europe/Paris"

[disk-setup]
filesystem = "zfs"
zfs.raid   = "raid1"
TOML

$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum render 98:fa:9b:50:d8:10
# format=toml group=rack-a
[global]
keyboard = "fr"
timezone = "Europe/Paris"
…
```

That is one rack **as Proxmox**. The same directory holds `groups/rack-a.ks` for the RHEL
nodes and `groups/rack-a.preseed` for the Debian ones — same idea, different extension. A
document is keyed by *(machine, format)*, so one machine can be several operating systems
at once and the URL picks between them.

Then point whatever you are installing at **its own URL** — one server answers them all:

| Installing | Point it at | Serves |
|---|---|---|
| Proxmox VE | `--url http://SERVER:8000/proxmox/answer` | `.toml` |
| RHEL · CentOS · Fedora · Alma · Rocky | `inst.ks=http://SERVER:8000/rhel/ks?mac=${net0/mac}` | `.ks` |
| Debian | `url=http://SERVER:8000/debian/preseed?mac=${net0/mac}` | `.preseed` |
| Ubuntu | `ds=nocloud-net;s=http://SERVER:8000/ubuntu/?mac=${net0/mac}` | `.yaml` |
| Flatcar · Fedora CoreOS | `ignition.config.url=http://SERVER:8000/flatcar/config` | `.ign` |
| openSUSE · SLES | `autoyast=http://SERVER:8000/suse/profile` | `.autoyast` |
| Windows | your own tooling, from `http://SERVER:8000/windows/unattend` | `.unattend` |
| anything line-oriented | `http://SERVER:8000/cfg/…`, `/ipxe/…` | `.cfg`, `.ipxe` |

→ [Full quick start](https://z29k.github.io/rescriptum/guide/quickstart) ·
[preparing installer media](https://z29k.github.io/rescriptum/guide/iso)

## What it does

Each of these is a link into the [documentation](https://z29k.github.io/rescriptum/) —
go deep only where you are curious.

- **[Picks the right answer](https://z29k.github.io/rescriptum/guide/answers/selection)** —
  by filename, by a group's member list, or by a `[match]` block claiming a machine for
  what it *is*. Deterministic: naming beats matching, more criteria beats fewer, ties
  break on sorted name.
- **[One document per operating system](https://z29k.github.io/rescriptum/guide/answers/formats)** —
  `98fa9b50d810.toml` is that machine *as Proxmox*, `98fa9b50d810.preseed` the same
  hardware *as Debian*. Both exist at once; the URL chooses.
- **[Answers that compose](https://z29k.github.io/rescriptum/guide/answers/grouping)** —
  group chains via `extends`, machine documents on top. Maps merge, arrays replace, the
  machine always wins.
- **[Templating](https://z29k.github.io/rescriptum/guide/answers/templating)** —
  `fqdn = "node-{{ serial }}.example.com"`, filled from the request. Substitution happens
  on parsed values, so the format's own serializer does the escaping.
- **[Validation](https://z29k.github.io/rescriptum/guide/answers/validating)** —
  `render` prints what a machine would receive, `check` renders everything and calls the
  installer's own validator where one is on PATH.
- **[A SQLite store](https://z29k.github.io/rescriptum/guide/operations/sqlite)** and
  **[an admin API](https://z29k.github.io/rescriptum/guide/operations/admin-api)** —
  for a fleet administered by tooling. Its own listener, and a write that rolls itself
  back rather than leaving the answer set broken.
- **[Request capture](https://z29k.github.io/rescriptum/guide/operations/capture)** —
  record what machines actually send, replay it offline with `render --body`.
- **[Small enough for a NAS](https://z29k.github.io/rescriptum/guide/operations/synology)** —
  builds for ARMv7, aarch64 and x86_64 — static musl, except ARMv7, which targets DSM's own
  glibc because musl 1.2 cannot run on Synology's 3.10 kernels — plus a **DSM 7 package** that creates
  the shared folder, registers the port with the firewall and starts at boot. Everywhere
  else it is a systemd unit or a container.

## Install

Download a binary from the [releases page](https://github.com/z29k/rescriptum/releases) —
`armv7`, `aarch64` and `x86_64` Linux (musl, static), plus macOS — check its SHA-256, and
run it. There is nothing to install.

On a Synology, take the `.spk` instead and use **Package Center → Manual Install**.

```console
$ RESCRIPTUM_ANSWERS_DIR=/srv/answers ./rescriptum
$ curl http://localhost:8000/health
OK
```

→ [Install guide](https://z29k.github.io/rescriptum/guide/install) ·
[Configuration reference](https://z29k.github.io/rescriptum/guide/reference/configuration)

## Repository layout

- **`src/`** — the crate. `main.rs` is a thin binary over `lib.rs`.
- **`examples/`** — a commented, working example of every supported format.
- **`docs/`** — [this documentation](https://z29k.github.io/rescriptum/), in English and
  French, rendered and published by [notabene](https://z29k.github.io/notabene/).
- **`tests/`** — the real binary over a socket and on its command line, plus the
  conformance suite that runs every behaviour against both stores.

## Contributing

```bash
cargo test                                                    # 308 tests
cargo clippy --all-targets --all-features -- -D warnings
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
```

[CONTRIBUTING.md](CONTRIBUTING.md) has the branching model and the conventions;
the [Development](https://z29k.github.io/rescriptum/development/) space is the honest
architecture document — the constraints, the internals, and a
[list of traps](https://z29k.github.io/rescriptum/development/traps) so nobody hits them
twice.

## Licence

[MIT](LICENSE)
