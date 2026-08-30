# The branded iPXE loaders

What TFTP hands out, and the first thing a machine executes that we wrote.

## Status: built and verified; not yet booted

`build.sh` has been run against the pinned commit on Debian bookworm and produces **all
eight loaders**, ARM64 included. Three things were checked on the output rather than
assumed:

- the EFI binaries carry `PRODUCT_NAME "rescriptum boot"` and `PRODUCT_URI`;
- they carry `embed.ipxe` **verbatim**, `chain http://${next-server}:8001/ipxe/bootstrap`
  and all — which is the entry point of the whole chain;
- `rescriptum boot check` agrees the set satisfies the loader table.

The BIOS `.kpxe` shows none of that to `strings`, because it is a compressed image and
what is visible is the decompressor stub. Expected, not a failure.

**What remains unproven is what firmware does with them.** A build says the bytes exist;
only a machine says they boot. That is the rig and then real hardware, and the project's
standing rule applies — nothing ships on harness evidence alone.

Two things the first build taught, both now handled:

- **The build must be amd64.** iPXE's BIOS targets are 32-bit x86; an ARM64 host's gcc
  produces a wall of `unrecognized command-line option '-m32'` that reads like a broken
  Makefile.
- **ARM64 needs `CROSS_COMPILE=aarch64-linux-gnu-`.** Without it the host compiler is
  used and fails on `-mlittle-endian`. `build.sh` now names the missing package and
  carries on with the x86 loaders rather than stopping.

## Why we build it at all

Three reasons converged, and any one would have settled it:

1. **The entry point.** A stock `undionly.kpxe` from ipxe.org does DHCP, is told to load
   iPXE, and loads itself forever — the documented chainloading loop. A stock
   netboot.xyz binary chains to the *public* `boot.netboot.xyz`: no loop, but our menu
   and our answers are never consulted. Only an embedded script gets a machine talking
   to this server, and only a build we control can carry one.
2. **The name on the first line**, before anything else is on screen, plus a framebuffer
   console that a stock binary may not have compiled in.
3. **The feature set.** We choose what is in — PNG, the menu commands, `sanboot`, the
   console — rather than discovering at a customer site that a variant lacks one.

## What is here

| File | What it is |
|---|---|
| `PINNED` | The upstream commit, **by SHA**. A tag is a pointer somebody can move |
| `branding.h` | Our name, and the two URIs we deliberately do *not* change |
| `embed.ipxe` | The embedded script: the entry point of the whole chain |
| `build.sh` | Clones, pins, patches, builds, hashes, writes `NOTICE` |

## The two URIs we leave alone

`PRODUCT_ERROR_URI` and the command-help URI point at ipxe.org's database, which turns a
32-bit error code into a sentence, names the source file that produced it, and links the
line of code that raised it. Redirecting them at us would replace a working diagnostic
service with nothing — and the person staring at a hex code at 3am is exactly who that
database exists for.

Keeping `PRODUCT_SHORT_NAME` as `iPXE` is upstream's own request, "to minimise end-user
confusion", and it is also the right way to use somebody's GPLv2 work.

## The port in `embed.ipxe` is a contract

The embedded script can read no configuration — it is baked in before any deployment
exists. It knows exactly two things: `${next-server}`, which DHCP supplies, and a port,
which nothing supplies. So **8001 is as fixed here as an answer URL baked into an ISO**.

Moving `RESCRIPTUM_MEDIA_ADDR` is allowed and costs exactly this. Three things keep it
survivable, and `boot check` reports the first sign of trouble:

- the generated `autoexec.ipxe` in the TFTP root carries the *configured* address, so
  platforms that fetch it recover with no rebuild;
- the embedded script's own `||` turns the refused `chain` into a second chance — a
  relative fetch that resolves, over HTTP, to whatever port actually served the loader;
- `boot check` warns when the configured port is not the one shipped loaders embed.

## Licensing

iPXE is **GPLv2** (with the UBDL exception); rescriptum is MIT. The loaders are separate
files, never linked into our binary — mere aggregation, obvious and auditable. **No
binaries in this repository, ever**: they are a release artifact carrying the upstream
licence texts, a `NOTICE` naming the exact commit and digests, and the written offer for
source, which is this directory.

## Building

```console
$ ./build.sh                # into ./out
$ ./build.sh --out /srv/boot
```

Then point `RESCRIPTUM_BOOT_DIR` at the output and ask the server whether it agrees:

```console
$ rescriptum boot check
```

That compares the directory against the same table the TFTP server serves from and
`boot dhcp-snippet` generates from. **A snippet naming a loader that is not on disk
fails silently, at the ROM**, with nothing on any console — it is the least diagnosable
failure in the whole chain, and this is what catches it.
