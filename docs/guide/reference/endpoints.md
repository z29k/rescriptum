---
title: HTTP surface
description: Methods, paths, status codes, headers and limits — for both listeners.
sidebar:
  label: HTTP surface
  order: 2
---

# HTTP surface

Two listeners, and they never share a port. The answer endpoint is what installers talk
to; the [admin API](../operations/admin-api.md) is off unless configured.

## The answer endpoint

| Request | Response |
|---|---|
| `POST` **any path** | the answer, content-typed by its format |
| `GET` **any path** | the same |
| `GET /health` | `200 OK`, body `OK\n` — no token needed, never rate-limited |
| any other method | `405` |

**Any path**, because the URL is baked into an ISO and this server does not get to choose
it. The path is not ignored, though: segments naming a
[format alias](./formats.md) restrict which documents may answer, and the path also
contributes the `path`, `file` and `segment`
[facts](../answers/selection.md#the-facts-a-selector-can-test).

### Status codes

| Code | When |
|---|---|
| `200` | an answer applied |
| `400` | the body could not be read |
| `401` | `RESCRIPTUM_ANSWER_TOKEN` is set and the request did not present it |
| `404` | nothing claimed the request and there is no `default` for the format asked for |
| `405` | a method other than `GET` or `POST` |
| `413` | body over 1 MB, or a `Content-Length` claiming one |
| `500` | a document would not parse, a group is missing, a template could not be filled, or the lookup panicked |
| `503` | at `RESCRIPTUM_MAX_CONNECTIONS` — written promptly, then the connection closes |

### Response headers

| Header | Value |
|---|---|
| `Content-Type` | from the answer's format — see the table below |
| `Content-Length` | always set |
| `Connection` | `close` |
| `WWW-Authenticate` | `Bearer`, on `401` |

| Format | `Content-Type` |
|---|---|
| `toml`, and every text format (`ks`, `preseed`, `cfg`, `seed`, `ipxe`) | `text/plain; charset=utf-8` |
| `yaml`, `yml` | `text/yaml; charset=utf-8` |
| `json`, `ign` | `application/json` |
| `xml`, `autoyast`, `unattend` | `application/xml; charset=utf-8` |

TOML is served as `text/plain` rather than `application/toml` because that is what the
Proxmox installer expects.

### Request handling limits

| | |
|---|---|
| **Body cap** | 1 MB. An implausible `Content-Length` is refused **from the header**, before the body is read at all — rather than allocating for it and tripping a limit later |
| **Header-read timeout** | `RESCRIPTUM_TIMEOUT_SECS`, default 10 s |
| **Whole-connection deadline** | the same value. Both are needed: the header timeout stops at the end of the headers, so a client that promises a body and sends nothing would otherwise park a connection indefinitely |
| **Concurrency** | `RESCRIPTUM_MAX_CONNECTIONS` in flight; over that, a `503` and close rather than queueing |
| **Authentication** | only when `RESCRIPTUM_ANSWER_TOKEN` is set. Compared in constant time. Failures are logged and **never** rate-limited |

### Authentication

```
Authorization: Bearer <RESCRIPTUM_ANSWER_TOKEN>
```

Proxmox sends this when its ISO was prepared with `--answer-auth-token`. Nothing else
can, which is why it is off by default. See [Security](../operations/security.md).

## The admin API

A separate listener, `RESCRIPTUM_ADMIN_ADDR`, over SQLite only. Full details on
[its own page](../operations/admin-api.md).

| Request | Does |
|---|---|
| `GET /machines`, `GET /groups` | list identifiers |
| `GET /machines/{id}`, `GET /groups/{name}`, `GET /default` | the stored document, as written |
| `PUT /machines/{id}`, `PUT /groups/{name}`, `PUT /default` | store a document |
| `DELETE /machines/{id}`, `DELETE /groups/{name}`, `DELETE /default` | remove one |
| `GET /resolve/{id}` | the merged answer that machine would receive |
| `GET /check` | current problems |
| `GET /health` | liveness — no token, never blocked |

All document endpoints take `?format=<ext>`, defaulting to `toml`.

| Code | When |
|---|---|
| `200` | done |
| `400` | malformed document, invalid identifier, or a non-UTF-8 body |
| `401` | missing or wrong token |
| `404` | no such document or endpoint; nothing resolves for that identifier |
| `409` | the write would have broken the answer set (rolled back), or a `resolve` that could not render |
| `413` | document over 256 KB |
| `429` | this address is blocked; `Retry-After` says for how long |
| `500` | the store could not be read or written |

Every admin response sets `Connection: close`. Successful `GET /resolve` also sets
`X-Answer-Source`, carrying the same description the log line uses.

## Logging

One line per request, on **stderr** by default. `RESCRIPTUM_LOG` chooses what is kept and
`RESCRIPTUM_LOG_FILE` chooses where it goes — see [logging](./configuration.md#logging).

```
2026-08-24T08:43:37Z 127.0.0.1:61721 POST /answer body=102 200 format=toml machine=98fa9b50d810 group=example-rack bytes=431
```

Server-level lines carry `-` where the peer address would be. See
[troubleshooting](../operations/troubleshooting.md#reading-a-line).
