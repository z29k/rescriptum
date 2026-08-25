---
title: The SQLite store
description: The same answers in a database instead of a directory — identical behaviour, a reversible move, and no extra software to install.
sidebar:
  label: SQLite store
  order: 5
---

# The SQLite store

A directory of files is the default, and it is the right answer for a handful of machines:
greppable, diffable, in git if you like, with no database to run, back up or migrate.

For a fleet administered by **tooling** rather than by hand, the same answers can live in
a SQLite database. It is compiled into the binary, so there is still nothing to install.

```console
$ export RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db
$ rescriptum import /srv/answers      # bring the files across
$ rescriptum check                    # the same checks, now against the database
$ rescriptum                          # serve from it
```

## Why you would

- **The [admin API](./admin-api.md) needs it.** Managing answers over HTTP requires the
  database; over files there would be two ways to change the same configuration — by hand
  and over the wire — racing each other.
- **Concurrent writes are safe.** WAL mode, so an administrative write never stalls an
  install in progress.
- **One file to back up**, and it moves atomically.

## Why you might not

- **A directory is legible.** `git log answers/` answers "who changed this rack and why";
  a database does not, unless your tooling records it.
- **A file is editable with anything.** `vi`, `scp`, a Makefile.
- **It is 1.2 MB of binary** — 2.4 MB with SQLite versus 1.3 MB without, on ARMv7. Build
  with `cargo build --no-default-features` if that matters and you do not need it.

## The behaviour is identical

Matching, groups, `extends`, merging, templating, `render`, `check` — all of it lives
*above* the store, which is deliberately thin: it hands back raw document text and a cheap
version token, and decides nothing.

That is enforced rather than asserted. `tests/stores.rs` runs **every behavioural case
twice**, once per store, and requires the identical outcome. A new behaviour belongs in
that suite, not in a store-specific test.

## Moving between them

```console
$ rescriptum import /srv/answers      # directory → the configured store
$ rescriptum export /tmp/backup       # the configured store → a directory
```

```console
$ RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db rescriptum import examples
copying files:examples -> sqlite:/srv/answers.db
  10 group(s), 8 machine(s)
  ok — now run `check` against the target
```

**The round trip is byte-identical.** Import a directory, export it again, and `diff -r`
reports nothing — comments, formatting and all. That is what makes the database safe to
adopt *and* safe to leave, and it is worth keeping true.

Both directions run `check` afterwards on your say-so rather than automatically; the
output above tells you to.

## Schema versions

The database carries a schema version (`user_version`). There is one so far, and nothing
has been released under an older one, so there is nothing to migrate from.

What the version is for is the other direction: an **older** binary refuses to open a
database written by a newer one rather than guessing at what changed.

```
database schema is version 2, this binary understands 1
```

So a rollback across a future schema change needs the export from before the upgrade, or a
binary new enough to read the database. Keep an `export` around when you upgrade across
one.

## Operational notes

- **`version()` is an in-process atomic**, not a query, because it is called per request.
  A change made by a *different* process is picked up by the 1-second reload backstop
  instead.
- **The database file and its `-wal`/`-shm` siblings** all need to be writable, and they
  all belong to the same backup.
- **`RESCRIPTUM_DB_PATH` defaults to `/srv/answers.db`**, a sibling of the default answers
  directory. The database holds the same curated content, not runtime state, so it belongs
  in the same tree.
- **The parent directory is created** if it does not exist.

## Related

- [The admin API](./admin-api.md) — the reason most people turn this on.
- [How the stores are built](../../development/stores.md) — the trait, and why it is thin.
