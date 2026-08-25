---
title: Capturing requests
description: Record what machines actually send, then replay it offline with render --body.
sidebar:
  label: Capturing requests
  order: 4
---

# Capturing requests

Most of what rescriptum knows about installers comes from their documentation. Until a
real installer has talked to it, that is a claim rather than a fact — and when a rollout
misbehaves, *"what did node07 actually send?"* is usually the only question worth
answering.

```console
$ export RESCRIPTUM_CAPTURE_DIR=/var/log/rescriptum-captures
```

Off unless set.

## What it writes

Two files per request:

```
20260824T084337Z-10.0.0.42-0000.body     the body, verbatim
20260824T084337Z-10.0.0.42-0000.meta     who asked, and what they got
```

```
time: 2026-08-24T08:43:37Z
peer: 10.0.0.42:51234
request: POST /proxmox/answer
body-bytes: 1876
outcome: 200 format=toml machine=98fa9b50d810 group=rack-a
```

The `.body` is **byte-for-byte what arrived**, so it replays unchanged. The filename
carries the timestamp, the peer address (sanitised — an IPv6 peer's colons do not belong
in a filename) and a sequence number, so two requests in the same second do not collide.

## Replaying one

```console
$ rescriptum render --body /var/log/rescriptum-captures/20260824T084337Z-10.0.0.42-0000.body
```

That resolves exactly as the server did, offline, with no machine involved — which is what
makes a bad answer debuggable at your desk instead of in front of a rack.

It is also the best way to build selectors against a body format you have not seen:
capture one real request, then iterate with `render --body` until it resolves the way you
meant.

## The limits, and why

- **Capped at 1000 captures.** A provisioning server that fills its own disk is worse than
  one that captures nothing. On reaching the cap it logs once and stops writing. The count
  is of captures, not files, and it survives a restart: the server counts what is already
  in the directory before it writes anything.
- **Nothing is ever deleted.** Rotating or clearing the directory is yours to do; the
  server counts what is already there at startup so a restart does not blow past the cap.
- **A capture failure never fails a request.** It is logged, and the install carries on.
  Losing a diagnostic is not worth losing an install.

## Before you attach one to a bug report

A captured body is a hardware inventory: MAC addresses, disk serials, DMI. The `.meta`
says which answer it received. Neither contains your password hashes — but the *answer*
does, so scrub anything you paste alongside it.

## Related

- [Troubleshooting](./troubleshooting.md) — reading the log, and the usual causes.
- [Validating](../answers/validating.md) — `render` in its other forms.
