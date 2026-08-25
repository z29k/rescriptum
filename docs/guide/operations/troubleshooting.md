---
title: Troubleshooting
description: The log line is the whole diagnostic story. What each failure means, and how to reproduce it offline.
sidebar:
  label: Troubleshooting
  order: 7
---

# Troubleshooting

When a PXE install will not start, the log is the only diagnostic anyone has — so it is
deliberately boring and greppable: **one line per request, on stderr**. Both halves are
configurable: [`RESCRIPTUM_LOG`](../reference/configuration.md#logging) drops the requests
that worked, and `RESCRIPTUM_LOG_FILE` sends the lines to a file instead.

```
2026-08-24T08:43:36Z - rescriptum 0.1.0 listening on 127.0.0.1:8999 — store=files:answers workers=10 max_conn=2048 timeout=10s
2026-08-24T08:43:37Z 127.0.0.1:61720 GET /health 200
2026-08-24T08:43:37Z 127.0.0.1:61721 POST /answer body=102 200 format=toml machine=98fa9b50d810 group=example-rack bytes=431
2026-08-24T08:43:37Z 127.0.0.1:61722 GET /rhel/ks?serial=7ABC123 body=0 200 format=text group=rhel-compute bytes=747
2026-08-24T08:43:37Z 127.0.0.1:61723 POST /answer body=27 404 no answer file applies
```

## Reading a line

```
2026-08-24T08:43:37Z 127.0.0.1:61721 POST /answer body=102 200 format=toml machine=98fa9b50d810 group=example-rack bytes=431
└─ UTC timestamp     └─ peer         └─ request      └─ body   └─ status
                                                                  └─ how the answer was composed  └─ bytes sent
```

Lines with `-` instead of a peer address are server-level: startup, accept failures,
shedding, load-time problems with the answer set.

`format=…` names the **family** (`toml`, `yaml`, `json`, `xml`, `text`) rather than the
extension — `ks` and `preseed` both report as `text`.

## Common failures

| Symptom | Likely cause |
|---|---|
| `404 no answer file applies` | Nothing claimed the request and there is no `default` for the format asked for. [Capture the body](./capture.md) and check the MAC really is in it |
| `404` on a URL that used to work | The URL now names a [format alias](../reference/formats.md) that excludes your document — `/ubuntu/answer` will not serve a `.toml` |
| `500 … extends unknown group` | A document references a group that does not exist. Deliberate: serving a configuration whose base is missing would install the machine half-configured |
| `500` on one machine only | That machine's document, or its group, will not parse. The reason is on the same log line. `rescriptum check` finds it without waiting for the machine to ask |
| `500 template needs {{ … }}` | A [placeholder](../answers/templating.md) the request could not fill. Never served as an empty string, on purpose |
| `401 bad or missing token` | `RESCRIPTUM_ANSWER_TOKEN` is set but the ISO was not prepared with the same `--answer-auth-token` |
| `413` | A body over 1 MB, or a `Content-Length` claiming one. Refused from the header, before anything is read |
| `503` | More concurrent connections than `RESCRIPTUM_MAX_CONNECTIONS`. Raise it, or find out who is connecting |
| Answer served, install still fails | The document is valid TOML but not valid *Proxmox*. Pipe [`render`](../answers/validating.md) into `validate-answer` |
| Installer never contacts the server at all | The ISO's URL or a firewall, not this server. `curl http://SERVER:8000/health` from the same network |

## The server starts but everything 404s

Check the startup line. The two usual causes announce themselves:

```
warning: /srv/answers does not exist yet — every request will 404 until it does
```

```
warning: /srv/answers cannot be read: Permission denied (os error 13) — every request
will 404 until that is fixed; check the directory's owner against the user this server
runs as
```

That second one is what you get when the directory exists but the process cannot list it —
the usual cause is a directory created as root and a server running as somebody else. It
is asked of the filesystem rather than read off the permission bits, so it accounts for the
owner, the group, ACLs and the mount.

```
… store=files:/srv/answers …
```

— that second one is the *default* answers directory. If you meant a different one,
`RESCRIPTUM_ANSWERS_DIR` did not reach the process. An empty or
whitespace-only value is treated as unset, and a zero or unparseable number falls back to
its default.

## Reproducing it offline

This is the fastest route from "a machine got the wrong thing" to a fix:

```console
$ export RESCRIPTUM_CAPTURE_DIR=/var/log/rescriptum-captures   # then let it fail once more
$ rescriptum render --body /var/log/rescriptum-captures/2026…-0000.body
```

`render` resolves exactly as the server does, so whatever it prints is what that machine
would have received. No rack required. See [capturing requests](./capture.md).

If you have no capture, rehearse from the identity and the URL:

```console
$ rescriptum render --query "path=/rhel/ks&mac=98:fa:9b:50:d8:10&serial=7ABC123"
```

Add `path=` — without it, resolution is unconstrained by format and may pick a document
the real URL would have excluded, which is exactly the bug you might be chasing.

## Checking the whole set

```console
$ rescriptum check
```

Load-time problems, every machine and group member rendered, and the installer's own
validator run where it is on PATH. Details in
[validating](../answers/validating.md#check--render-everything-report-what-breaks).

## Reporting something

For a wrong answer, the useful report is **what the machine sent and what it got back** —
the `.body` and `.meta` from a capture, plus the log line.

**Scrub the password hashes and SSH keys** before attaching anything: the answer is in the
capture's `outcome` only by name, but if you paste the rendered document too, it carries
real credentials.

[Open an issue](https://github.com/z29k/rescriptum/issues) with those and the version from
the startup line.
