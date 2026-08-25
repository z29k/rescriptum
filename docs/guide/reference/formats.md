---
title: Formats and endpoint aliases
description: Which extension is which format, which URL segment asks for it, and the two traps in the alias table.
sidebar:
  label: Formats & aliases
  order: 3
---

# Formats and endpoint aliases

Three tables. The narrative version is in
[one document per operating system](../answers/formats.md).

## Document extensions

The allowlist of extensions rescriptum will pick up from a store. Anything else is
ignored — `txt` is deliberately **not** on the list, so a stray notes file next to your
answers never becomes a candidate.

| Extension | Family | Layering | Content-Type |
|---|---|---|---|
| `toml` | TOML | structural merge | `text/plain; charset=utf-8` |
| `yaml`, `yml` | YAML | structural merge | `text/yaml; charset=utf-8` |
| `json`, `ign` | JSON | structural merge | `application/json` |
| `xml`, `autoyast`, `unattend` | XML | structural merge, by element | `application/xml; charset=utf-8` |
| `ks`, `cfg`, `preseed`, `seed`, `ipxe` | text | concatenation in layer order | `text/plain; charset=utf-8` |

The **family** is what the log line's `format=` field reports, so `ks` and `preseed` both
appear as `format=text`. The **extension** is what an endpoint filters on, and what
`check` needs in order to pick the right validator.

## Endpoint aliases

A path segment naming one of these restricts the answer to documents with the listed
extensions. **Any** segment of the path may name it, so `/rhel/ks`, `/ks` and
`/provision/rhel/node.cfg` all restrict to kickstart.

| Segment | Serves | Typical use |
|---|---|---|
| `proxmox`, `pve`, `toml` | `.toml` | Proxmox VE |
| `debian`, `preseed` | `.preseed`, `.seed` | Debian preseed |
| `rhel`, `centos`, `fedora`, `alma`, `rocky`, `kickstart`, `ks` | `.ks` | kickstart |
| `ubuntu`, `autoinstall`, `cloudinit`, `nocloud`, `yaml`, `yml` | `.yaml`, `.yml` | Ubuntu autoinstall, cloud-init |
| `flatcar`, `coreos`, `ignition`, `ign` | `.ign`, `.json` | Ignition |
| `suse`, `opensuse`, `autoyast` | `.autoyast`, `.xml` | AutoYaST |
| `windows`, `unattend` | `.unattend`, `.xml` | Windows unattend.xml |
| `json` | `.json`, `.ign` | |
| `xml` | `.xml` | |
| `cfg` | `.cfg` | |
| `ipxe` | `.ipxe` | |

**A segment naming none of these constrains nothing**, which is why `/answer` keeps
working exactly as it always has.

### Two traps in this table

- **Filtering is on the extension, not the family.** `.ks` and `.preseed` are both text
  documents; filtering by family would let a preseed answer `/rhel/ks`.
- **`seed` is deliberately not an alias.** `s=http://server/seed/` is an ordinary NoCloud
  seed URL, and it serves YAML. An alias has to be specific enough that nobody reaches it
  by accident. (The `.seed` *extension* still exists, and `/debian/` serves it.)

## Control keys, per format

Stripped before the answer is sent.

| Format | Spelling |
|---|---|
| TOML | top-level `extends = "base"`, `members = […]`, `[match]` table |
| YAML | top-level `extends:`, `members:`, `match:` |
| JSON | top-level `"extends"`, `"members"`, `"match"` |
| XML | `<answer-meta extends="base"><member>…</member><match k="v" /></answer-meta>` |
| Text | `# answer: extends <name>` · `# answer: member a, b` · `# answer: match k=v k2=v2` |

Text directives also accept `//` as the comment marker. `match` takes space-separated
`key=pattern` pairs; `member` a comma-separated list. Ordinary comments in a text document
are **served** — only `# answer:` lines are removed.

## Merge semantics

| | Structural formats | Text formats |
|---|---|---|
| Maps / objects / elements | merge recursively | — |
| Scalars | higher layer replaces | — |
| Arrays / lists | **replace**, never append | — |
| Whole document | — | concatenated in layer order |

XML pairs siblings by element name plus a discriminating attribute — `name`, `id`, `key`,
`alias`, `pass` — and honours `config:type="list"`. Declarations, doctypes, namespaces and
attributes survive a merge; original indentation and comment placement do not.

## Validators `check` can call

| Format | Tool | Invoked as |
|---|---|---|
| `toml` | `proxmox-auto-install-assistant` | `validate-answer <file>` |
| `xml`, `autoyast`, `unattend` | `xmllint` | `--noout <file>` |
| `ks` | `ksvalidator` | `<file>` |
| everything else | — | none exists |

A tool that is not on PATH is reported once as a note, never as a failure.
