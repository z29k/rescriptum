---
title: Selection internals
description: Normalization, facts, scoring, the listing cache — the module with the most behaviour per line.
sidebar:
  label: Selection
  order: 4
---

# Selection internals

`src/select.rs` and `src/facts.rs` hold the behaviour that matters. Both are pure logic
over data handed to them, and both are heavily unit-tested — 27 and 22 tests
respectively.

## Normalization

```rust
pub fn normalize(input: &[u8]) -> String   // lowercase ASCII alphanumerics, everything else dropped
```

It takes **bytes, not `&str`**, on purpose: a request body is arbitrary bytes and need not
be valid UTF-8. Filtering to ASCII alphanumerics sidesteps the question entirely — no
validation, no lossy conversion, no failure mode.

This is what makes matching indifferent to separator style and to how Proxmox structures
its JSON this version. It is a substring test over bytes, not a schema.

> **`normalize_pattern` is the other one.** Ordinary normalization strips `*` and `?`
> along with the rest of the punctuation, which turns every glob into a literal —
> quietly. Selector patterns must go through `normalize_pattern`, which keeps them.

## Facts

`Facts` is a map of label → values, plus the haystack. Three sources, layered from most to
least structured:

**Query parameters** — hand-rolled parsing with percent-decoding, rather than pulling in a
URL crate for twenty lines of work. Values also go **into the haystack**, so a document
named after a MAC resolves whether that MAC arrived in a POST body or a query string.
Without that, a `GET` — which has no body at all — could never match by name.

**The path** contributes three synthesized labels:

| Label | From |
|---|---|
| `path` | the whole path, trimmed of slashes |
| `file` | its last segment |
| `segment` | every segment, as separate values |

`file` is not decoration: cloud-init's NoCloud datasource fetches `user-data` **and**
`meta-data` from one URL and skips the datasource entirely if either is missing, so the
same server has to answer them differently. Path segments feed the haystack too, because
NoCloud can expand `__dmi.chassis-serial-number__` into the URL.

**The JSON body**, flattened by `flatten()` into both full dotted paths and bare leaf
names. Array indices become part of the path but not of the leaf name, so
`network_interfaces.0.mac` is also reachable as plain `mac`.

### The departure from "do not parse the JSON"

The original rule was that the request body is never parsed as JSON. That has been
relaxed, deliberately and narrowly:

```rust
if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
    flatten(&value, &mut String::new(), &mut facts);
}
```

Untyped, opportunistic, and non-fatal — a body that is not JSON simply contributes the
haystack and nothing more. No struct is derived, so no assumption about Proxmox's schema
is baked into a type.

**The leaf-name form is why this exists.** Proxmox's own documentation warns that the
contents of `dmi` "might vary wildly, depending on the system". A serial cannot be reached
any other way, because the URL baked into an ISO is the same for every machine. A selector
saying *"a field called `serial`, wherever it lives"* survives a reorganisation that a
fixed path would not.

## Scoring

```rust
const IDENTITY_SCORE: u32 = 1_000;

fn score(control: &Control, identity: &[String], facts: &Facts) -> Option<u32> {
    if identity.iter().any(|n| !n.is_empty() && facts.haystack().contains(n)) {
        return Some(IDENTITY_SCORE);      // naming a machine is as specific as it gets
    }
    if control.matchers.is_empty() { return None; }
    control.matchers.iter()
        .all(|(k, p)| facts.matches(k, p))
        .then_some(control.matchers.len() as u32)
}
```

- `identity` is the normalized stem for a machine document, and the normalized `members`
  for a group.
- **All** matchers must hold; the score is how many there are.
- `IDENTITY_SCORE` is 1000 rather than `u32::MAX` so that "an identity match beats any
  selector" stays readable, and a selector with a thousand criteria remains a theoretical
  problem rather than a subtle one.

Ties break on **sorted name**, alphabetically first:

```rust
.max_by(|(a, ca), (b, cb)| a.cmp(b).then_with(|| cb.id.cmp(&ca.id)))
```

The reversed inner comparison is what makes `max_by` prefer the *smaller* name. matchbox,
the closest prior art, documents that its own resolution between competing groups "will
not be deterministic". This one is, and a test pins it.

## Format filtering

```rust
fn wanted(facts: &Facts) -> Option<&'static [&'static str]>   // from the `segment` facts
fn acceptable(wanted: Option<&…>, format: &str) -> bool       // None ⇒ anything answers
```

Filtering is on the **extension**, never the family. `.ks` and `.preseed` are both
`Kind::Text`; filtering by family would let a preseed answer `/rhel/ks`.

`None` — a URL naming no alias — constrains nothing, which is what keeps `/answer` working
for a deployment that only ever serves one format.

## The listing cache

```rust
struct Cached { version: Version, loaded_at: Instant, listing: Arc<Listing> }
```

Reused only when the store's `version()` is unchanged, **is `Some`**, and less than
`RELOAD_BACKSTOP` (1 s) has passed.

The literal reading of the specification — re-read the directory on every request — is a
`readdir` plus a sort plus a normalization pass per request. With one answer document per
machine, throughput collapses:

| Documents | Literal re-read | mtime-cached |
|---|---|---|
| 10 | 11,954 req/s | 12,922 req/s |
| 200 | 3,198 req/s | 12,890 req/s |
| 2,000 | 311 req/s | 12,520 req/s |
| 10,000 | — | 6,924 req/s |

One `stat` replaces the whole walk, and a new document is still picked up with no restart
— which is the guarantee the specification actually wanted. Normalized stems are computed
once per store read, not once per request.

> **The backstop is not redundant.** Editing a group file's *contents* moves no directory
> mtime, and a change made by another process moves no in-process atomic. An integration
> test covers exactly this.

The remaining cost at 10,000 documents is a linear scan of precomputed needles — pure CPU,
no syscalls. Bucketing needles by length and sliding a window over the body would remove
it, but a 10,000-machine rollout already completes in under two seconds. **Measure before
adding it.**

## Building a `Listing`

`build(snapshot)` does everything expensive once:

- parse every document, keeping the error rather than failing the load;
- normalize every stem and every `members` entry;
- resolve `extends` chains, **detecting cycles and missing parents** — the broken group is
  dropped rather than half-applied, and the problem is recorded;
- pre-merge each group's chain, and **pre-render it as a string** when it carries no
  placeholders.

That last one is why grouping is the fast path: the common datacenter case parses nothing
per request. `Group::has_placeholders` is the flag that decides it.

`problems` is collected here, not at request time, which is what lets the admin API's
[rollback guard](./admin.md#3-the-write-that-cannot-break-the-fleet) catch a broken
`extends` before anyone asks for it.

## Resolution

`resolve()` is a match on `(machine, machine_doc, group)`:

| Case | Behaviour |
|---|---|
| group only | serve the prepared string, or clone-fill-strip-render when templated |
| machine only | fill, strip, render |
| both | group chain, merge the machine on top, fill, strip, render |
| neither | fall back to `default` for the requested format, which may itself `extends` a group |

Template variables are the request's facts plus `machine` and `group`, which the facts
cannot carry because they are only known once matching has happened.

> **`machine` is bound only when a machine *document* matched.** A machine claimed by a
> group's `members` with no document of its own resolves with `machine: None`, so
> `{{ machine }}` in a group fails for exactly the members it was meant to serve. The
> [templating guide](../guide/answers/templating.md#machine-needs-a-machine-document) says
> to use a request fact there instead.
