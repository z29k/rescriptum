---
title: The stores
description: Two backends behind a two-method trait, and the conformance suite that keeps them from drifting.
sidebar:
  label: Stores
  order: 6
---

# The stores

Answers come from either a directory of documents (`RESCRIPTUM_STORE=files`, the default)
or a SQLite database (`RESCRIPTUM_STORE=sqlite`), chosen at runtime.

## The trait is deliberately thin

```rust
pub trait Store: Send + Sync {
    fn version(&self) -> Version;               // Option<String>, cheap per request
    fn snapshot(&self) -> io::Result<Snapshot>; // only when version moved
    fn describe(&self) -> String;
}
```

A `Snapshot` is raw document text and nothing else: `RawMachine`, `RawGroup`,
`RawDefault`, each carrying an identifier, a format and a body.

**Every decision lives above this.** Matching, `extends` chains, merging, rendering,
`check` — all in `select.rs` and `merge.rs`, shared. Keep it that way: the moment a
backend starts deciding behaviour, the two drift and the conformance suite stops being
able to prove they have not.

The write half is separate, because serving answers never needs it:

```rust
pub trait StoreWrite: Store {
    fn put_machine(&self, id: &str, format: &str, body: &str) -> io::Result<()>;
    fn delete_machine(&self, id: &str, format: &str) -> io::Result<bool>;
    fn put_group(&self, name: &str, format: &str, body: &str) -> io::Result<()>;
    fn delete_group(&self, name: &str, format: &str) -> io::Result<bool>;
    fn put_default(&self, format: &str, body: &str) -> io::Result<()>;
    fn delete_default(&self, format: &str) -> io::Result<bool>;
}
```

**Every operation names a format.** A document is keyed by *what it is for* — a machine
*and* an operating system.

> An earlier `put` deleted the other formats of a stem, to avoid "two answers for one
> machine". That was the wrong model: they are that machine's answers for **two operating
> systems**, and both are meant to exist. See [traps](./traps.md).

## `tests/stores.rs` is the guarantee

Every behavioural case runs **twice**, once per store, and asserts the identical outcome.
35 cases at last count.

**A new behaviour belongs there, not in a store-specific test.** A test that covers one
backend proves half of what it claims.

## The file store

**One directory per identity.** A machine is a directory named after it, holding one
document per format; `groups/` holds the same shape for groups, and `default/` the
fallbacks. Both names are reserved, so a machine cannot claim them — `valid_machine_id`
refuses them in *both* stores, because a database that accepted one would export into a
directory that cannot hold it.

Inside a directory, **the extension is the format and the stem is nothing at all**. That is
the rule that makes two documents of one format in one directory a *reported problem* rather
than a resolved one: there is no tiebreak an operator could have predicted. Sorted order
decides which of the two answers, so the choice at least does not depend on readdir — and
the loser is named in `problems()`.

A servable document left at the top of the answers directory — the layout that came before —
is **reported and not served**, with its destination spelled out. Half-reading an old layout
would mean a machine whose answer moved silently between two files. `pending_moves()` is the
same knowledge exposed for `migrate`, so the command and the reader cannot disagree about
where a document belongs.

`version()` is the directory's mtime:

```rust
fs::metadata(&self.dir).ok()
    .and_then(|m| m.modified().ok())
    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    .map(|d| d.as_nanos().to_string())
```

One `stat` replaces a whole directory walk — see
[the listing cache](./selection.md#the-listing-cache).

> The directory's mtime moves when an entry is **added or removed** *in it*, not when one
> is **edited**, and not when something changes one level down. So a machine's whole
> directory appearing or leaving is seen at once, while a document added or edited *inside*
> one waits for the 1-second reload backstop — which is what already covered a file edited
> in place. A unit test pins each half.

> **What the layout costs on a read.** A full reload is now a `readdir` per identity on top
> of the file it already opened. Measured at 2,000 machines on an M1 Pro: **28 ms flat,
> 63 ms with a directory each**. It is amortised over a second's worth of requests, and
> end-to-end throughput did not move measurably — but it is a real 2.2× on the one operation
> the backstop guarantees will run every second, and it is the reason to reach for a group
> before a directory per machine.

**Writes go through a temporary file plus `rename`**, which is atomic within a directory
on POSIX, so a reader never meets a half-written answer. The temporary name carries the
process id, and is removed if the rename fails. A test asserts no `.tmp` file survives.

**Reading is `DirEntry::file_type()`, not `fs::metadata`.** The file type comes back free
with the `readdir` on Unix; only a symlink needs the stat to resolve. That alone was worth
65% at 2,000 files, *before* caching was added.

## The SQLite store

`rusqlite` with the `bundled` feature — SQLite is compiled from source into the binary, so
there is nothing to install. It cross-compiles to `armv7-musl` under zigbuild; CI builds
that target on every push precisely because a C dependency breaks there first.

**WAL mode**, so the admin API never stalls an install in progress.

**`version()` reads an in-process atomic**, not the database:

```rust
Some(self.revision.load(Ordering::Relaxed).to_string())
```

It is called per request, and a query per request would defeat the point of caching. The
consequence is that a change made by **another process** does not move it — the
[reload backstop](./selection.md#the-listing-cache) is what catches that.

**Schema versions** live in `PRAGMA user_version`. There is one, and nothing has been
released under an older one, so `migrate()` has no steps: it refuses a database from the
future, creates the schema when the version is `0`, and stamps it. The shapes this went
through while it was being written never left the repository, and carrying migrations from
them would be carrying code that cannot run.

What the version is for is the rollback direction:

```
database schema is version 2, this binary understands 1
```

Refused rather than guessed at, because a database written by a newer binary may hold
columns this one would silently ignore — and silently ignoring part of an answer set is how
a machine gets installed wrongly.

## `import` / `export`

```console
$ rescriptum import <dir>    # directory → the configured store
$ rescriptum export <dir>    # the configured store → a directory
```

Both go through `Snapshot`, so they share every rule. **The round trip is byte-identical**
— import a directory, export it again, `diff -r` reports nothing, paths included. A test
compares both sides at the same path for exactly that reason: `export` writing a document
somewhere `import` would not look for it is what would make the database unsafe to leave. That is what makes the
database safe to adopt *and* safe to leave, and it is worth keeping true.

## Identifiers become directory names

```rust
pub fn valid_id(id: &str) -> bool           // letters, digits, - _ . : and no separators
pub fn valid_machine_id(id: &str) -> bool   // …and not `groups` or `default`
```

Enforced at the admin API boundary **and** in both stores. The store is the layer that
turns an identifier into a path, so it is the layer that must not be fooled — checking
only at the boundary would make the guard depend on every future caller remembering.

`valid_format` is the equivalent for extensions: a document in a format nobody can read
never reaches the store in the first place.

## The `sqlite` cargo feature

On by default, and removable:

| Build | ARMv7 size |
|---|---|
| default | 2,103,456 bytes |
| `--no-default-features` | 944,928 bytes |

Dropping it also drops the admin API, which needs the database. CI builds
`--release --no-default-features` on every push so the smallest build cannot rot
unnoticed.
