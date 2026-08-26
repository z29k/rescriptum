---
title: Security
description: Two tokens that deliberately behave differently, what they protect, and what neither of them does.
sidebar:
  label: Security
  order: 3
---

# Security

Answer documents carry `root-password-hashed` and `root-ssh-keys`. **Whoever can read
them can log into every machine you install; whoever can write them decides those
credentials.** That is the whole of the threat model, and it is worth being plain about.

## The answer endpoint is open by default

By default, anyone who can reach the port can fetch an answer. That is not an oversight:
**most installers have no credential to offer.** A kickstart client fetching
`inst.ks=http://…` has nothing to present, and refusing it would refuse the install.

The right primary control is the network. A provisioning VLAN that only PXE-booting
machines sit on is worth more than any token.

## `RESCRIPTUM_ANSWER_TOKEN`

Proxmox *can* present a credential when its ISO was prepared for it:

```console
$ proxmox-auto-install-assistant prepare-iso … --answer-auth-token 'a-long-random-string'
$ export RESCRIPTUM_ANSWER_TOKEN='a-long-random-string'
```

The installer then sends `Authorization: Bearer …`, and the server refuses anything
without it, comparing in constant time.

**Failures here are logged but never rate-limited.** A whole rack can sit behind one
address, and shutting that address out would turn one bad token into a failed rollout.
The [admin API](#the-admin-api-token), which no installer talks to, does lock out.

A token shorter than 16 characters is a startup **warning**, not an error — refusing to
start would leave a fleet unable to install.

`GET /health` stays open either way, so monitoring does not go dark.

## The admin API token

`RESCRIPTUM_ADMIN_TOKEN` is a different thing protecting a different surface, and it is
treated accordingly. The admin API sets the root password and SSH keys of every machine
installed afterwards, so:

- **it never shares the answer endpoint's listener** — its own `RESCRIPTUM_ADMIN_ADDR`;
- the server **refuses to start** without a token, with a token under 16 characters, or
  over the file store — errors, not warnings;
- an address that keeps guessing is **shut out**: five failures within a minute earn a
  block, doubling on repeats up to fifteen minutes, and the block applies to a *correct*
  token from that address too — otherwise guessing until you got it right would cost
  nothing.

Generate a real one. Not a word you thought of:

```console
$ openssl rand -hex 24        # or: head -c 24 /dev/urandom | base64
```

Full details on the [admin API page](./admin-api.md#looking-after-the-token).

## Why constant-time comparison

An ordinary `==` returns as soon as two bytes differ, so a wrong token sharing a longer
prefix takes measurably longer to reject. That difference is enough to recover a token one
byte at a time — a few thousand requests rather than an impossible number. Comparing every
byte regardless removes the signal.

Over a network the timing is usually lost in jitter, so this is precautionary. It costs
five lines.

## Do not put a token on a command line

Anything in a process's arguments is visible to every other user on the machine through
`ps`. That includes putting it directly in a DSM scheduled task. Keep it in a file only
root can read:

```sh
# /etc/rescriptum.env   (chmod 600, owned by root)
RESCRIPTUM_STORE=sqlite
RESCRIPTUM_DB_PATH=/srv/answers.db
RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
RESCRIPTUM_ADMIN_TOKEN=…
```

Then hand it to the server — `EnvironmentFile=/etc/rescriptum.env` under systemd, or
[`RESCRIPTUM_ENV_FILE=/etc/rescriptum.env`](../reference/configuration.md#the-env-file)
anywhere else. The second form makes the binary read it, so a file it cannot read is a
startup error rather than a server quietly running without the token you thought you had
set. The file is never discovered on its own — there is no `./.env`, deliberately: this
process runs as root, and a file picked up from the working directory would be a way to
hand someone the admin token.

## What the server refuses on its own

| | |
|---|---|
| **Path traversal** | a filesystem path is **never** built from request data. Only direct entries of the answers directory are read. Identifiers reaching the admin API accept letters, digits and `- _ . :` only, because `export` turns them back into filenames |
| **Oversized bodies** | an implausible `Content-Length` is refused from the header, before anything is read; the body is capped at 1 MB regardless |
| **Slow clients** | a header-read timeout **and** a whole-connection deadline, so a client that promises a body and sends nothing cannot park a connection |
| **Bursts** | over `RESCRIPTUM_MAX_CONNECTIONS` in flight, a prompt `503` and close, rather than queueing into an out-of-memory |
| **Malformed input** | a parse failure is an error response and a log line, never a panic that takes a connection — or a server — down mid-install |

## TLS

The server speaks plain HTTP. On a trusted provisioning network that is normally fine, and
it is what keeps the binary small and dependency-free.

If you need TLS — some installer versions want a certificate fingerprint when fetching
over HTTPS — terminate it in front with nginx or Caddy and point the ISO at that. The
answer endpoint does not care what is upstream of it.

The admin API is the one place where this matters by default: it speaks plain HTTP too, so
the token crosses the network in the clear. On loopback that is moot. Anywhere else, put a
TLS-terminating proxy in front.

## The desktop application

Only on Synology, and only there — the [DSM application](./synology.md#the-desktop-application)
is part of the package, not of the server. Its backend is a CGI that DSM serves from
`/webman/3rdparty/rescriptum/`, and two things about that path decide its whole security
model. **Both were measured on a DSM 7.2.2 machine rather than read in a guide, which does
not mention either:**

1. **A CGI there runs as the owner of the script.** DSM chowns a package's files to the
   package user, so the backend runs as `rescriptum` — the same identity that owns the
   `0600` env file and the log. That is what lets the application edit the configuration
   and read the log *while the server itself is stopped*, which is exactly when a settings
   panel earns its place. It is not root, and it cannot become anybody: it has no
   privilege to start or stop the package, which is why restarting goes through DSM's own
   API with the administrator's session instead. (A script left owned by root **does** run
   as root there. Worth knowing, and worth never doing.)
2. **DSM does not authenticate that path.** An unauthenticated request reaches the script
   and is answered. DSM protects its own pages; a package's are the package's problem.

Put together: the checks inside the script are the only thing in front of it, so it makes
three, in this order, before it touches anything.

- **A DSM session.** It runs DSM's own `authenticate.cgi`, which prints the signed-in
  user's name and prints nothing at all when there is no session.
- **An administrator.** Being signed in is not enough; the user must be in
  `administrators`. Anything less would let any account on the NAS set the root password of
  every machine it installs.
- **Intent, for a write.** A write must carry a header the application sends and a form on
  another site cannot: a browser will not send an invented header cross-origin without a
  preflight first, and this script answers no preflight. DSM's own `SynoToken` is sent
  along too, which is what keeps the application working with DSM's cross-site request
  forgery protection switched on.

`check-spk.sh` asserts that the first two are still in the script, and `lifecycle-test.sh`
drives the script with a stubbed authenticator to prove all three actually refuse. They
were watched failing: removing the session check turns four green into four red.

The application never receives a token. `RESCRIPTUM_ANSWER_TOKEN` and
`RESCRIPTUM_ADMIN_TOKEN` reach it as *set* or *not set* and nothing more — the command it
calls will not print a credential, whatever it is asked.

## Known and accepted

- **Per-address rate limiting does not stop an attacker with many addresses.** The admin
  token's *length* is what makes guessing hopeless — hence the 16-character floor.
- **The answer endpoint is not rate-limited at all**, deliberately, for the reason above.
- **Binding the admin API beyond loopback is your call**, and the server says so in the
  log when you do. `127.0.0.1` plus an SSH tunnel is the safe default.
