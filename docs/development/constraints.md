---
title: The constraints
description: Deliberate design decisions that look like things worth improving until you know why they are there.
sidebar:
  label: Constraints
  order: 2
---

# The constraints

These are decisions, not oversights. Several of them look like obvious improvements from
the outside. **Do not change one without asking** — and if you do change one, change this
page and `CLAUDE.md` with it.

## Async, on tokio and hyper

The original specification asked for zero dependencies and a thread per connection. Both
were overridden deliberately, once the requirement became *"absorb a professional
provisioning burst"*. A 2,000-machine rollout is 2,000 near-simultaneous connections, and a
thread each is 2,000 stacks on a box with 512 MB.

What survived from the spec: no `serde` derive, no framework, and a very short direct
dependency list. See [architecture](./architecture.md#dependencies).

## hyper directly, not axum

axum gives no way to set a **header-read timeout**, which is precisely the slowloris guard
that motivated going async. Routing here is one `if` on method and path, so a framework
buys nothing and costs the one thing that mattered.

## Bounded concurrency, even though tasks are cheap

A connection costs kilobytes rather than a thread — that is the whole point of the async
rewrite. But *cheap* is not *free*, and unbounded accept still turns a burst into an
out-of-memory.

A `Semaphore` of `RESCRIPTUM_MAX_CONNECTIONS` caps in-flight connections. Over the cap the
server writes a **prompt `503` and closes** rather than queueing: a client told to retry is
better off than one parked in a queue that will not drain.

## Filesystem work goes through `spawn_blocking`

`read_dir` and `read` are blocking calls, and blocking an async worker thread stalls every
other connection that thread is driving. **On a NAS with a sleeping disk that is not
theoretical** — a spin-up is seconds, not milliseconds.

`resolve()` holds both the parse and the IO, and is only ever called inside
`spawn_blocking`. A panic there returns a `500`; it cannot take the server down.

## Never panic on malformed input

Any parse failure becomes an error response plus a log line. Write the code as if there
were no safety net.

There is one, deliberately: **the release profile does not set `panic = "abort"`.** With
unwinding, a panic is contained to the connection that caused it instead of killing a
server mid-install. Measured cost on ARMv7: **+2416 bytes, +0.8%**. Do not optimize it
back.

If the design ever moves to a thread pool, add `catch_unwind` at the worker boundary — a
pool thread that dies silently is worse than either.

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
# panic = "abort" is deliberately ABSENT
```

## The store decides nothing

It hands back raw document text and a cheap version token. Matching, `extends` chains,
merging, rendering and `check` all live above it and are shared.

**Keep it that way.** The moment a backend starts deciding behaviour, the two drift — and
`tests/stores.rs` stops being able to prove they have not.

## Storage layout is not the URL

Directories and database rows are a **lookup space** and must stay free to be reorganised.
A URL is a **public contract baked into an ISO** and must not move because someone renamed
a folder. An earlier design made the directory name *be* the URL segment and was discarded
for exactly that reason.

The consequence is that a document's key is **(identifier, format)**, which is what the
SQLite schema is built around.

## Never build a filesystem path from request data

This is the path-traversal guard, and it is structural rather than a check: only **direct
entries** of the answers directory are ever read. Identifiers arriving at the admin API
are separately validated, at the API boundary *and* in both stores, because `export` turns
them back into filenames.

## Answers must be valid documents

Before merging, an answer file was served as opaque bytes, so a malformed one reached the
installer. Now it is a `500` with the parse error in the log.

That is the better failure — an installer receiving half-valid TOML fails in a much more
confusing way — but it **is** a behaviour change, and fixtures written as YAML-ish text
stopped working when it landed.

## Fail loudly

A missing group, an unfillable template, a document that will not parse: all are errors
with a reason, never a best-effort answer.

The reasoning is always the same. **A half-built answer installs a machine wrongly, and
nobody finds out until it is running.** A failed install is noticed in minutes.

## Deliberate asymmetries

Two places where the obvious symmetry is wrong on purpose:

| | |
|---|---|
| **The answer token is never rate-limited; the admin token is** | a rack can sit behind one address, so shutting it out turns a bad token into a failed rollout. No installer talks to the admin API |
| **A short answer token warns; a short admin token refuses to start** | refusing to start would leave a fleet unable to install. Refusing to start the admin API costs nobody an install |

## What the spec asked for and did not get

`plans/rescriptum-spec.md` (gitignored, so a contributor will not have it) is the record
of what was first asked for, **not a description of what exists**. The project outgrew it
in every direction: multi-OS, selectors, templating, an admin API, a database store.

Three specific departures, all listed above: async rather than a thread per connection,
`panic = "abort"` omitted, and the request body parsed as untyped JSON to harvest facts.
Where the spec and this page disagree, this page is right.
