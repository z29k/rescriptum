---
title: One document per operating system
description: The extension is the format, the endpoint chooses between them, and the same machine can exist as Proxmox and as Debian at once.
sidebar:
  label: Formats
  order: 2
---

# One document per operating system

An installer fetching a URL expects one particular thing back. A kickstart client wants
kickstart and would choke on TOML. That is the protocol, not a convention anyone chose.
So:

- **the endpoint declares the format** — `/rhel/ks` asks for kickstart;
- **the document carries it as its extension** — `rhel-compute.ks`;
- **only documents of that format may answer.**

## The consequence that makes it click

**A machine's answer is specific to the operating system it is for.** So this is not one
machine and two files:

```
answers/
├── 98fa9b50d810.toml       "that machine, as Proxmox"
└── 98fa9b50d810.preseed    "that machine, as Debian"
```

It is one piece of hardware with two answers, and both can exist at once. Which one a
request receives depends on the URL it arrived on — `/proxmox/answer` gets the TOML,
`/debian/preseed` gets the preseed. Neither is more "the" answer than the other.

Internally this is why a document is keyed by **(identifier, format)** rather than by
identifier alone.

## Storage is not the URL

Everything lives in one flat directory, and that is deliberate. **Directories and
database rows are a lookup space** — they must stay free to be reorganised. **A URL is a
public contract baked into an ISO** — it must not move because someone renamed a folder.
An earlier design made the directory name *be* the URL segment and was discarded for
exactly that reason.

Which alias serves which extension is in the
[format reference](../reference/formats.md); how to pick one for your media is in
[preparing installer media](../iso.md).

## The formats

| Extension | For | Layering |
|---|---|---|
| `toml` | Proxmox VE | structural merge |
| `yaml`, `yml` | Ubuntu autoinstall, cloud-init | structural merge |
| `json`, `ign` | Ignition, Flatcar, Fedora CoreOS | structural merge |
| `xml`, `autoyast`, `unattend` | AutoYaST, Windows unattend.xml | structural merge, by element |
| `ks` | kickstart — RHEL, CentOS, Fedora, Alma, Rocky | concatenation |
| `preseed`, `seed` | Debian preseed | concatenation |
| `cfg`, `ipxe` | boot scripts and other line-oriented config | concatenation |

The allowlist is deliberate: `txt` is **not** on it, so a stray notes file next to your
answers never becomes a candidate.

`.autoyast` and `.unattend` are XML under a name that says which one it is, so a store
holding both a SUSE profile and a Windows unattend can keep them apart. Plain `.xml`
still answers either, which is fine right up until you have both.

## Structural merge

For `toml`, `yaml`, `json` and `xml`, layering is a real merge:

- **Maps merge key by key**, recursively — including TOML's inline and dotted tables.
- **Any other value is replaced** outright by the higher layer.
- **Arrays replace, they do not append.** Appending would make a list impossible to
  shorten from a higher layer, and "this node has two disks, not four" has to be
  expressible.

The details, with examples, are in [grouping](./grouping.md#merge-rules).

## Concatenation

For `ks`, `preseed`, `cfg`, `seed` and `ipxe`, layering is **concatenation in layer
order**, and the module says so rather than pretending otherwise. A directive in a later
layer *follows* an earlier one rather than removing it.

Whether that amounts to an override is the target format's business: preseed's last
answer wins, kickstart's does not always. **Render the result and read it** before
trusting a rack to it.

One thing worth knowing before you write an essay at the top of a kickstart: **ordinary
comments are served**. Only `# answer:` directive lines are stripped. That is fine —
kickstart and preseed both allow comments — but the installer will see everything else.

## XML

XML pairs siblings by element name **plus a discriminating attribute** — `name`, `id`,
`key`, `alias` or `pass`. That is what makes

```xml
<settings pass="specialize">
  <component name="Microsoft-Windows-Shell-Setup" …>
```

mergeable: overriding one `pass` leaves the others alone, and overriding one `component`
does not replace every other component in the file. Repeated siblings **without** a
discriminating attribute are treated as a list, and AutoYaST's `config:type="list"` is
honoured.

What survives a merge: the `<?xml?>` declaration, the `<!DOCTYPE>`, namespaces, and
attributes. What does **not**: the original indentation and comment placement — the
output is re-rendered, not patched.

It understands no schema. Render and check before trusting a rack to it.

## Where the control keys live

The [control keys](./index.md#control-keys) travel in whatever each format allows, and
are stripped before the answer is sent.

**TOML**

```toml
extends = "base"
members = ["98:fa:9b:50:d8:10"]

[match]
product = "PowerEdge R6*"
```

**YAML / JSON** — the same three as top-level keys:

```yaml
extends: base
members: ["98:fa:9b:50:d8:10"]
match:
  file: "user-data"
  product: "PowerEdge R6*"
```

**XML** — an `<answer-meta>` element, with `extends` as an attribute on it:

```xml
<answer-meta extends="base">
  <member>52:54:00:11:22:33</member>
  <match manufacturer="Dell Inc." product="PowerEdge R6*" />
</answer-meta>
```

**Kickstart, preseed, and anything line-oriented** — `# answer:` directives (`//` works
too, for formats that comment that way):

```
# answer: extends base
# answer: member 00:11:22:33:44:55, 00:11:22:33:44:56
# answer: match serial=7ABC* product=PowerEdge*
```

`match` takes space-separated `key=pattern` pairs, `member` a comma-separated list.

## One answer, one format

**Every layer of one answer must be the same format.** A YAML machine document over a
TOML group is refused, not half-served, and `extends` resolves within one format for the
same reason — layering a preseed onto a TOML base is meaningless.

Grouping is otherwise untouched by any of this: a rack shares one group *per format*, and
a machine that exists as two operating systems joins two of them.

`default` follows the same rule — `default.toml` answers a request that asked for TOML,
and never one that asked for kickstart.
