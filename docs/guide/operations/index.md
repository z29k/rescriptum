---
title: Running it
description: Deployment, security, storage, and reading the log when a rollout misbehaves.
sidebar:
  label: Running it
  order: 5
  indexLabel: Overview
---

# Running it

rescriptum is one process, configured entirely through the environment, that writes
nothing outside the store you point it at. Running it well is mostly about deciding what
it is allowed to serve and to whom.

- **[Deployment](./deployment.md)** — a systemd unit, a container, or nothing at all.
- **[Synology DSM 7](./synology.md)** — the original target: no systemd, so autostart is
  a Task Scheduler entry.
- **[Security](./security.md)** — the two tokens, why they behave differently, and what
  neither of them protects.
- **[Capturing requests](./capture.md)** — record what machines actually send, and replay
  it offline.
- **[The SQLite store](./sqlite.md)** — for a fleet administered by tooling rather than
  by hand.
- **[The admin API](./admin-api.md)** — manage answers over HTTP, on its own listener,
  with a write that cannot break the fleet.
- **[Troubleshooting](./troubleshooting.md)** — the log line is the whole diagnostic
  story.

## The shape of a deployment

| | |
|---|---|
| **One process** | no supervisor tree, no workers to size, no sidecar |
| **One port** by default | plus a second, only if you enable the admin API |
| **No writes** | outside the answers directory or database, and none at all unless you enable the admin API or request capture |
| **No state** | between requests. Restarting loses nothing |
| **Graceful shutdown** | on SIGTERM (what DSM's scheduler sends) and Ctrl-C |

Configuration is [environment variables](../reference/configuration.md) only. A zero or
unparseable numeric value falls back to its default rather than starting a server that
accepts connections and never answers.

## What it needs from the network

The installer has to reach it, and that is all. It makes no outbound connections, needs no
DNS, and does not care whether it is behind NAT.

Plain HTTP is the normal choice on a provisioning network. If you need TLS — some
installer versions ask for a certificate fingerprint — terminate it in front with nginx or
Caddy and point the ISO at that. See [Security](./security.md#tls).
