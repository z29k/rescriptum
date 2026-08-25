---
title: Configuration
description: Every environment variable, its default, and what happens when you get one wrong.
sidebar:
  label: Configuration
  order: 1
---

# Configuration

Environment variables only — and, optionally, a file to read some of them from. There is
no configuration *format* to learn and no command line to get wrong.

## The variables

| Variable | Default | Meaning |
|---|---|---|
| `RESCRIPTUM_ENV_FILE` | unset | Read defaults from this file too — see [below](#the-env-file) |
| `RESCRIPTUM_STORE` | `files` | `files` (a directory) or `sqlite` (a database) |
| `RESCRIPTUM_ANSWERS_DIR` | `/srv/answers` | Directory of answer documents |
| `RESCRIPTUM_DB_PATH` | `/srv/answers.db` | Database path, when `RESCRIPTUM_STORE=sqlite` |
| `RESCRIPTUM_LISTEN_ADDR` | `0.0.0.0:8000` | Listen address. `:0` picks a free port, and the bound one is printed |
| `RESCRIPTUM_WORKERS` | CPU count | Async runtime threads. **Not** a concurrency limit |
| `RESCRIPTUM_MAX_CONNECTIONS` | `2048` | In-flight connections before shedding with `503` |
| `RESCRIPTUM_TIMEOUT_SECS` | `10` | Header-read timeout **and** whole-connection deadline |
| `RESCRIPTUM_ANSWER_TOKEN` | unset | Bearer token the answer endpoint requires. Unset means open |
| `RESCRIPTUM_ADMIN_ADDR` | unset | Admin API listener. Unset means the admin API is off |
| `RESCRIPTUM_ADMIN_TOKEN` | unset | Admin bearer token, 16+ characters. Required with `RESCRIPTUM_ADMIN_ADDR` |
| `RESCRIPTUM_CAPTURE_DIR` | unset | Record request bodies here. Unset means no capture |
| `RESCRIPTUM_LOG` | `all` | `all`, `problems` or `off` — see [below](#logging) |
| `RESCRIPTUM_LOG_FILE` | unset | A file to append to, or `stdout` / `stderr`. Unset means stderr |

`/srv` is where the filesystem hierarchy standard puts data served by the system, which is
what an answers directory is. Both defaults live there so that a bare `rescriptum` does
something plausible on any Linux host. Nothing creates the directory for you, and the server
says so at startup if it is missing.

## Logging

One line per event, on stderr by default. Two knobs, because the two questions are
different.

**What** — `RESCRIPTUM_LOG`:

| Value | Keeps |
|---|---|
| `all` (default) | every request, plus startup, warnings and errors |
| `problems` | startup, warnings, errors, and only the requests that **did not** succeed |
| `off` / `none` | nothing at all |

A successful answer is one line, and at thirteen thousand requests a second that is the
only thing here with any volume. `problems` is what you want once a rollout is routine and
the disk is not. Everything else is low-volume and diagnostic, so it survives both.

A request that never reached a status at all — a connection that timed out mid-body —
counts as a problem. An unrecognised value falls back to `all` with a warning: a typo must
not be the reason nobody can see why a rollout failed. The level is named in the startup
line (`log=problems`), so an empty log explains itself.

**Where** — `RESCRIPTUM_LOG_FILE`:

| Value | Goes to |
|---|---|
| unset, or `stderr` | stderr, which is what a supervisor reads |
| `stdout` | stdout |
| any other value | that file, appended to; parent directories are created |

A file that cannot be opened is a **startup error**, not a fall back to stderr — that would
be a silent surprise discovered much later. A write that fails once the server is running
is dropped instead: a provisioning server that died because its log disk filled up would
fail every install in flight in order to report that it could not report something.

Rotation is yours. Under systemd there is nothing to do, since the log goes to the journal;
with a file, point `logrotate` at it with `copytruncate`.

## The env file

`RESCRIPTUM_ENV_FILE` names a file of the same variables. It exists for deployments with
nowhere good to put a token — chiefly **Synology DSM 7, which has no systemd**. Under
systemd, `EnvironmentFile=` already does this and you do not need it.

```sh
# /etc/rescriptum.env   (chmod 600, owned by root)
RESCRIPTUM_STORE=sqlite
RESCRIPTUM_DB_PATH=/srv/answers.db
RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
RESCRIPTUM_ADMIN_TOKEN=…
```

```console
$ RESCRIPTUM_ENV_FILE=/etc/rescriptum.env rescriptum
2026-08-24T12:42:02Z - reading configuration defaults from /etc/rescriptum.env (4 set)
```

**It is never discovered, only named.** There is no `./.env`. This binary runs as root: if
it picked a file up from whatever directory it happened to be launched in, anyone who could
write there would own `RESCRIPTUM_ADMIN_TOKEN` — and with it the root password of every
machine installed afterwards.

**The real environment wins.** The file supplies defaults, so something exported
deliberately at launch is never silently overridden. An exported-but-empty variable counts
as unset, so the file still applies.

**A file that was asked for and cannot be read is a startup error**, not a warning. That is
the whole point: the failure it replaces is a server coming up on its defaults — wrong
answers directory, no admin token — without a word in the log.

### The format

| | |
|---|---|
| `KEY=value`, one per line | leading `export` is accepted, so the same file can also be `source`d |
| `#` at the **start of a line** | a comment |
| `#` anywhere else | **part of the value.** There are no inline comments: truncating a token at a `#` it legitimately contains would be silent, and a comment landing in a value is loud |
| `"quoted"` or `'quoted'` | the quotes are stripped and inner whitespace is kept; unquoted values are trimmed |
| `$HOME`, `${x}` | **not expanded.** This is not a shell — no substitution, no continuation lines |
| the same key twice | a startup error, rather than a guess about which was meant |
| a key this program does not read | a warning naming the key — so `RESCRIPTUM_ADMIN_TOKENN` is caught rather than ignored |
| a file others can read | a warning with its mode, because it may hold the admin token |

Warnings name keys and paths, never values.

## Invalid values

| Case | What happens |
|---|---|
| Exported but empty (`RESCRIPTUM_LISTEN_ADDR=`) | treated as **unset** — an empty value is a mistake, not an instruction |
| Whitespace-only | same, and values are trimmed |
| A zero or unparseable number | falls back to the **default**, rather than starting a server that accepts connections and never answers |
| `RESCRIPTUM_STORE` set to anything else | a warning, and `files` is used |
| `RESCRIPTUM_ENV_FILE` naming a missing, unreadable or malformed file | a startup **error** |
| `RESCRIPTUM_STORE=sqlite` on a binary built without the feature | a startup **error** |

## Startup errors

These stop the server rather than warning, because starting anyway would be worse:

| Condition | Why it is fatal |
|---|---|
| `RESCRIPTUM_ADMIN_ADDR` set with `RESCRIPTUM_STORE` not `sqlite` | two ways to change the same configuration, racing |
| `RESCRIPTUM_ADMIN_ADDR` set with no `RESCRIPTUM_ADMIN_TOKEN` | an open API that rewrites root credentials |
| `RESCRIPTUM_ADMIN_TOKEN` under 16 characters | short enough to guess |
| The listen address cannot be bound | nothing to do |
| The store cannot be opened | nothing to serve |

## Startup warnings

These are printed and the server carries on:

| Condition | Line |
|---|---|
| Answers directory missing | `warning: … does not exist yet — every request will 404 until it does` |
| Answers path exists but is not a directory | `warning: … is not a directory — every request will 404 until it is` |
| Answers directory present but unreadable | `warning: … cannot be read: … — every request will 404 until that is fixed`. The likeliest cause is the server running as a user that is not the directory's owner |
| Admin API not on loopback | `warning: the admin API is not bound to loopback — …` |
| `RESCRIPTUM_ANSWER_TOKEN` under 16 characters | a warning, **not** an error — refusing to start would leave a fleet unable to install |
| Any problem in the answer set | one `warning:` line each, the same set `check` reports |

## Compile-time options

| Feature | Default | Effect |
|---|---|---|
| `sqlite` | on | The SQLite store and the admin API. `cargo build --no-default-features` drops both — 944,928 bytes instead of 2,103,456 on ARMv7 |

## Fixed limits

Not configurable, and deliberately so:

| Limit | Value | Where |
|---|---|---|
| Request body | 1 MB | answer endpoint — an aberrant `Content-Length` is refused from the header |
| Document size | 256 KB | admin API `PUT` |
| Captured requests | 1000 captures | counted from the directory at startup, so a restart does not start again |
| Admin failures before a block | 5 within 60 s | block doubles to a maximum of 900 s |
| Addresses tracked by the guard | 4096 | so the guard cannot be turned into a memory leak |
| Listing reload backstop | 1 s | forces a re-read even when the directory mtime looks unchanged |
