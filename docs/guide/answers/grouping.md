---
title: Groups and merging
description: A rack shares one document; a machine that differs carries only its difference. What merges, what replaces, and why arrays replace.
sidebar:
  label: Grouping
  order: 3
---

# Groups and merging

A rack of machines usually shares everything except its MAC addresses. Writing that out
once per machine is how a fleet's configuration drifts. So answers compose.

```
answers/
├── groups/
│   ├── base/
│   │   └── proxmox.toml       shared by everything
│   └── rack-a/
│       └── proxmox.toml       extends = "base"; members = [ … ]
├── 98-fa-9b-50-d8-10/
│   └── proxmox.toml           one machine's overrides (optional)
└── default/
    └── proxmox.toml           only when nothing else matches
```

## The shared part

```toml
# answers/groups/rack-a/proxmox.toml
members = [
  "98:fa:9b:50:d8:10",
  "98:fa:9b:50:d8:11",
  "98:fa:9b:50:d8:12",
]

[global]
keyboard = "fr"
country  = "fr"
timezone = "Europe/Paris"

[disk-setup]
filesystem = "zfs"
zfs.raid   = "raid1"
disk-list  = ["sda", "sdb"]
```

## The difference

A machine that differs gets a document with **only the difference** in it:

```toml
# answers/98-fa-9b-50-d8-10/proxmox.toml
[global]
fqdn = "node01.example.com"

[disk-setup]
zfs.raid  = "raid10"                       # this one has four disks
disk-list = ["sda", "sdb", "sdc", "sdd"]
```

…and receives the two merged, with its own values winning:

```console
$ rescriptum render 98:fa:9b:50:d8:10
# format=toml machine=98-fa-9b-50-d8-10 group=rack-a

[global]
keyboard = "fr"
country = "fr"
timezone = "Europe/Paris"
fqdn = "node01.example.com"

[disk-setup]
filesystem = "zfs"
zfs.raid = "raid10"
disk-list = ["sda", "sdb", "sdc", "sdd"]
```

## Merge rules

| | |
|---|---|
| **Layers** | group chain first, machine document last — **the machine always wins** |
| **Maps** | merge recursively, including TOML's inline and dotted tables |
| **Other values** | replaced outright by the higher layer |
| **Arrays** | **replace, they do not append** |
| **Text formats** | concatenated in layer order — see [formats](./formats.md#concatenation) |

**Why arrays replace.** Appending is the intuitive choice right until you need to shorten
a list. `disk-list = ["sda", "sdb"]` in a group and `["sda"]` in a machine document has
exactly one sensible meaning — *this one has a single disk* — and appending cannot express
it. The same rule holds in every format, so you never have to remember which one you are
in.

## `extends`

A group may extend another group, giving a chain — what every rack shares in one file,
per-rack differences in another:

```toml
# answers/groups/base/proxmox.toml
[global]
mailto   = "ops@example.com"
timezone = "Europe/Paris"
root-ssh-keys = ["ssh-ed25519 AAAA…REPLACE ops@example.com"]
```

```toml
# answers/groups/rack-a/proxmox.toml
extends = "base"
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11"]

[disk-setup]
filesystem = "zfs"
```

Layers then apply `base` → `rack-a` → machine document.

**`extends` in a machine document overrides membership.** It is the escape hatch for a
machine that needs a group it is not listed in:

```toml
# answers/98-fa-9b-50-d8-99/proxmox.toml
extends = "rack-a"          # even though rack-a does not list this MAC

[global]
fqdn = "spare01.example.com"
```

`extends` resolves **within one format** — layering a preseed onto a TOML base is
meaningless, and the merge would refuse it anyway.

## Only the first matching group applies

If two groups both claim a machine, one of them applies —
[the most specific](./selection.md#when-several-documents-claim-the-same-request), ties
broken on sorted name. Compose with `extends` rather than relying on several groups
matching at once: the order between them would be arbitrary, and an arbitrary order is
how a machine quietly gets the wrong disk layout.

## When a group is broken

Cycles and missing parents are detected **when the store is read**, reported in the log
once, and the broken group is **dropped rather than half-applied**:

```
2026-08-24T08:43:36Z - warning: group "rack-a": extends unknown group "base"
```

One bad group does not stop the other racks from installing. A machine that
*needed* that group gets a loud `500` rather than a half-built answer — serving a
configuration whose base is missing would install the machine half-configured, and nobody
would find out until it was running.

`rescriptum check` reports the same problems, which is a better place to find out than
the log at 3am.

## Grouping is the fast path

Measured at 2,000 machines, 3,000 requests at 100 concurrent:

| Layout | Throughput |
|---|---|
| 2,000 machine documents, no group | 12,132 req/s |
| one group of 2,000 members, no machine documents | **13,036 req/s** |
| 2,000 machine documents plus a group (a merge per request) | 8,816 req/s |

A group with no machine overrides and no placeholders is rendered **once, when the store
is read**, and served afterwards as a prepared string. The common datacenter case parses
nothing per request. Adding a per-machine override buys a merge per request — worth it
where it is needed, and worth avoiding where it is not.

The other half of the same argument is what a *read* costs. The whole store is re-read at
most once a second, and with a directory per identity that read is a `readdir` per machine
on top of the file it already opened — measured at 2,000 machines on an M1 Pro, **28 ms
before the layout changed and 63 ms after**. It is amortised over a second's worth of
requests either way, and the throughput figures above did not move measurably; but a group
that needs no per-machine directory avoids that cost too.

## Next

- [Templating](./templating.md) — `{{ serial }}` removes the remaining reason for a
  directory per machine.
- [Validating](./validating.md) — a merged answer is a document nobody wrote; look at it
  before a rack does.
