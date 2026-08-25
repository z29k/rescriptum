---
title: Admin API internals
description: Its own listener, a constant-time token, an exponential-backoff guard, and a write that rolls itself back.
sidebar:
  label: Admin API
  order: 7
---

# Admin API internals

`src/admin.rs`, enabled only by `RESCRIPTUM_ADMIN_ADDR`, and only over SQLite. Three
properties are load-bearing — **a change that quietly drops any of them is a regression**.

## 1. Its own listener

The answer endpoint is unauthenticated by necessity: the installer has no credentials to
offer. This API sets the root password and SSH keys of every machine installed afterwards.
It never shares that port.

`Config::validate` refuses to start — as an error, not a warning — without a token, with a
token under 16 characters, or over the file store. Those checks run *before* the listener
is bound, so a misconfiguration is never briefly live.

## 2. SQLite only

Over a directory of files there would be two ways to change the same configuration, by
hand and over the wire, racing each other.

## 3. The write that cannot break the fleet

```rust
fn guarded(admin, kind, id, format, body) -> Response<Body> {
    let before   = admin.answers.problems()?;   // snapshot the damage
    let previous = admin.store.snapshot()?…;    // what was there, so it can be restored
    let existed  = apply(…)?;                   // put or delete
    let after    = admin.answers.problems();
    let introduced = after.filter(|p| !before.contains(p));
    if !introduced.is_empty() { restore(previous); return 409 }
    200 with `problems: before`
}
```

- Only **newly** introduced problems roll back. A store that was already broken stays
  editable — otherwise a bad state would be unfixable through the API that caused it.
- A successful write still reports the **pre-existing** problems, so a clean response
  never implies the whole set is healthy.
- This is why a machine's `extends` pointing at a missing group is detected at
  [**load** time in `select.rs`](./selection.md#building-a-listing) rather than only when
  that machine asks. **The guard can only catch what `problems()` reports.** Adding a new
  class of breakage means adding it there, or the guard silently stops covering it.

Malformed documents are refused at write time (`400`) rather than becoming a `500` the
next time a machine asks for one.

## Authentication

**Constant-time comparison.** An ordinary `==` returns early on the first differing byte,
which leaks the token one byte at a time to anyone timing the responses — a few thousand
requests instead of an impossible number. Comparing every byte regardless removes the
signal. Over a network the timing is usually lost in jitter, so this is precautionary; it
costs five lines.

**`AuthGuard`** shuts out an address after repeated failures:

| Constant | Value |
|---|---|
| `MAX_FAILURES` | 5 |
| `FAILURE_WINDOW` | 60 s |
| `BASE_BLOCK` | 60 s, doubling on repeats |
| `MAX_BLOCK` | 900 s |
| `MAX_TRACKED` | 4096 addresses |

Three details that are not accidents:

- **The block applies to a *correct* token too.** Otherwise guessing until you got it
  right would cost nothing.
- **`MAX_TRACKED` is bounded**, so the guard cannot itself be turned into a memory leak by
  an attacker cycling source addresses.
- **`GET /health` is checked before the guard and before auth**, so monitoring does not go
  dark during an attack.

A block answers `429` with `Retry-After`.

## Request handling

```rust
let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
match (&method, segments.as_slice()) {
    (&Method::GET,    ["machines"])      => list(…),
    (&Method::GET,    ["resolve", id])   => resolve(…),
    (&Method::PUT,    ["groups", id])    => put(…).await,
    …
    _ => error(NOT_FOUND, "no such endpoint"),
}
```

`?format=` selects the document's extension, defaulting to `toml` — which is what this
server started life serving.

> **Read the request body before rejecting a request.** Answering and closing while the
> client is still writing earns a `ECONNRESET` instead of the response. `put()` drains
> first, then validates the identifier.

> **Every admin response must set `Connection: close`.** Without it, every test client
> waited out the connection timeout — the suite took 30 s instead of 0.4 s — and the
> eventual drop sometimes arrived as a reset rather than a clean EOF.

`GET /resolve` sets `X-Answer-Source` with the same description the log line uses.

> **`GET /resolve/{id}` ignores the path identifier when a query string is present** —
> facts come from the query alone, so it can rehearse a real request. That makes
> `?format=toml` on that endpoint actively wrong: it resolves nothing. Documented in the
> [guide](../guide/operations/admin-api.md#rehearsing-a-real-request).

## Identifiers

`valid_id` — letters, digits, `- _ . :`, no path separators — is enforced **at the API
boundary and in both stores**. `export` turns identifiers back into filenames, so
anything that could traverse a directory has to be rejected in the layer that builds the
path, not only in the layer that received it.

## Known and accepted

- **Per-address limiting does not stop an attacker with many addresses.** The token's
  *length* is what makes guessing hopeless — hence the 16-character floor at startup.
- **It speaks plain HTTP.** Put TLS in front if it leaves loopback.
- **Binding beyond loopback logs a warning** rather than refusing, because a management
  network is a legitimate choice.

## Tests

`tests/admin.rs` (15 cases) covers routing, the rollback, identifier validation and the
status codes. `tests/guards.rs` (5) covers the lockout arithmetic and that `/health` stays
reachable through it.
