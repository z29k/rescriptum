---
title: Deployment
description: A systemd unit, an environment file, and how deploy.sh replaces a running instance without shipping a broken answer set.
sidebar:
  label: Deployment
  order: 1
---

# Deployment

The binary is self-contained: copy it somewhere, give it an answers directory, and start
it. Everything below is about doing that repeatably.

For Synology DSM 7 — which has no systemd — see [its own page](./synology.md).

## An environment file

Keep configuration in one root-readable file rather than in a unit or a command line.
**Anything on a command line is visible to every user on the machine through `ps`**,
which matters as soon as a token is involved:

```sh
# /etc/rescriptum.env   (chmod 600, owned by root)
RESCRIPTUM_ANSWERS_DIR=/srv/answers
RESCRIPTUM_LISTEN_ADDR=0.0.0.0:8000
RESCRIPTUM_TIMEOUT_SECS=10
# RESCRIPTUM_ANSWER_TOKEN=…
```

Under systemd, `EnvironmentFile=` below reads it and you need nothing else. Elsewhere —
and on [DSM 7](./synology.md), which has no systemd — point
[`RESCRIPTUM_ENV_FILE`](../reference/configuration.md#the-env-file) at the same file and
the binary reads it itself, refusing to start if it cannot.

## A systemd unit

```ini
# /etc/systemd/system/rescriptum.service
[Unit]
Description=rescriptum — per-machine answer files for unattended installs
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/rescriptum
EnvironmentFile=/etc/rescriptum.env
Restart=on-failure
RestartSec=2

# It needs to read one directory and bind one port. Nothing else.
DynamicUser=yes
ReadOnlyPaths=/srv/answers
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
ProtectHome=yes
ProtectKernelTunables=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6
SystemCallFilter=@system-service

[Install]
WantedBy=multi-user.target
```

```console
$ sudo systemctl enable --now rescriptum
$ curl -s http://localhost:8000/health
OK
```

Adjust for what you actually enable:

- **SQLite store** — the database needs to be writable, so `ReadWritePaths=/srv` and drop
  `ReadOnlyPaths`.
- **Request capture** — `ReadWritePaths=` the capture directory.
- **A port below 1024** — add `AmbientCapabilities=CAP_NET_BIND_SERVICE`.

Logs go to stderr, so `journalctl -u rescriptum -f` is the live view.

## In a container

There is nothing to install, so the image is the binary:

```dockerfile
FROM scratch
COPY rescriptum /rescriptum
ENV RESCRIPTUM_ANSWERS_DIR=/answers RESCRIPTUM_LISTEN_ADDR=0.0.0.0:8000
EXPOSE 8000
ENTRYPOINT ["/rescriptum"]
```

Use the build for the right architecture — the musl ones are statically linked, which is what
makes `FROM scratch` work. Mount the answers directory read-only.

## Sizing it

The defaults are already right for both ends of the range this was built for.

| Setting | Default | Change it when |
|---|---|---|
| `RESCRIPTUM_WORKERS` | CPU count | you are sharing a small box and want to cap runtime threads |
| `RESCRIPTUM_MAX_CONNECTIONS` | 2048 | you are seeing `503`s during a burst — or want to shed earlier |
| `RESCRIPTUM_TIMEOUT_SECS` | 10 | clients are on a slow link, or you want to cut off slowloris sooner |

`MAX_CONNECTIONS` is not a throughput limit. Over the cap the server writes a prompt `503`
and closes rather than queueing — a client that is told to retry is better off than one
parked in a queue that turns a burst into an out-of-memory.

A 2,000-machine rollout completes in under two seconds at the measured throughput, so
sizing is rarely the interesting problem. [Troubleshooting](./troubleshooting.md) usually
is.

## Replacing a running instance

```console
$ ./deploy.sh admin@nas
$ ./deploy.sh admin@nas /volume1/netboot        # a different remote directory
```

What it does, in order:

1. **Builds** for the target (`TARGET`, default `armv7-unknown-linux-gnueabihf`).
2. **Checks the local answers** with `rescriptum check` and refuses to continue if
   anything fails — shipping a broken answer set is worse than not deploying.
3. **Copies the binary under a temporary name**, then renames it into place. Replacing a
   running binary in place is how a half-copied file gets executed.
4. **Stops the running instance, starts the new one** detached, and confirms it stayed up.
5. **Confirms `/health` answers** over the network, so a firewall problem is reported as
   one rather than as a mysterious silence.

| Environment | Default |
|---|---|
| `TARGET` | `armv7-unknown-linux-gnueabihf` |
| `ANSWERS` | `<remote-dir>/answers` |
| `PORT` | `8000` |

It replaces what is running; it does not install autostart. On DSM that is a
[Task Scheduler entry](./synology.md#3-autostart); with systemd it is `systemctl enable`.

## Upgrading

Answers are data, not state: nothing is migrated, and a new binary reads the same
directory. Replace it and restart.

The exception is the SQLite store, which carries a schema version. There is one so far, so
there is nothing to migrate; what the version buys is the other direction, an **older**
binary refusing to open a database written by a newer one rather than guessing at it. See
[the SQLite store](./sqlite.md).
