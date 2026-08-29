---
title: Serve your first answer
description: From an empty directory to a machine receiving a document composed for it — five minutes, no ISO required.
sidebar:
  label: First answer
  order: 2
---

# Serve your first answer

Five minutes, one terminal, no installer needed. Everything here is testable offline:
`rescriptum render` resolves an answer exactly as the server would, so you can get the
answer right before any machine boots.

## 1. A directory and one document

```console
$ mkdir -p answers/groups/rack-a
```

**One directory per identity.** A directory at the top level is one machine, named after
it; `groups/` holds the shared ones. Inside either, the extension names the format and the
rest of the filename is just a label. Start with a group, since that is the shape almost
every real deployment ends up with — a rack of machines that agree about everything except
which disks they have:

```toml
# answers/groups/rack-a/proxmox.toml
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11"]

[global]
keyboard = "fr"
country = "fr"
timezone = "Europe/Paris"
root-password-hashed = "$6$rounds=656000$REPLACE$ME"

[network]
source = "from-dhcp"

[disk-setup]
filesystem = "zfs"
zfs.raid = "raid1"
disk-list = ["sda", "sdb"]
```

`members` lists the machines this group answers for. Separator style does not matter —
`98:fa:9b:50:d8:10`, `98-FA-9B-50-D8-10` and `98fa9b50d810` are one MAC, on both sides of
the comparison. `members` is rescriptum's key, not Proxmox's, and is stripped from what
the installer receives.

## 2. See what a machine would get

```console
$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum render 98:fa:9b:50:d8:11
# format=toml group=rack-a

[global]
keyboard = "fr"
country = "fr"
timezone = "Europe/Paris"
root-password-hashed = "$6$rounds=656000$REPLACE$ME"

[network]
source = "from-dhcp"

[disk-setup]
filesystem = "zfs"
zfs.raid = "raid1"
disk-list = ["sda", "sdb"]
```

The first line goes to stderr and says how the answer was reached — the format family,
which machine document matched, which group applied. The document itself goes to stdout,
so `render … > answer.toml` gives you just the document.

## 3. One machine that differs

The second node in the rack has four disks. It gets a directory named after its MAC,
holding a document with **only the difference** in it:

```toml
# answers/98-fa-9b-50-d8-10/proxmox.toml
[global]
fqdn = "node01.example.com"

[disk-setup]
zfs.raid = "raid10"
disk-list = ["sda", "sdb", "sdc", "sdd"]
```

```console
$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum render 98:fa:9b:50:d8:10
# format=toml machine=98-fa-9b-50-d8-10 group=rack-a

[global]
keyboard = "fr"
country = "fr"
timezone = "Europe/Paris"
root-password-hashed = "$6$rounds=656000$REPLACE$ME"
fqdn = "node01.example.com"

[network]
source = "from-dhcp"

[disk-setup]
filesystem = "zfs"
zfs.raid = "raid10"
disk-list = ["sda", "sdb", "sdc", "sdd"]
```

The group came first, the machine's own document on top, and **the machine won** wherever the two
disagreed. Tables merged key by key; `disk-list` was **replaced**, not appended — a list
that could only grow could never be shortened from a higher layer.

## 4. Check the whole set

```console
$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum check
checking files:answers
  1 group(s), 1 machine document(s)
  note: toml answers not schema-checked — proxmox-auto-install-assistant is not on PATH
  ok — everything renders
```

`check` renders every machine and every group member and reports whatever breaks: a
document that will not parse, a group extending one that does not exist, a placeholder
nothing can fill. Where the installer's own validator is on PATH it runs that too, and
says which formats it could not check.

This is the command to put in CI if your answers live in git.

## 5. Actually serve them

```console
$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum
2026-08-24T08:43:36Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=files:answers workers=10 max_conn=2048 timeout=10s
```

In another terminal, imitate what the Proxmox installer sends:

```console
$ curl -s -X POST http://localhost:8000/answer \
    -d '{"network_interfaces":[{"mac":"98:fa:9b:50:d8:10","link":"up"}]}'
```

and watch the server say what it did:

```
2026-08-24T08:43:37Z 127.0.0.1:61721 POST /answer body=102 200 format=toml machine=98fa9b50d810 group=rack-a bytes=431
```

That line is the whole diagnostic story when a rollout misbehaves: who asked, how big
their body was, what they got, and what it was built from.

New documents are picked up as you add them — no restart, no reload signal. A machine's
whole directory appearing or leaving is noticed at once; a document added or edited *inside*
one is picked up within a second.

## What to read next

- **[How an answer is picked](./answers/selection.md)** — you have seen naming and
  membership; selectors claim a machine by what it *is*.
- **[One document per operating system](./answers/formats.md)** — the same machine as
  Proxmox, as Debian, as Ubuntu, side by side.
- **[Templating](./answers/templating.md)** — `fqdn = "node-{{ serial }}.example.com"`,
  so one group covers a rack without a directory per machine.
- **[Preparing installer media](./iso.md)** — the URL to bake into the ISO, per OS.
