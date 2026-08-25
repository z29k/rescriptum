---
title: Writing answers
description: Selection, formats, groups, templating and validation — everything about the documents rescriptum serves.
sidebar:
  label: Writing answers
  order: 4
  indexLabel: Overview
---

# Writing answers

An **answer** is the document an installer receives: a Proxmox `answer.toml`, an Ubuntu
autoinstall `user-data`, a kickstart, a preseed, an Ignition config, an AutoYaST profile,
a Windows `unattend.xml`. rescriptum's whole job is to pick the right one for the machine
asking and hand it back, assembled from however many layers you wrote.

## The layout

Everything lives in one flat directory. There is no per-OS folder, because storage layout
and URL are deliberately kept apart — see [formats](./formats.md#storage-is-not-the-url).

```
answers/
├── 98fa9b50d810.toml       this machine, as Proxmox
├── 98fa9b50d810.preseed    …and the same machine, as Debian
├── aabbccddeeff.yaml       another machine, Ubuntu
├── default.toml            when nothing else matches
└── groups/
    ├── rack-a.toml         shared by a rack, claims its members
    ├── base.preseed
    └── rhel-compute.ks     claims machines by what they are
```

- **A machine document** is named after the machine — a MAC address, in any separator
  style — and holds that machine's own configuration, or only the part of it that differs
  from its group.
- **A group** lives in `groups/` and is shared. It claims machines by listing them in
  `members`, or by a `match` block tested against the request.
- **`default.<ext>`** answers when nothing else does. One per format: a TOML default must
  not answer a client that asked for kickstart.

## The five things to know

| | |
|---|---|
| **[How an answer is picked](./selection.md)** | By name, by member list, or by what the machine *is*. Naming always wins; among selectors, more criteria wins; ties break on sorted name |
| **[One document per operating system](./formats.md)** | The extension is the format, the endpoint chooses between them, and a machine can exist as several operating systems at once |
| **[Groups and merging](./grouping.md)** | Layers apply lowest to highest and the machine always wins. Maps merge; **arrays replace** |
| **[Templating](./templating.md)** | `{{ serial }}` filled from the request, so one group file covers a rack |
| **[Validating](./validating.md)** | `render` shows what a machine would get; `check` renders everything and reports what breaks |

## Control keys

Four keys steer resolution and are **stripped before the answer is sent**, so the
installer never sees them:

| Key | Does |
|---|---|
| `members` | the machines this group answers for |
| `match` | criteria tested against the request's facts |
| `extends` | the group this document layers on top of |

They travel in whatever the format allows — top-level keys in TOML, YAML and JSON, an
`<answer-meta>` element in XML, `# answer:` directives in kickstart and preseed. The
per-format spelling is in [formats](./formats.md#where-the-control-keys-live).

## Worked examples

The repository's [`examples/`](https://github.com/z29k/rescriptum/tree/main/examples)
directory carries a commented example of **every** supported format, all selected
differently — by hardware, by member list, by filename — and they are exercised by the
test suite:

```console
$ RESCRIPTUM_ANSWERS_DIR=examples rescriptum check
$ RESCRIPTUM_ANSWERS_DIR=examples rescriptum render --query "path=/rhel/ks&serial=7ABC123"
```

It is the only place the formats are shown composing together; start there when you are
unsure what a real file looks like.
