# The boot rig

Everything from a DHCP offer to a machine sitting on its own disk, in one command, on a
network that is its whole world.

```console
$ packaging/boot-rig/run.sh
```

**It produces no features and it is not optional.** Everything built on top of the boot
chain depends on knowing the chain works, and this is what keeps that known: from here
on, every push can re-prove the BIOS path without a single real machine.

## What it proves, and what it cannot

| Where | What it proves |
|---|---|
| `cargo test` | protocol and logic: TFTP over real UDP, ranges over real sockets, the ISO reader against synthetic images, snippet ↔ loader-table coherence |
| **This rig** | the chain end to end — a DHCP offer, a loader over TFTP, a script, a menu, and a machine that lands where it should |
| A real machine | that firmware agrees. **Nothing ships on rig evidence alone** |

The third row is not a formality. The rig runs one emulator with one NIC model; the
failures it cannot see are exactly the ones firmware has — a ROM that reads only the
BOOTP `file` field, a UEFI build that cannot see its own network card, an option 93 value
nobody expected.

## The two markers

Both deterministic, neither a screenshot.

**An unclaimed machine must reach its own disk.** It gets a disk whose only content is a
512-byte boot sector that prints `RESCRIPTUM-RIG-LOCAL-DISK-REACHED` to the serial console
and halts ([`local-disk.asm`](local-disk.asm)). Reaching it means the machine went through
DHCP, the loader, the bootstrap and the menu, found nothing claiming it, waited out the
timeout, and fell through — which is the safety behaviour the whole design rests on. A
machine that PXE-boots by accident must never sit at a menu forever, and must never
install anything.

**A claimed machine must reach its own answer.** Its `.ipxe` answer ends by fetching a
sentinel URL, so the assertion is a line in the **server's own log** rather than something
the client printed. That is the stronger form: it proves the request arrived.

And a third, which comes free: **dnsmasq answered from the snippet we generate.** The rig
builds its DHCP configuration by running `rescriptum boot dhcp-snippet`, so if what we
tell operators to paste is wrong, nothing here boots.

## The shape, and why

**One container**, holding the loaders, the server, dnsmasq and a QEMU machine on a
private bridge with no uplink. Nothing crosses Docker's network, which means nothing can
be filtered by it — the rig's network really is its whole world.

[`run-compose.sh`](run-compose.sh) is the four-service variant the plan describes:
loaders, server, dnsmasq and client as separate containers on a Docker network with
`internal: true`. It is the more honest shape and it works on a Linux host. **It does not
work on Docker Desktop**, and the reason is worth recording because it is not obvious: a
QEMU guest bridged into a container has a MAC of its own, and Docker Desktop's virtual
switch does not forward frames from a MAC it did not assign.

That was measured rather than assumed. Container-to-container TCP works (a SYN and its
SYN-ACK captured on the receiving side); a DHCP broadcast from the guest reaches nothing
at all — `tcpdump` on the DHCP container captures zero packets while the client's own
`tap0` and `eth0` counters show the frames leaving. The guest's MAC even turns up in the
bridge's forwarding table on the *wrong* port.

**No `/dev/kvm` in either.** KVM would make this fast; the rig has to pass without it,
because the development machine is a Mac. It needs `NET_ADMIN` to build the bridge and
`/dev/net/tun` for the tap the guest sits on, and nothing else — no host network, no
published port.

## What running it for the first time cost

Four attempts, and every failure was invisible until the thing actually ran. They are
listed because each one is a shape that will recur, not because the fixes are
interesting:

| What failed | Why it was invisible |
|---|---|
| `iproute2` missing from the client image | every `ip` call tolerated its own failure, and the guard below them never fired. Five `command not found` lines scrolled past and QEMU booted with no network |
| The client image was never rebuilt | `docker compose run` reuses whatever image exists, so a Dockerfile fix silently did not apply |
| dnsmasq installed at *run* time | the network is `internal: true`, so apt could not reach anything. The install failed into `/dev/null` and the container exited 127 a minute before a client was booted at it |
| A 6 GB build context | no `.dockerignore`, so every run spent two minutes transferring `target/` |

The pattern in three of the four: **a service that died looks exactly like one still
starting.** `run.sh` now checks every container is still running before it boots a
client, and prints the log of any that is not.

The rule the third one broke is the rig's own, stated two paragraphs above it: if
something on this network needs the internet, it gets it before the network exists.

## Two host facts worth knowing before the first run

- **The loader image is pinned to `linux/amd64`.** iPXE's BIOS targets are 32-bit x86 and
  its `ipxe.efi` here is x86-64; a compiler on an ARM64 host produces neither, and the
  failure is a wall of `unrecognized command-line option '-m32'` that reads like a broken
  Makefile. On Apple Silicon that image builds under emulation, which is slow and works.
- **ARM64 loaders need `gcc-aarch64-linux-gnu`.** Without it `build.sh` says which package
  is missing and carries on with the x86 loaders, rather than failing with
  `unrecognized command-line option '-mlittle-endian'` from the host compiler.

## Running it

```console
$ packaging/boot-rig/run.sh          # both clients, BIOS
$ packaging/boot-rig/run.sh --uefi   # both clients, OVMF
$ packaging/boot-rig/run.sh --keep   # leave the stack up to poke at
```

Results land in `results/`: `serial.log` from the clients, `server.log` and `dhcp.log`
from the containers. When a marker is missing those three files are the whole
investigation.

## Watch it fail, link by link

A green rig that has never been red proves nothing — the same reasoning as the
listing-cache test that passed for the wrong reason. Break each link and watch the marker
it guards disappear:

| Break | What should go red |
|---|---|
| Delete a loader from the volume | the ROM gets nothing; `boot check` also goes red |
| Point `RESCRIPTUM_MEDIA_ADDR` somewhere else without rebuilding the loaders | the embedded script chains into a refused connection, and the recovery paths must *engage* |
| Remove the claimed machine's answer | it should land on its **disk**, not hang — the fallthrough covers a machine whose answer was deleted, too |
| Drop a fact a template needs | the claimed machine must fail loudly rather than install with a broken hostname |
| Stop the server, boot a client | it must fall through to its next boot device rather than wait |

That last one turns the blast-radius table in the guide from a claim into a recorded run.

## Status: green, and it has been red

**Run, on this machine, under TCG.** All four markers reached:

```
  ok   the DHCP handoff answered, from our own generated snippet
  ok   a loader was fetched over TFTP
  ok   the unclaimed machine fell through to its local disk
  ok   the claimed machine fetched its sentinel

rig: all markers reached
```

That is the whole chain: a DHCP offer built from `boot dhcp-snippet`'s own output, our
branded loader over TFTP, its embedded script, the bootstrap, the answer engine — and
then either a machine's own unattended answer or the menu and a fall through to its disk.

And it has been **watched failing**, which is what makes the green mean anything:

| Break | What went red |
|---|---|
| Delete `ipxe-undionly.kpxe` | `boot check` says MISSING and the run stops before a client boots |
| Delete the claimed machine's answer | that marker alone goes red — and the machine still falls through to its disk, so the fallthrough covers a deleted answer too |

The rows in the table above that have *not* been run yet are the moved media port, the
missing template fact, and the stopped server. Those are the next ones to watch go red.

**What is still not proven is what real firmware does.** The rig runs one emulator with
one NIC model, and the standing rule is unchanged: nothing ships on harness evidence
alone.
