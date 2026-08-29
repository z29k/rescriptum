---
title: How an answer is picked
description: Three ways to claim a machine — by name, by member list, by what it is — and the deterministic rule that settles competing claims.
sidebar:
  label: Selection
  order: 1
---

# How an answer is picked

A document does not get *looked up*; it **claims** the request. Three ways to do that,
ordered by how narrowly they target one machine.

## 1. By name

Name a **directory** after the machine's MAC address, and put its documents in it:

```
answers/
├── 98-fa-9b-50-d8-10/
│   └── proxmox.toml
├── aabbccddeeff/
│   └── proxmox.toml
└── default/
    └── proxmox.toml
```

When a request arrives, the server lowercases everything it carries and drops every
non-alphanumeric character, does the same to each directory's name, and serves the first
whose name appears **inside** the request. So `98-fa-9b-50-d8-10`, `98:fa:9b:50:d8:10` and
`98fa9b50d810` all name the same machine — you never have to care which separator style
Proxmox happens to use this version, or how it structures its JSON.

The identity is the **directory** name; the filenames inside it choose nothing, they only
carry the format in their extension.

That normalization is the whole trick, and it is why this survives Proxmox changing its
body format between releases: it is a substring test over the bytes, not a schema.

Nothing prevents naming a directory after a serial number, an asset tag or a hostname
instead. Any string that appears in what the machine sends will do.

## 2. By member list

A group claims a set of machines by listing them:

```toml
# answers/groups/rack-a/proxmox.toml
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11", "98:fa:9b:50:d8:12"]
```

Member strings are normalized exactly like directory names, so separator style does not
matter here either. A listed machine needs no directory of its own unless it has something
to override — see [grouping](./grouping.md).

## 3. By what the machine is

A `match` block claims a machine by its properties rather than its identity:

```toml
# answers/groups/dell-r620/proxmox.toml
[match]
manufacturer = "Dell Inc."
product      = "PowerEdge R620"
serial       = "7ABC*"          # * and ? work
```

**Every criterion must hold** for the group to claim the request. `*` matches any run of
characters and `?` exactly one; both sides are normalized before comparing, so case and
separator style never matter.

A machine document may carry a `match` block too — useful for "whatever machine is
currently in this chassis slot".

## The facts a selector can test

Facts come from three places, deliberately layered from most to least structured.

### Query parameters

`?mac=…&uuid=…&serial=…` — how every installer other than Proxmox identifies itself,
because iPXE substitutes the values into the URL before fetching. Reliable, arbitrary
keys, no guessing.

Three more are synthesized from the URL itself:

| Fact | Is |
|---|---|
| `path` | the whole path, trimmed of slashes — `rhel/ks` |
| `file` | its last segment — `ks`. This is what tells cloud-init's `user-data` from its `meta-data` |
| `segment` | every segment, as separate values — `rhel` *and* `ks` |

### A POSTed JSON body

When the body really is JSON, it is flattened to **both** its full dotted paths and its
bare leaf names:

```json
{ "dmi": { "system": { "serial": "7ABC123" } } }
```

gives both `dmi.system.serial` and plain `serial`. **The leaf form is the point.**
Proxmox's own documentation warns that the contents of `dmi` "might vary wildly,
depending on the system", so a selector saying *"a field called `serial`, wherever it
lives"* survives a reorganisation that a fixed path would not.

Array indices become part of the path but not of the leaf name, so
`network_interfaces.0.mac` is also reachable as plain `mac`.

A body that is not JSON is not an error — it simply contributes nothing but the haystack.

### The raw body

Normalized to lowercase alphanumerics: the substring haystack that makes matching by name
work. Query values and path segments are appended to it too, so a directory named after a
MAC resolves whether that MAC arrived in a POST body or a query string. Without that, a
`GET` — which has no body at all — could never match by name.

## When several documents claim the same request

The rule is fixed, and a test pins it:

1. **Naming a machine always wins.** However many criteria a selector carries, an
   identity match beats it — naming a machine is as specific as anyone can be.
2. **Among selectors, more criteria wins.** Three matching criteria beat two; a more
   deliberate rule is a more specific one.
3. **Ties break on sorted name.** Alphabetically first.

The answer never depends on filesystem order or on the order rows came out of a database.
matchbox, the closest prior art, documents that its own resolution between competing
groups "will not be deterministic". This one is.

**Only the first matching group applies.** Composition is expressed with
[`extends`](./grouping.md#extends), not by merging every group that happens to match —
the order between several matching groups would be arbitrary, and an arbitrary order is
how a machine quietly gets the wrong disk layout.

## And if nothing matches

`default.<ext>` is served, if there is one for the format the endpoint asked for.
Otherwise the answer is **404** — logged as `no answer file applies`.

## Try it before booting anything

`render` resolves exactly as the server would, from facts you supply:

```console
$ rescriptum render 98:fa:9b:50:d8:10                              # by identity
$ rescriptum render --query "serial=7ABC123&mac=98:fa:9b:50:d8:10" # by label
$ rescriptum render --query "path=/rhel/ks&serial=7ABC123"         # including the endpoint
$ rescriptum render --body captured-request.json                   # a real captured body
```

A bare identifier claims nothing about *what kind* of identifier it is — it fills the
haystack and nothing else. That is enough for name matching, but a selector on `serial`
needs `--query "serial=…"` to have anything to test. `check` works the same way, which is
why a template needing a request-only fact is [reported as a
problem](./templating.md#check-and-request-only-facts).

To capture what your machines really send, see
[Capturing requests](../operations/capture.md).
