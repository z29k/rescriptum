---
title: Formats and merging
description: One Doc type over five parsers, the merge rules, the XML tree, and why substitution happens on parsed values.
sidebar:
  label: Formats
  order: 5
---

# Formats and merging

`src/format/mod.rs` gives every document format one interface, so `select.rs` never has to
know which one it is holding.

```rust
enum Inner {
    Toml(toml_edit::DocumentMut),
    Yaml(serde_yaml_ng::Value),
    Json(serde_json::Value),
    Xml(xml::Document),
    Text(String),
}
```

`Doc` wraps it and offers `parse`, `merge`, `render`, `control`, `strip_control`,
`substitute` and `has_placeholders`. Adding a format means adding a variant and filling in
those seven — nothing above this module changes.

## `Kind`

`Kind::for_extension` is a deliberate **allowlist**. `txt` is not on it, so a stray notes
file next to the answers never becomes a candidate.

`Kind` is the *family*; the *extension* is kept separately, because they are not the same
thing:

- **`Kind`** decides how to parse, how to merge, and the `Content-Type`.
- **The extension** decides whether an endpoint may be answered, and which validator
  `check` calls. `ks` and `preseed` are both `Kind::Text` but do not share a validator —
  which is why `Resolution` carries `format_name` alongside `format`.

Filtering on the family instead of the extension would let a preseed answer `/rhel/ks`.

## `endpoint_formats`

A small alias table mapping a URL segment to the extensions it accepts. Two traps live in
it, both already paid for:

- **Filter on the extension, not the `Kind`** — as above.
- **An alias must be specific enough that nobody reaches it by accident.** `seed` was
  removed: `s=http://server/seed/` is an ordinary NoCloud seed URL, and it serves YAML.

A segment naming no alias constrains nothing, so `/answer` keeps working.

## Merge rules

| | |
|---|---|
| Maps / objects | merge recursively |
| Any other value | replaced outright by the higher layer |
| Arrays | **replace, they do not append** |
| `Kind::Text` | concatenation in layer order |

Arrays replace because appending would make a list impossible to shorten from a higher
layer, and *"this node has two disks, not four"* has to be expressible. The rule is the
same in every format so you never have to remember which one you are in.

`merge.rs` holds the TOML case, and uses `as_table_like` so `[table]` and
`{ inline = "table" }` merge with each other — a group can use one style and a machine the
other without surprises.

The text case is honest about being concatenation rather than pretending otherwise:
whether that amounts to an override is the target format's business (preseed's last answer
wins; kickstart's does not always).

## The XML tree

`format/xml.rs` is a small hand-built tree over `quick-xml`, because none of the
general-purpose crates preserve what an answer document needs preserved.

**Pairing.** Children are paired by element name **plus a discriminating attribute**:

```rust
const DISCRIMINATORS: [&str; 5] = ["name", "id", "key", "alias", "pass"];
```

That is what makes `<component name="Microsoft-Windows-Shell-Setup">` and
`<settings pass="specialize">` mergeable: overriding one pass leaves the others alone.

> **Repeated siblings are not always a list.** Treating them as one replaced every
> `<component>` in an unattend.xml with the one the overlay happened to mention. If they
> carry a discriminating attribute they are a **keyed collection**. AutoYaST's
> `config:type="list"` is honoured for the genuine list case.

**Fidelity.** Declarations, doctypes, namespaces and attributes survive a merge. Original
indentation and comment placement do **not** — the output is re-rendered, not patched.

> **quick-xml emits entity references as their own events.** Ignoring them welds the
> surrounding text fragments together: `1 &lt; 2 &amp; 3` came back as `123`. Numeric
> entities are resolved; unknown ones are refused rather than silently dropped.

It understands no schema. `check` calls `xmllint` where it is installed, and that is the
extent of the guarantee.

## Control keys

```rust
pub const CONTROL_KEYS: [&str; 3] = ["extends", "members", "match"];
pub const XML_CONTROL_ELEMENT: &str = "answer-meta";
pub const TEXT_DIRECTIVE: &str = "answer:";
```

They travel in whatever the format allows — native top-level keys in the structured
formats, an `<answer-meta>` element in XML, `# answer:` (or `// answer:`) directives in
text — and `strip_control()` removes all of them before the answer is sent.

`Control` is the parsed form: `extends: Option<String>`, `members: Vec<String>`,
`matchers: BTreeMap<String, String>`.

## Templating

Two rules, both load-bearing:

**Substitution happens on parsed string values, never on raw document text.** The value
goes into the document's own data model and the format's serializer writes it out, so the
serializer does the escaping. A value containing a quote cannot break the TOML it lands
in; one containing `<` cannot break the XML. A test feeds `a"b'c<d>e&f` into all four
structured formats and **reparses the output**.

**A missing fact is an error, never an empty string.** Serving `node-.example.com`
installs a machine with a broken hostname and nobody notices until later. Control
characters are refused for the same class of reason — a newline in a kickstart value
injects a directive into a file the installer executes.

`Group::has_placeholders` is why a group with no template costs no parsing per request:
the string prepared at load is served as-is.

## The worked examples are part of the design

[`examples/`](https://github.com/z29k/rescriptum/tree/main/examples) carries a commented
example of **all thirteen extensions the allowlist names**, and

```bash
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
```

exercises them all. **Keep it that way.** They are the only place the formats are shown
composing together, and two of them — `suse-node.autoyast` and `windows-node.unattend` —
are what caught the missing doctype and the unpaired `pass`.
