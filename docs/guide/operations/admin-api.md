---
title: The admin API
description: Manage answers over HTTP — on its own listener, over SQLite only, with a write that can never leave the answer set broken.
sidebar:
  label: Admin API
  order: 6
---

# The admin API

With `RESCRIPTUM_STORE=sqlite`, answers can be managed over HTTP instead of by editing
files. It is **off unless you configure it**, and it runs on its own listener.

```console
$ export RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db
$ export RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
$ export RESCRIPTUM_ADMIN_TOKEN=$(openssl rand -hex 24)
$ rescriptum
2026-08-24T08:52:30Z - admin API listening on 127.0.0.1:8001
2026-08-24T08:52:30Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=sqlite:/srv/answers.db …
```

## Three properties that are load-bearing

**1. Its own listener.** The answer endpoint is unauthenticated by necessity — the
installer has no credentials to offer. This API sets the root password and SSH keys of
every machine installed afterwards. It never shares that port.

**2. SQLite only.** Over a directory of files there would be two ways to change the same
configuration, by hand and over the wire, racing each other.

**3. A write can never leave the answer set broken.** Every write snapshots the current
problems, applies itself, and compares. Anything **newly** broken is rolled back and
answered `409`.

The server **refuses to start** — as an error, not a warning — if you point the admin API
at the file store, leave the token out, or set a token shorter than 16 characters.

## Endpoints

| Request | Does |
|---|---|
| `GET /machines`, `GET /groups` | list identifiers |
| `GET /machines/{id}`, `GET /groups/{name}`, `GET /default` | the stored document, **as written** — comments and formatting intact |
| `PUT /machines/{id}`, `PUT /groups/{name}`, `PUT /default` | store a document (the body is the document) |
| `DELETE /machines/{id}`, `DELETE /groups/{name}`, `DELETE /default` | remove one |
| `GET /resolve/{id}` | the **merged** answer that machine would receive |
| `GET /check` | current problems, the same set as the `check` subcommand |
| `GET /health` | liveness — the only endpoint needing no token, and never blocked |

Every endpoint that names a document takes **`?format=`** — the extension the document is
in. It defaults to `toml`, which is what this server started life serving:

```console
$ curl -H "$AUTH" -X PUT --data-binary @base.preseed \
    'http://127.0.0.1:8001/groups/base?format=preseed'
```

Because a document's key is [(identifier, format)](../answers/formats.md), an identifier
appears in `GET /machines` **once per format it exists in** — a machine that is both a
Proxmox node and a Debian node is listed twice.

## Examples

```console
$ AUTH="Authorization: Bearer $RESCRIPTUM_ADMIN_TOKEN"

$ curl -s -H "$AUTH" http://127.0.0.1:8001/groups
{"group":["base","example-rack","rhel-compute","ubuntu-web"]}

$ curl -s -H "$AUTH" -X PUT --data-binary @rack-a.toml \
    http://127.0.0.1:8001/groups/rack-a
{"status":"stored","problems":[]}

$ curl -s -H "$AUTH" http://127.0.0.1:8001/resolve/98:fa:9b:50:d8:10
[global]
country = "fr"
keyboard = "fr"
…
```

`GET /resolve` also answers the response header **`X-Answer-Source`**, carrying the same
description the log line uses:

```
x-answer-source: format=toml machine=98fa9b50d810 group=example-rack
```

### Rehearsing a real request

`GET /resolve` accepts the same labels a real request would carry, so you can rehearse a
particular URL — the difference between `/user-data` and `/meta-data`, for instance:

```console
$ curl -s -H "$AUTH" 'http://127.0.0.1:8001/resolve?path=/rhel/ks&serial=7ABC123'
```

**When a query string is present, the identifier in the path is ignored** — the facts come
from the query alone. So `GET /resolve/98:fa:9b:50:d8:10?format=toml` resolves *nothing*,
because `format=toml` is not an identity. Use the bare path form, or put the identity in
the query: `?mac=98:fa:9b:50:d8:10`.

## It will not let you break the fleet

Every write is checked after it is applied. If it introduced a problem — a cycle between
groups, a document referring to a group that no longer exists — the write is **rolled
back** and you get a `409` saying what you broke:

```console
$ curl -s -H "$AUTH" -X DELETE 'http://127.0.0.1:8001/groups/base?format=preseed'
{"error":"refused: this would break the answer set (rolled back)",
 "problems":["machine \"98fa9b50d810\": extends unknown group \"base\""]}
```

Two things follow from how this works:

- **A successful write still reports any *pre-existing* problems**, in the `problems`
  array. A clean response never implies the whole set is healthy — only that you did not
  make it worse.
- It is why a machine's `extends` pointing at a missing group is detected at **load** time
  rather than only when that machine asks. The guard can only catch what the problem
  report knows about.

Malformed documents are refused at write time too, rather than becoming a `500` the next
time a machine asks:

```console
$ curl -s -H "$AUTH" -X PUT --data-binary 'x = = 1' http://127.0.0.1:8001/machines/aa-bb-cc-dd-ee-01
{"error":"document: invalid TOML: TOML parse error at line 1, column 5 …"}
```

## Identifiers

Letters, digits and `- _ . :` only. They are written out as **filenames** by `export`, so
anything that could traverse a directory is rejected — at the API boundary *and* in both
stores.

## Status codes

| Code | Means |
|---|---|
| `200` | done |
| `400` | a malformed document, or an invalid identifier |
| `401` | missing or wrong token |
| `404` | no such document, or nothing resolves for that identifier |
| `409` | a write that would have broken the answer set (rolled back), or a `resolve` that could not render |
| `413` | document over 256 KB |
| `429` | this address is blocked after repeated authentication failures |
| `500` | the store could not be read or written |

## Looking after the token

The token is the whole of the authentication, and what it protects is worth saying
plainly: answer documents carry `root-password-hashed` and `root-ssh-keys`, so **whoever
can write to this API decides the root credentials of every machine you install
afterwards**.

Generate a real one — not a word you thought of:

```console
$ openssl rand -hex 24        # or: head -c 24 /dev/urandom | base64
```

**Do not put it on a command line.** Anything in a process's arguments is visible to every
other user through `ps`, which includes putting it directly in a DSM scheduled task. Keep
it in a root-only file and source it — see [Security](./security.md#do-not-put-a-token-on-a-command-line).

What the server does on its side:

- **Compares the token in constant time**, so it cannot be recovered a byte at a time by
  whoever is timing the responses.
- **Shuts out an address that keeps guessing.** Five failures within a minute earn a
  block, doubling on repeats to a maximum of fifteen minutes, and every attempt is logged.
  The block applies to a **correct** token from that address too — otherwise guessing
  until you got it right would cost nothing.
- **Bounds its own bookkeeping** to 4096 tracked addresses, so the guard cannot itself be
  turned into a memory leak.
- **Leaves `GET /health` unauthenticated and unblocked**, so monitoring does not go dark
  during an attack.

```
2026-08-24T08:52:32Z - admin: 10.0.0.9 failed authentication 5 times — blocked for 60s
```

## Two limits to plan around

- **It speaks plain HTTP**, so the token crosses the network in the clear. On loopback
  that is moot. Anywhere else, put a TLS-terminating reverse proxy in front.
- **Per-address blocking does not stop an attacker with many addresses.** The token's
  length is what makes guessing hopeless — hence the 16-character floor at startup.

Binding it beyond loopback is your call, and the server says so in the log when you do:

```
2026-08-24T08:52:30Z - warning: the admin API is not bound to loopback — it rewrites what gets installed on every machine, so restrict it to a management network
```

`127.0.0.1` plus an SSH tunnel is the safe default.

## Related

- [The SQLite store](./sqlite.md) — the prerequisite.
- [How the guard is built](../../development/admin.md) — the internals.
