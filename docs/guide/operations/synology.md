---
title: Synology DSM 7
description: The original target — an ARMv7 DS416j with 512 MB and no Docker. Autostart, firewall, and replacing a running instance.
sidebar:
  label: Synology DSM 7
  order: 2
---

# Synology DSM 7

A Synology DS416j is why this project exists: ARMv7, 512 MB of RAM, DSM 7, no Docker. A
static binary with no runtime is not an aesthetic preference there — it is the only thing
that fits.

DSM gives you no systemd, so autostart goes through the Task Scheduler.

## 1. Get the binary onto it

Use the **`armv7-unknown-linux-musleabihf`** build from the
[releases page](https://github.com/z29k/rescriptum/releases), or cross-compile one
yourself (see [building](../../development/building.md)).

```console
$ scp rescriptum admin@nas:/volume1/netboot/rescriptum
$ ssh admin@nas chmod +x /volume1/netboot/rescriptum
```

If ARMv7 misbehaves, confirm the real architecture before assuming:

```console
$ ssh admin@nas uname -m
armv7l
```

A DS918+ or any x86 model wants `x86_64-unknown-linux-musl`; a DS220j and other newer ARM
models want `aarch64-unknown-linux-musl`.

The build must be **statically linked** — DSM's glibc is old enough that a dynamically
linked binary fails at exec time, on the NAS, with an error that does not obviously say
so:

```console
$ file rescriptum
ELF 32-bit LSB executable, ARM, EABI5 version 1 (SYSV), statically linked, stripped
```

## 2. Put your answers next to it

```console
$ ssh admin@nas mkdir -p /volume1/netboot/answers
```

`/volume1/netboot/` is where a DSM shared folder lives, so it is the natural home for both
the binary and the answers. It is not a default: `RESCRIPTUM_ANSWERS_DIR` defaults to
`/srv/answers`, which does not exist on DSM, so set it explicitly here. The env file below
is the tidiest place to do that.

## 3. Autostart

**Control Panel → Task Scheduler → Create → Triggered Task → User-defined script**

| Field | Value |
|---|---|
| Event | **Boot-up** |
| User | `root` |
| Command | see below |

```sh
RESCRIPTUM_ANSWERS_DIR=/volume1/netboot/answers /volume1/netboot/rescriptum
```

If you use a token, **do not put it in that box.** Anything in a process's arguments —
and in DSM's case, in the task definition — is readable by every user on the machine
through `ps`. Put the configuration in a root-only file and name it instead:

```sh
# /volume1/netboot/rescriptum.env   (chmod 600, owned by root)
RESCRIPTUM_ANSWERS_DIR=/volume1/netboot/answers
RESCRIPTUM_STORE=sqlite
RESCRIPTUM_DB_PATH=/volume1/netboot/answers.db
RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
RESCRIPTUM_ADMIN_TOKEN=…
RESCRIPTUM_ANSWER_TOKEN=…
```

```sh
# the Task Scheduler entry runs this
RESCRIPTUM_ENV_FILE=/volume1/netboot/rescriptum.env exec /volume1/netboot/rescriptum
```

**Prefer this to sourcing it.** The older form —
`. /volume1/netboot/rescriptum.env && exec …` — works, and still does, but it fails
*silently*: drop the leading `.`, mistype a line, or get the permissions wrong, and the
shell sources nothing while the server comes up on its **defaults** — the default answers
directory, no admin token, and not a word about it in the log. With `RESCRIPTUM_ENV_FILE`
the binary reads the file itself and **refuses to start** if it cannot. It also warns if
the file is readable by anyone but root, and names any key it does not recognise, so a
`RESCRIPTUM_ADMIN_TOKENN` is caught rather than quietly ignored.

Details of the format are in the
[configuration reference](../reference/configuration.md#the-env-file).

Run the task once by hand from the Task Scheduler rather than waiting for a reboot to
find out it does not work.

## 4. Open the port

**Control Panel → Security → Firewall** — allow TCP 8000 (or whatever you set
`RESCRIPTUM_LISTEN_ADDR` to) from your provisioning network.

DSM's firewall is the single most common reason a machine "never contacts the server".

## 5. Verify

```console
$ curl http://NAS_IP:8000/health
OK
```

## Where the log goes

Nowhere, by default: DSM's scheduler discards a task's output. Name a file in the env file
and the server writes there itself, with no shell redirection to get wrong:

```sh
RESCRIPTUM_LOG_FILE=/volume1/netboot/rescriptum.log
```

The log line is the whole diagnostic story when a PXE install will not start, so this is
not optional. Once a rollout is routine, `RESCRIPTUM_LOG=problems` keeps the failures and
drops the successful answers, which are the only high-volume thing in there. Rotate the
file yourself; the server does not.

## Replacing a running instance

```console
$ ./deploy.sh admin@nas
```

It builds for ARMv7, [checks the answers first](../answers/validating.md), copies the
binary under a temporary name so a half-copied file is never executed, restarts it, and
confirms `/health` responds. Details in
[deployment](./deployment.md#replacing-a-running-instance).

The Task Scheduler entry is still what starts it after a reboot — `deploy.sh` only
replaces what is running now.

## Shutdown

DSM's scheduler sends `SIGTERM` on shutdown, which the server handles: it stops accepting
and exits. There is no state to lose either way.

## What to expect from a DS416j

512 MB and an ARMv7 core is not much, and it does not need to be. A connection costs
kilobytes rather than a thread, the directory listing is cached and invalidated by mtime
rather than walked per request, and a group with no per-machine overrides is rendered once
at load and served afterwards as a prepared string.

The one thing worth knowing: filesystem work happens on a blocking thread pool, because
`read_dir` on a NAS with a sleeping disk is not a fast call, and blocking an async worker
would stall every other connection it was driving.
