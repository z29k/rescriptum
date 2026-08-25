---
title: Working on rescriptum
description: Orientation for contributors — what the repository holds, how to run it, and where the reasoning lives.
sidebar:
  label: Orientation
  order: 0
---

# Working on rescriptum

rescriptum is a small, focused thing: it works out which install config each machine should
get, composes it from layers, and serves it. Around 4,000 lines of Rust, 308 tests, and a
short list of constraints that are not up for casual revision.

This space is the *why*. The [Guide](../guide/index.md) is the *what*.

## Get it running

```bash
git clone https://github.com/z29k/rescriptum && cd rescriptum
cargo test                      # 308 tests
cargo run -- --help
```

Try a change against the worked examples rather than only against tests — they are the
only place all the formats are shown composing together:

```bash
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- render --query "path=/rhel/ks&serial=7ABC123"
```

Before opening a PR:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --no-default-features   # the smallest build must keep working
```

Those four are exactly what [CI](./testing.md#ci) runs.

## The repository

| Path | Holds |
|---|---|
| `src/main.rs` | runtime setup, accept loop, connection serving, routing, and the blocking half of a request |
| `src/lib.rs` | the crate. `main.rs` is a thin binary over it, so behaviour is testable directly |
| `src/select.rs` | normalization, matching, layering — [the behaviour that matters](./selection.md) |
| `src/facts.rs` | what a request says about the machine |
| `src/format/` | one interface per document format; `xml.rs` holds the XML tree |
| `src/merge.rs` | the TOML merge, used by `format` |
| `src/store/` | where documents come from: `file.rs`, `sqlite.rs`, behind a thin trait |
| `src/admin.rs` | the write API, and the guarantee that a write cannot break the fleet |
| `src/config.rs` | environment configuration |
| `src/envfile.rs` | the optional file of defaults `RESCRIPTUM_ENV_FILE` names — never discovered, only named |
| `src/capture.rs` | recording what machines actually send |
| `src/cli.rs` | the `render`, `check`, `import` and `export` subcommands |
| `src/log.rs` | one line per event, UTC timestamps without a date crate, and the two knobs over both |
| `tests/` | the real binary over a socket (`integration`, `admin`, `guards`), its command line (`cli`), and the two-store conformance suite (`stores`) |
| `examples/` | a worked example of every supported format |
| `docs/` | this site |

**Never re-declare a module in `main.rs`.** It compiles a second copy, runs every unit
test twice, and lets the two copies drift.

## Where to start reading

- **[The constraints](./constraints.md)** — first. They explain most of the code's shape,
  and several of them look like things worth "improving" until you know why they are
  there.
- **[Architecture](./architecture.md)** — the module map and what flows between them.
- **[The request lifecycle](./request-lifecycle.md)** — a request from accept to response.
- **[Selection](./selection.md)** — the part with the most behaviour per line.
- **[Traps already hit](./traps.md)** — a list of things that cost time once. Reading it
  is cheaper than rediscovering them.

## Conventions

- **English** for code, comments, commit messages, and the source of the documentation.
  The docs are additionally published in French (`*.fr.md` siblings) — see
  [the documentation site](./docs-site.md#two-languages).
- **Behaviour belongs in `tests/stores.rs`**, which runs every case against both stores
  and requires the identical outcome. A test covering one store proves half of what it
  claims. See [testing](./testing.md).
- **Arrays replace, they do not append**, in every format.
- **Fail loudly.** A missing group, an unfillable template, a document that will not
  parse — all are errors with a reason. Serving a half-built answer installs a machine
  wrongly, and nobody finds out until it is running.
- **Adding a dependency needs a reason in the commit message.** This binary runs as root
  on other people's hardware, and CI's `audit` job is the other half of that rule: a reason
  to add one is not a reason to keep it.
- **Conventional commits with a scope** — `feat(http): …`, `fix(select): …`.

## Also worth reading

[`CLAUDE.md`](https://github.com/z29k/rescriptum/blob/main/CLAUDE.md) at the repository
root is the architecture document written for coding agents. It overlaps this space
heavily and is the file to update when a constraint changes.
