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
./build.sh armv7-unknown-linux-musleabihf
./build.sh --help
```

`build.sh` adds a missing Rust target for you and **warns if a musl build came out
dynamically linked** — which DSM would refuse to run, at exec time on the NAS rather than
at build time on your laptop.

Plain `cargo build` works too; `build.sh` exists for the size report and that warning.

## The release targets

| Target | For | Cross |
|---|---|---|
| `armv7-unknown-linux-musleabihf` | the DS416j, the reason this project exists | zigbuild |
| `aarch64-unknown-linux-musl` | modern ARM NAS, Raspberry Pi | zigbuild |
| `x86_64-unknown-linux-musl` | most other Linux hosts | zigbuild |
| `aarch64-apple-darwin` | local development | native |
| `x86_64-apple-darwin` | local development | native |

## Cross-compiling

[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) uses Zig as the linker,
which avoids a full cross toolchain per target:

```bash
cargo install cargo-zigbuild
cargo zigbuild --release --target armv7-unknown-linux-musleabihf
```

**Verify it really is static.** A dynamically linked musl binary fails at exec time, on
the NAS, with an error that does not obviously say so:

```console
$ file target/armv7-unknown-linux-musleabihf/release/rescriptum
ELF 32-bit LSB executable, ARM, EABI5 version 1 (SYSV), statically linked, stripped
```

CI asserts this on every push, for exactly that target.

### Installing Zig on the maintainer's machine

Zig is **not** a Homebrew install here: `brew install` aborts on that machine over
untrusted third-party taps unrelated to Zig. It lives in `~/.local/zig`, symlinked at
`~/.local/bin/zig`. **To upgrade, replace that directory** — `brew upgrade zig` does
nothing.

Verified toolchain: Rust 1.93, `cargo-zigbuild` 0.23.0, Zig 0.16.0, with targets
`aarch64-apple-darwin` and `armv7-unknown-linux-musleabihf` installed.

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
| `TARGET` | `armv7-unknown-linux-musleabihf` |
| `ANSWERS` | `<remote-dir>/answers` |
| `PORT` | `8000` |
