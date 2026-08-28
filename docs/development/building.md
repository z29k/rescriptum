---
title: Building
description: Native builds, cross-compilation with zigbuild, the five release targets, and the size budget.
sidebar:
  label: Building
  order: 9
---

# Building

```bash
./build.sh                    # this machine, and print the size
./build.sh --all              # every target a release ships
./build.sh --no-sqlite        # the smallest binary
./build.sh armv7-unknown-linux-gnueabihf
./build.sh --help
```

`build.sh` adds a missing Rust target for you and **warns if a musl build came out
dynamically linked** — which DSM would refuse to run, at exec time on the NAS rather than
at build time on your laptop.

Plain `cargo build` works too; `build.sh` exists for the size report and that warning.

## The release targets

| Target | For | Cross |
|---|---|---|
| `armv7-unknown-linux-gnueabihf` | the DS416j, the reason this project exists — **glibc, not musl**, see below | zigbuild, floor 2.17 |
| `aarch64-unknown-linux-musl` | modern ARM NAS, Raspberry Pi | zigbuild |
| `x86_64-unknown-linux-musl` | most other Linux hosts | zigbuild |
| `aarch64-apple-darwin` | local development | native |
| `x86_64-apple-darwin` | local development | native |

## Cross-compiling

[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) uses Zig as the linker,
which avoids a full cross toolchain per target:

```bash
cargo install cargo-zigbuild
cargo zigbuild --release --target armv7-unknown-linux-gnueabihf.2.17
```

## Why armv7 is the one target that is not musl

Every other target is static musl. ARMv7 is glibc, and it is not a preference — it is the
only way the machine this project exists for runs the binary at all.

**Synology's ARMv7 kernels are 3.10, and they answer the *time64* syscalls with `EINVAL`
rather than `ENOSYS`.** musl 1.2 made `time_t` 64-bit on 32-bit architectures and tries
`clock_gettime64` (and `clock_nanosleep`, and the timed futex) first, falling back to the
32-bit syscall **only on `ENOSYS`**. On a kernel that says `EINVAL` the fallback never
happens, so every call for the time fails. Measured on a DS416j running DSM 7.1, kernel
3.10.108:

```console
$ ./probe
libc clock_gettime(CLOCK_REALTIME)  -> -1  errno=22 (Invalid argument)
syscall 263 (time32)                -> 0   ok
syscall 403 (time64)                -> -1  errno=22 (Invalid argument)
```

The symptom is a binary that answers `--version` and then panics the moment it wants a
timestamp — `time.rs:131`, `Os { code: 22, kind: InvalidInput }`. It is not an ABI problem
and not a kernel-too-old-for-the-instructions problem, which is what it looks like.

glibc on 32-bit uses the time32 syscalls, and DSM ships its own (2.20 on `armada38x`). So
the armv7 build targets a **glibc floor of 2.17** — low enough for DSM, and since glibc is
backward compatible, the same binary runs on newer ARMv7 Linux as well.

**What to verify, then, is not that it is static — it is that it needs no glibc newer than
the floor.** Anything newer fails at exec time on the NAS, naming a symbol version and
nothing else:

```console
$ readelf --dyn-syms target/armv7-unknown-linux-gnueabihf/release/rescriptum \
    | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1
GLIBC_2.17
```

CI asserts exactly that on every push. The musl targets are still checked for being static,
because for them that is the promise.

### Installing Zig on the maintainer's machine

Zig is **not** a Homebrew install here: `brew install` aborts on that machine over
untrusted third-party taps unrelated to Zig. It lives in `~/.local/zig`, symlinked at
`~/.local/bin/zig`. **To upgrade, replace that directory** — `brew upgrade zig` does
nothing.

Verified toolchain: Rust 1.93, `cargo-zigbuild` 0.23.0, Zig 0.16.0, with targets
`aarch64-apple-darwin` and `armv7-unknown-linux-gnueabihf` installed.

## The release profile

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
```

`panic = "abort"` is **deliberately absent** — see
[constraints](./constraints.md#never-panic-on-malformed-input). Measured cost of keeping
unwinding on ARMv7: +2416 bytes, +0.8%.

## Size

| Build | ARMv7 |
|---|---|
| default | 2,103,456 bytes |
| `--no-default-features` (no SQLite, no admin API) | 944,928 bytes |

Most of the difference is bundled SQLite, compiled from source. CI builds
`--release --no-default-features` on every push so the small build cannot rot unnoticed.

## Features

| Feature | Default | Gives |
|---|---|---|
| `sqlite` | on | the SQLite store and the admin API |

```bash
cargo build --no-default-features          # smallest
cargo test --all-features                  # what CI runs
```

## The Synology package

A `.spk` is a **release format**, not a build: the binary is finished before packaging
begins, there is no DSM-specific build, and nothing in `src/` knows Synology exists.

```bash
./build.sh --spk x86_64-unknown-linux-musl   # build, then wrap it
packaging/dsm/make-spk.sh armv7              # wrap a build that already exists
packaging/dsm/check-spk.sh                   # structural check over dist/*.spk
```

**The package carries the loaders, so build them first or it will not pass its own
check.** `make-spk.sh` takes them from `packaging/ipxe/out` (override with
`RESCRIPTUM_LOADERS`), and `check-spk.sh` fails a package that has none — a TFTP server
with nothing to hand out boots nothing. Building iPXE needs a Linux C toolchain, which on
a Mac means a container:

```bash
docker run --rm --platform linux/amd64 -v "$PWD:/w" -w /w debian:bookworm-slim sh -c '
  apt-get update -qq &&
  apt-get install -y --no-install-recommends build-essential liblzma-dev mtools \
    xorriso isolinux gcc-aarch64-linux-gnu git ca-certificates perl &&
  packaging/ipxe/build.sh --out /w/packaging/ipxe/out'
```

Once, not per package: the loaders are the same bytes in every ABI's `.spk`, because they
run on the machines being *booted*, not on the NAS. `packaging/ipxe/out` is gitignored —
**no binaries in git, ever**.

| ABI | `arch` in `INFO` | From |
|---|---|---|
| `x86_64` | `x86_64` — the *family* name, so it covers every Intel platform | `x86_64-unknown-linux-musl` |
| `armv7` | `armada38x` — the family shorthand does not reach the Marvell platforms | `armv7-unknown-linux-gnueabihf` |
| `aarch64` | `armv8` | `aarch64-unknown-linux-musl`, once the binary has been run on one |

The rule for widening that: **claim an ABI once the binary has run on the oldest-kernel
member of it**, never because a platform is plausible.

`make-spk.sh` is deterministic — fixed mtimes, ownership `0:0`, `ustar`, `gzip -n`, a
pre-sorted file list — so the same inputs give a byte-identical `.spk`, which is what makes
the published checksum worth something.

`check-spk.sh` runs in CI on every push. It asserts the outer archive is an *uncompressed*
tar, that `INFO` has its six required fields and an all-numeric version, that the icons are
exactly 64×64 and 256×256, that the lifecycle scripts parse and are executable, and that
**the packaged binary's own `--version` matches `INFO`** — the x86_64 build runs on the
runner, so that last one is a real assertion rather than a re-read of the same string.

`lifecycle-test.sh` then drives the package's own scripts against a fake `/var/packages`
tree — install, start, `/health`, the exit codes, an upgrade over a hand-edited
configuration, an uninstall over a canary in the share — and also runs on every push.

```bash
packaging/dsm/lifecycle-test.sh
```

What none of that can prove is that DSM will accept the package; only installing it can.
That is the rig in
[`packaging/dsm/vm/`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/vm/README.md):
a QEMU launcher, and one script that runs the on-machine checks against the VM while you
iterate and against the DS416j for the verdict. See
[testing](./testing.md#the-package-is-tested-too-in-three-places).

## Deploying a build

```bash
./deploy.sh admin@nas
./deploy.sh admin@nas /volume1/netboot
```

Builds, **checks the answers and refuses to ship if they do not come back clean**, copies
under a temporary name, restarts, and confirms `/health`. See
[deployment](../guide/operations/deployment.md#replacing-a-running-instance).

| Environment | Default |
|---|---|
| `TARGET` | `armv7-unknown-linux-gnueabihf` |
| `ANSWERS` | `<remote-dir>/answers` |
| `PORT` | `8000` |
