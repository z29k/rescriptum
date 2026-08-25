---
title: Testing
description: Where a test belongs, the two-store conformance suite, and the assertions that have actually caught things.
sidebar:
  label: Testing
  order: 8
---

# Testing

308 tests. `cargo test` runs all of them in a couple of seconds.

```bash
cargo test                                # everything
cargo test <name>                         # one, by substring
cargo test -- --nocapture                 # show stdout
cargo test --all-features                 # what CI runs
```

## Where a test belongs

| Suite | Cases | For |
|---|---|---|
| `tests/stores.rs` | 38 | **every behaviour, against both stores** |
| `tests/integration.rs` | 45 | the real binary over a real socket |
| `src/select.rs` | 27 | normalization, scoring, layering, template filling |
| `src/format/mod.rs` | 27 | parsing, merging, control keys, endpoint aliases |
| `src/facts.rs` | 22 | query parsing, JSON flattening, globbing |
| `tests/admin.rs` | 26 | the admin API end to end, formats included |
| `tests/cli.rs` | 29 | `render`, `check`, `import`, `export`, and the env file — against the real binary |
| `src/log.rs` | 4 | level parsing, and the timestamp arithmetic |
| `src/format/xml.rs` | 18 | the XML tree — pairing, entities, fidelity |
| `src/config.rs` | 19 | the environment, and what refuses to start |
| `src/merge.rs` | 11 | the TOML deep merge |
| `tests/guards.rs` | 7 | the answer token, and the lockout that deliberately is not there |
| `src/envfile.rs` | 14 | the env-file parser, and what it refuses |
| `src/admin.rs`, `src/capture.rs`, `src/store/mod.rs` | 21 | unit-level behaviour |

## `tests/stores.rs` — the conformance suite

Every behavioural case runs **twice**, once per store, and asserts the identical outcome.
That suite is what keeps two backends from drifting.

**A new behaviour belongs there, not in a store-specific test.** A test that covers one
backend proves half of what it claims — and the half it does not cover is exactly where a
divergence hides.

## `tests/cli.rs` — the commands people are told to run

`check` is what [`deploy.sh`](./building.md#deploying-a-build) runs before it ships
anything, and what the documentation tells people to put in CI — so **its exit code is a
contract**, not a convenience. `render`'s stdout/stderr split is another: the document
goes to stdout so `render … > answer.toml` yields a usable file, and the provenance line
goes to stderr so it does not end up inside it.

Also pinned here: the `import` → `export` round trip is **byte-identical**, comments and
formatting included. That is what makes the database safe to adopt *and* safe to leave;
if it ever stops being exact, `export` is no longer a way back out.

## `tests/integration.rs` — against the real binary

It starts the actual binary on an **ephemeral port** and talks HTTP to it. The binary
prints the address it bound, so there is no port race and no sleep-and-hope.

This suite exists because some failures are invisible to unit tests. The clearest example:
hyper **panics at runtime** if `header_read_timeout` is set without `.timer(…)`. It
compiles. Only a real connection finds it.

Explicitly covered:

- a **truncated request**, and one with **no `Content-Length`**;
- an aberrant `Content-Length` — *and* a chunked body that outgrows the cap while
  streaming, which is the other way in and trips the limit mid-read;
- an unknown method, an empty body, a 1 MB body, a body that is not valid UTF-8;
- the connection cap: over it, a prompt `503` rather than a queue — and the permit coming
  back afterwards;
- **and, after each of those, that the server still answers.** That last assertion is the
  one that matters — the abuse is only interesting if the server survives it.

> **`cargo test` does not rebuild `target/debug/rescriptum`.** A manual check against a
> stale binary once "reproduced" a bug that had already been fixed. Rebuild before poking
> at the binary by hand.

## Check that a test can fail

A test that passes for the wrong reason is worse than no test: it reports coverage that
does not exist. Before trusting a new one, break the thing it guards and watch it go red.

One in this suite did not survive that check. It claimed to protect the `version.is_some()`
clause in the listing cache; removing the clause left it green, because with either store a
version is unreadable only when the store is also empty, so the clause cannot currently
fire at all. The test proves something real — a directory that appears after startup is
served on the very next request — and now says so instead.

## Assertions worth copying

- **Assert on parsed values, not on formatting.** Replacing a table with a scalar leaves
  the key's original decor, so the output can read `value= 3` — valid TOML, different
  text. A string comparison there fails for the wrong reason, or passes for one.
- **Cache-invalidation tests must share one `Answers` instance.** A test that constructs a
  fresh one per call bypasses the cache entirely and silently proves nothing.
- **`Config::from_lookup` takes a closure**, so configuration tests never touch the
  process environment — and therefore never race each other under a parallel test runner.
- **Assert the old text was found before writing.** Two `python`/`sed` patches in this
  project's history silently matched nothing and were only caught by checking test counts
  afterwards.

## The example answers are a test too

```bash
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
```

[`examples/`](https://github.com/z29k/rescriptum/tree/main/examples) holds a worked example
of every format, and it is the only place they are shown composing together. Two of them
caught real bugs — a missing doctype and an unpaired `pass` attribute. Keep them working.

## CI

`.github/workflows/ci.yml`, on every push to `main` and `develop` and on every pull
request:

| Job | Runs |
|---|---|
| **gates** | `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -D warnings`, `cargo test --all-features`, `cargo build --release --no-default-features` |
| **docs** | builds the public site and runs `notabene lint` |
| **audit** | `cargo audit --deny warnings` over the dependency tree |
| **cross** | a full ARMv7-musl build, then asserts the binary really is `statically linked` |

The cross job is not redundant. **SQLite is compiled from source, and `armv7-musl` is the
least forgiving target shipped** — it is where a C dependency breaks first. Catching that
on a push beats catching it while cutting a release.

The **audit** job is the other half of the rule that adding a dependency needs a reason: a
reason to add one is not a reason to keep it. `--deny warnings` fails on an unmaintained or
yanked crate too, not only on a vulnerability. When something appears with no fix, add
`--ignore RUSTSEC-…` with a line saying why rather than dropping the flag.

Every action used is an official `actions/*` one, and both Zig and `cargo-audit` are
installed directly rather than through a third-party action. That is deliberate: this
toolchain vets and links a binary people run as root.

The docs site has its own gate — see [the docs site](./docs-site.md#the-ci-gate).
