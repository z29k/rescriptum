---
title: Powering machines on
description: Telling a machine to boot from the network and turn on, over Redfish or through a script you supply — and the one command that checks everything before anything moves.
sidebar:
  label: Powering machines on
  order: 10
---

# Powering machines on

Everything else here answers machines that ask. This is the one part that *emits*: it
tells a BMC to arm a network boot and press the power button, so an install can be started
from a terminal instead of from a chair in front of the rack.

It is **off unless `RESCRIPTUM_CONTROLLERS_FILE` names a file**. Unset, there are no
credentials, no outbound connections and no code path that reaches any. A deployment that
wants a pure answer server gets exactly what it had.

It is also **synchronous and operator-triggered, always**. Nothing here reconciles, retries
or decides on its own that a machine should be reinstalled. Every action is a person or a
script, once.

## The controllers file

A TOML file, keyed by the same identifier the answers directory uses — so
`98:fa:9b:50:d8:10` and `98fa9b50d810` are one machine on both sides.

```toml
# mode 0600. Not in the answers directory: it is a .toml, and every servable .toml at the
# top of that directory is an answer document.

["98-fa-9b-50-d8-10"]
kind   = "redfish"
url    = "https://10.0.0.51"      # scheme and host only — a path belongs in `base`
base   = "/redfish/v1"            # PiKVM serves "/api/redfish/v1"
user   = "root"
pass   = "…"
pinnedpubkey = "sha256//…"        # or cacert = "…", or verify = false. One is required

["aa-bb-cc-dd-ee-ff"]
kind    = "command"               # anything Redfish cannot reach
on      = ["/usr/local/bin/pdu", "outlet", "7", "on"]
off     = ["/usr/local/bin/pdu", "outlet", "7", "off"]
pxe     = []                      # nothing to do — the boot order is permanently network
timeout = 30                      # seconds; a hung script would otherwise hang `install`
```

Four rules are worth knowing before you write one.

**The server never reads this file.** `power` and `install` do. A malformed credentials
file cannot stop the answer listener, because a fleet's installs going down for a reason
unrelated to answering is exactly the failure that would not be worth this feature.

**Say how the certificate is to be trusted.** An entry carrying none of `verify = false`,
`cacert` or `pinnedpubkey` is refused, naming all three. BMCs ship self-signed
certificates, so "do not verify" is the *convenient* default and therefore not the one you
get — the same rule `media add` already has, where a URL requires `--sha256` unless
`--unverified` is passed. `pinnedpubkey` is the right answer for a self-signed BMC and
costs nothing.

**A group-readable file is refused at use.** Not warned about: this one holds credentials
that can power-cycle a rack. `chmod 600` it. (Note that mode bits under-report on DSM,
where an ACL can grant access `st_mode` never mentions — so this is a check on the mode,
not a proof of privacy.)

**`on`, `off` and `pxe` are argument vectors, never command lines.** Nothing is passed
through a shell, no word splitting happens, and nothing a machine sent over the network can
reach them. A string there is refused with an explanation rather than split.

## The commands

```bash
rescriptum power list             # what is configured, joined to the answer set
rescriptum power list --state     # ...and ask each one whether it is on
rescriptum power status <id>
rescriptum power on <id>
rescriptum power off <id>         # graceful; --hard forces it
rescriptum power pxe <id>         # arm a one-time network boot, where there is one
rescriptum install <id>           # check, arm, pxe, power on — the whole gesture
rescriptum install <id> --dry-run # everything except the powering
```

`power list` **does not probe**. Reading state is one HTTPS round trip per controller, each
up to its deadline; with two hundred controllers and a handful unreachable, a listing that
asked would take minutes and look hung. `--state` is the version that asks, and it is
bounded and concurrent.

## `install`, and what it refuses

`install` is the command the rest exists for, and most of it is checking. Powering on a
machine that network-boots, chains the installer and then meets a 404 leaves an installer
sitting at a prompt in a rack — which is the failure this whole project exists to prevent.

In order:

1. **Every format this machine resolves for renders**, templates filled, no missing fact.
   Not a guess at which one the boot script leads to — all of them.
2. **The policy is checked**, and this is where it most often stops. See below.
3. **Its boot script is put back**, if a previous install archived it into an
   `installed-<id>/` sibling. Your own document comes back byte for byte.
4. **A one-time network boot is armed**, where the controller has one — and **read back to
   confirm it took**.
5. **It is powered on, or restarted**, decided by reading the current power state.

Three refusals, each for a different reason:

| It says | Because |
|---|---|
| *nothing arms it, so it would sit on the boot menu* | With `RESCRIPTUM_BOOT_UNCLAIMED=menu`, an unarmed machine waits for a human who is not coming, and burns a boot cycle |
| *…would boot its own disk and report nothing* | With `local`, the same machine looks **exactly like a successful install**. That is the dangerous one |
| *its boot script comes from a group, and a group is never disarmed* | See below |

### Why a group cannot arm an install

`POST /installed` moves a **machine's own** `.ipxe` aside when it reports success — never a
group's, deliberately, so that one machine finishing cannot disarm a whole rack.

The consequence is easy to miss. A machine armed only by its group installs, reports
success, is not disarmed, and finds the same boot script waiting on its next network boot.
With a permanently network-first boot order that is a reinstall loop, and the webhook logs
`nothing was claiming it`, which reads like everything worked.

So `install` refuses it, `check` reports it as a note, and the fix is to give the machine
its own `.ipxe` document. Keep a group `.ipxe` for what is meant to be served forever —
booting the local disk, or a menu.

## What each kind of controller can do

| Controller | Power | One-time network boot | Notes |
|---|---|---|---|
| Server BMC (iDRAC, iLO, generic Redfish) | yes | **yes** | |
| PiKVM | yes | **no** — it presses buttons, it is not the firmware | Its `PATCH` answers `204` and changes nothing; the read-back catches that |
| JetKVM, switched PDU, Wake-on-LAN | yes | no | Through `kind = "command"` |
| Intel AMT | yes | yes | See the trap below |

**A missing boot override is not a gap.** Where one does not exist, leave the boot order on
the network permanently and let the server decide whether the machine installs — which is
exactly what `RESCRIPTUM_BOOT_UNCLAIMED` and the `installed-` disarm already do. A PiKVM
plus rescriptum is a complete solution; a BMC with one-time boot is belt and braces.

## Things that will bite

**A timeout is not a failure — it is an unknown.** A reset that timed out may have powered
the rack on. Nothing here retries a write automatically, and the message says the outcome
is unknown rather than implying nothing happened. Read the state back with `power status`.

**There is no TLS in this binary**, so Redfish calls go through `curl`. Unlike `media add`,
there is no `wget` fallback: a Redfish call needs a POST with a JSON body, custom headers
and a credential kept out of the process table, and wget does none of that combination. The
credential is passed on curl's stdin, so `ps` shows only `curl --config -`.

**A BMC in front of several systems is refused rather than guessed at.** A blade chassis, a
Dell FX2, and a PiKVM with a switch all expose more than one; picking the first would power
somebody else's machine. Add `system = "…"` to the entry to say which.

**Intel AMT on a shared NIC can starve the host's DHCP.** With the Management Engine holding
the interface on a static address while the host asks for a lease, the Proxmox installer's
`dhclient` gives up after about eleven seconds and the install aborts on
`Network is unreachable` — while `dhclient -v eno1` from the installer's own shell succeeds
instantly afterwards. Set AMT to DHCP. Nothing here can widen that window.

## What this is not

- **Not a configuration manager.** The boundary is the moment SSH answers. That is
  Ansible's ground.
- **Not a reconciliation loop.** No agent decides a machine should be reinstalled.
- **Not a fan-out.** There is no group form, and if one is ever added it will be sequential
  with a settable delay — powering forty machines at once is an electrical event before it
  is a software one, and datacenters stagger power-on for inrush current.
- **Never a request.** No HTTP endpoint powers anything. The answer endpoint is
  unauthenticated by necessity, and wiring power control anywhere near it is how a
  provisioning server becomes a weapon.
