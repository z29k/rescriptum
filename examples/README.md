# Example answers

A worked, commented example of **every format rescriptum serves**, all in one directory —
and a directory that really runs:

```bash
RESCRIPTUM_ANSWERS_DIR=examples rescriptum check
RESCRIPTUM_ANSWERS_DIR=examples rescriptum render 98:fa:9b:50:d8:10
RESCRIPTUM_ANSWERS_DIR=examples rescriptum render --query "path=/rhel/ks&serial=7ABC123"
RESCRIPTUM_ANSWERS_DIR=examples rescriptum          # serve them on :8000
```

`check` has to come back clean here. `deploy.sh` runs it before it ships anything, and
these files are the only place the formats are shown composing together — two of them
(`suse-node.autoyast`, `windows-node.unattend`) are what caught a missing doctype and an
unpaired `pass` attribute.

> **These are examples, not configuration.** Every password hash is `REPLACE$ME` and every
> SSH key is `AAAA...REPLACE`. Copy what you need into your own answers directory; do not
> point a provisioning server at this one.

## What is here

| File | Format | Claimed by |
|---|---|---|
| `example.toml` | Proxmox VE | nothing — a commented reference, copy it and rename to a MAC |
| `98fa9b50d810.toml` | Proxmox VE | its filename; overrides `groups/example-rack.toml` |
| `98fa9b50d810.preseed` | Debian | its filename — **the same machine as Debian** |
| `aabbccddeeff.yaml` | Ubuntu autoinstall | its filename |
| `aabbccddeeff.seed` | Debian | its filename — the same machine again, second OS |
| `52-54-00-aa-00-04.yml` | Ubuntu autoinstall | its filename (`.yml` spelling) |
| `52-54-00-aa-00-05.json` | Ignition | its filename (`.json` spelling) |
| `52-54-00-aa-00-06.xml` | AutoYaST | its filename (generic `.xml` spelling) |
| `groups/example-rack.toml` | Proxmox VE | a `members` list |
| `groups/base.preseed` | Debian | a `# answer: member` directive |
| `groups/rhel-compute.ks` | kickstart | `# answer: match serial=7ABC*` |
| `groups/ubuntu-web.yaml` | Ubuntu autoinstall | `match: file=user-data, product=PowerEdge R6*` |
| `groups/ubuntu-meta.yaml` | Ubuntu autoinstall | `match: file=meta-data` — NoCloud's other half |
| `groups/flatcar-node.ign` | Ignition | a `members` list |
| `groups/suse-node.autoyast` | AutoYaST | `<answer-meta><member>` |
| `groups/windows-node.unattend` | Windows unattend | `<match manufacturer="Dell Inc." />` |
| `groups/edge-router.ipxe` | iPXE boot script | `# answer: member` |
| `groups/legacy-node.cfg` | generic line-oriented | `# answer: member` |

Between them they exercise all three ways of claiming a machine — by filename, by member
list, by selector — and both layering strategies: structural merge for TOML, YAML, JSON
and XML; concatenation for kickstart, preseed, `.cfg` and iPXE.

## Three things these files are trying to teach

**A machine's answer is specific to the operating system it is for.** `98fa9b50d810.toml`
and `98fa9b50d810.preseed` are one piece of hardware with two answers, and the URL decides
which one is served. A document is keyed by *(identifier, format)*, not by identifier.

**Some extensions are two spellings of one format.** `.yml`/`.yaml`, `.json`/`.ign`,
`.seed`/`.preseed`, `.xml`/`.autoyast`/`.unattend`. They parse identically, but they are
different keys — a `.yml` machine file will not layer onto a `.yaml` group. Pick one
spelling per fleet.

**`check` renders from an identity alone.** It has no request, so it cannot supply a fact
that only ever arrives with one. That is why the templating in here uses `{{ machine }}`
(bound whenever a machine document matched) rather than `{{ mac }}` — and why the two
groups describe templating in a comment instead of using it. In a line-oriented format the
document is an opaque string, so **a placeholder written inside a comment is still a
placeholder** and still has to resolve.

Verify request-fed templating with a real request instead:

```bash
RESCRIPTUM_ANSWERS_DIR=examples rescriptum render --query "path=/ipxe/boot&mac=52:54:00:aa:00:01"
```

## Not a Cargo examples directory

The name is Cargo's convention for example *binaries*, but nothing here is Rust: Cargo
discovers no targets, `cargo build` is unaffected, and `cargo build --examples` is a no-op.

---

Full documentation: <https://z29k.github.io/rescriptum/guide/answers/>
