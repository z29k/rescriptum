---
title: Install
description: Download a binary or build one — then run it. There is no install step, no runtime and no container.
sidebar:
  order: 1
---

# Install

rescriptum is **one self-contained binary**. There is no runtime to install, no
interpreter, no container image, and nothing written outside the directory you point it
at. Copy it somewhere and run it.

## Download a release

Binaries for every published target are attached to each
[release](https://github.com/z29k/rescriptum/releases), with a SHA-256 sum beside them.

| Target | For |
|---|---|
| `armv7-unknown-linux-gnueabihf` | Synology DS416j and other ARMv7 NAS boxes (glibc ≥ 2.17) |
| `aarch64-unknown-linux-musl` | modern ARM NAS, Raspberry Pi |
| `x86_64-unknown-linux-musl` | most other Linux hosts |
| `aarch64-apple-darwin` | local development, Apple silicon |
| `x86_64-apple-darwin` | local development, Intel Macs |

```console
$ VERSION=0.2.0 TARGET=x86_64-unknown-linux-musl
$ curl -fsSLO https://github.com/z29k/rescriptum/releases/download/v$VERSION/rescriptum-$VERSION-$TARGET.tar.gz
$ curl -fsSLO https://github.com/z29k/rescriptum/releases/download/v$VERSION/rescriptum-$VERSION-$TARGET.tar.gz.sha256
$ shasum -a 256 -c rescriptum-$VERSION-$TARGET.tar.gz.sha256
$ tar xzf rescriptum-$VERSION-$TARGET.tar.gz
$ sudo install -m755 rescriptum-$VERSION-$TARGET/rescriptum /usr/local/bin/
```

Check the sum. This binary runs as root on hardware you are about to install, which is
about as much trust as a program gets.

## On a Synology

Take the `.spk` for your model instead — `rescriptum-<version>-armv7.spk` for the DS416j
and other `armada38x` machines, `-x86_64.spk` for every Intel model — and install it with
**Package Center → Manual Install**. It creates the shared folder, registers the port with
the firewall, links the CLI onto `PATH` and starts at boot. Details, and what it
deliberately does not do for you, are on the
[Synology page](./operations/synology.md).

The Linux builds are linked against musl, statically, so they do not care how old the
host's glibc is:

```console
$ file /usr/local/bin/rescriptum
ELF 64-bit LSB executable, x86-64, ... statically linked, stripped
```

## Or build it

You need a Rust toolchain and nothing else for a native build:

```console
$ git clone https://github.com/z29k/rescriptum && cd rescriptum
$ ./build.sh
```

Cross-compiling for the NAS needs [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild)
and Zig, which stand in for a full cross toolchain. The
[build page](../development/building.md) has the details, including how to confirm the
result really is static — a dynamically linked musl binary fails at exec time, on the
NAS, rather than at build time on your laptop.

## Run it

```console
$ mkdir -p /srv/answers
$ RESCRIPTUM_ANSWERS_DIR=/srv/answers rescriptum
2026-08-22T18:00:00Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=files:/srv/answers workers=8 max_conn=2048 timeout=10s
2026-08-22T18:00:00Z - warning: /srv/answers does not exist yet — every request will 404 until it does
```

The startup line is worth reading rather than scrolling past:

| Field | Meaning |
|---|---|
| `listening on` | the address actually bound, not the one requested — with `:0` they differ |
| `store=` | `files:<dir>` or `sqlite:<path>`, so a misconfigured store is visible immediately |
| `workers=` | runtime threads, CPU count by default. Not a concurrency limit |
| `max_conn=` | in-flight connections before the server sheds with `503` |
| `timeout=` | header-read timeout **and** whole-connection deadline |

Anything wrong with the answer set — a group extending a group that does not exist, a
document that will not parse — is reported here too, once, at startup. It is also
reported by [`rescriptum check`](./answers/validating.md), which is the better place to
find out.

Confirm it is alive:

```console
$ curl http://localhost:8000/health
OK
```

`GET /health` is the one endpoint that never needs a token and is never rate-limited, so
a monitor keeps working even while the server is refusing everything else.

## Where it looks by default

`RESCRIPTUM_ANSWERS_DIR` defaults to **`/srv/answers`**, and `RESCRIPTUM_DB_PATH` to
`/srv/answers.db`. `/srv` is where the filesystem hierarchy standard puts data served by
the system, which is what these are. Nothing creates the directory for you; the startup
line says so if it is missing.

Everything is configured through the environment; there is no configuration format to
learn and no command line to get wrong. If you have nowhere good to put a token — DSM 7,
say — `RESCRIPTUM_ENV_FILE` names a file of the same variables. The full list is in the
[configuration reference](./reference/configuration.md).

## Next

- [Serve your first answer](./quickstart.md) — a real machine getting a real document.
- [Deployment](./operations/deployment.md) — systemd, or the DSM task scheduler.
