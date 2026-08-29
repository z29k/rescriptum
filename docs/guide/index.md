---
title: What rescriptum is
description: A machine describes its hardware; rescriptum composes the answer for it and sends it back. The problem that needs, and the four ideas that solve it.
sidebar:
  label: What rescriptum is
  order: 0
---

# What rescriptum is

*rescriptum* — in Roman law, an authority's **written answer to a question raised by a
particular case**. You set out your situation; you received a document drafted for it.

rescriptum serves the **install config a machine asks for while installing itself**, and
composes it per machine out of the layers you share across a rack. One server answers every
installer you run. Serving a file is easy — deciding which file is the whole job.

## The problem

**Every unattended installer fetches its configuration over HTTP, and every machine needs a
different one.** The URL is baked into the media, so it is identical on every machine —
which means a static file server cannot serve them. Installers ask in one of two shapes.

### They POST what they found

Since Proxmox VE 8.2, an installer prepared with `--fetch-from http` **POSTs a JSON
description of the hardware it found** — NICs and their MAC addresses, disks, DMI — and
expects the answer file back in the response body:

```json
{
  "network_interfaces": [{ "mac": "98:fa:9b:50:d8:10", "link": "up" }],
  "dmi": { "system": { "serial": "7ABC123", "product": "PowerEdge R620" } }
}
```

**The reply depends on the request.** A static file server cannot do that: it has one
answer for one URL, and the URL is baked into the ISO — identical on every machine.

### They GET with their identity in the query string

Everything else has the opposite shape. A kickstart, a preseed, an Ubuntu autoinstall, an
Ignition config, an AutoYaST profile are *fetched*, with the machine's identity in the
query string, because iPXE substitutes it into the URL it was told to fetch:

```
GET /rhel/ks?serial=7ABC123&mac=98:fa:9b:50:d8:10
```

Either way, the answer has to be chosen — and usually assembled — per machine.

## The four ideas

**1. The endpoint declares the format.** A kickstart client wants kickstart and would
choke on TOML. So `/rhel/ks` serves `.ks` documents and nothing else, `/proxmox/answer`
serves `.toml`, `/ubuntu/` serves YAML. The consequence that makes the model click:
a machine's answer is specific to the OS it is for, so `98fa9b50d810/proxmox.toml` is not
"that machine" but *"that machine as Proxmox"* — and `98fa9b50d810/debian.preseed`, in the
same directory, is the same hardware as Debian. Both exist at once.
→ [One document per operating system](./answers/formats.md)

**2. A machine is claimed, not looked up.** Name a directory after the MAC and it wins.
Or list the machine in a group's `members`. Or write a `[match]` block and let the
machine be claimed by what it *is* — a Dell R620 with a serial starting `7ABC`. The
resolution is deterministic: naming beats matching, more criteria beats fewer, ties break
on sorted name.
→ [How an answer is picked](./answers/selection.md)

**3. Answers compose.** A rack of machines shares everything except its MAC addresses.
Put the shared part in a group; a machine that differs gets a document containing **only the
difference**. Structured formats really merge — maps key by key, arrays replaced so a
list can still be shortened. Add `{{ serial }}` placeholders and one group document covers
five hundred machines.
→ [Groups and merging](./answers/grouping.md) · [Templating](./answers/templating.md)

**4. What gets served is reviewable before it is served.** Merging creates a document
nobody ever wrote, and a bad merge surfaces as a failed unattended install at 3am.
`rescriptum render` prints exactly what a given machine would receive; `rescriptum check`
renders everything and reports what breaks, calling the installer's own validator where
one is on PATH.
→ [Validating what will be served](./answers/validating.md)

## What it is not

- **Not a DHCP server, in any form.** Not a responder, not a proxy, not behind a flag.
  Sites that deploy this already run one, and pointing it at a boot server is a solved
  problem with thirty years of tooling.
- **Not a config management system.** It hands over a document at install time and then
  has nothing more to do with the machine — nothing it installs depends on it afterwards.
- **Not a schema validator.** It proves your documents are well-formed and merge cleanly.
  Whether the result is valid *Proxmox* is `proxmox-auto-install-assistant`'s job, and
  `check` will call it when it is installed.

## Two deployment realities

Both are real, and the design has to satisfy both:

- **A Synology DS416j** — ARMv7, 512 MB, DSM 7, no Docker. The original motivation, and
  the reason this is a single static binary with no runtime and no interpreter.
- **A datacenter host** fielding a provisioning burst, with one answer directory per machine.
  The reason it is async, bounds its own concurrency, and caches the directory listing
  instead of walking it per request.

At 2,000 machines a rack served from one group renders **13,000 requests/second** with
nothing parsed per request — grouping is the fast path, not just the tidy one.

## Where to go next

- [Install](./install.md) — get the binary running.
- [Serve your first answer](./quickstart.md) — end to end in five minutes.
- [Preparing installer media](./iso.md) — the URL to bake into each ISO.
- [Writing answers](./answers/index.md) — selection, formats, groups, templating.
- [Running it](./operations/index.md) — deployment, security, storage, troubleshooting.
- [Boot media](./operations/media.md) and [netbooting](./operations/netboot.md) — serve the installer itself, not only its answer.

Working on rescriptum rather than with it? The [Development](../development/index.md)
space is the other half of this site.
