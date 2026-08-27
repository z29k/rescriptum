---
title: Testing
description: Where a test belongs, the two-store conformance suite, and the assertions that have actually caught things.
sidebar:
  label: Testing
  order: 8
---

# Testing

545 tests. `cargo test` runs all of them in about twenty seconds — most of that is
`tests/tftp.rs`, which waits on real UDP timeouts because that is what it is testing.

**`cargo test` does not run the harnesses that matter most**: the boot rig, the DSM
package's three, and the loader build. See [The package is tested too](#the-package-is-tested-too-in-three-places)
and [the boot rig](#the-boot-chain-belongs-in-the-rig).

```bash
cargo test                                # everything
cargo test <name>                         # one, by substring
cargo test -- --nocapture                 # show stdout
cargo test --all-features                 # what CI runs
```

## Where a test belongs

| Suite | Cases | For |
|---|---|---|
| `tests/cli.rs` | 47 | `render`, `check`, `import`, `export`, `config`, and the env file — against the real binary |
| `tests/integration.rs` | 45 | the real binary over a real socket |
| `tests/media.rs` | 45 | boot media against the real binary, with both listeners up |
| `src/config.rs` | 42 | the environment, what refuses to start, and which of the file and the environment wins |
| `tests/stores.rs` | 39 | **every behaviour, against both stores** |
| `src/select.rs` | 27 | normalization, scoring, layering, template filling |
| `src/format/mod.rs` | 27 | parsing, merging, control keys, endpoint aliases |
| `tests/admin.rs` | 26 | the admin API end to end, formats included |
| `src/envfile.rs` | 23 | the env-file parser and writer, and what each refuses |
| `src/facts.rs` | 22 | query parsing, JSON flattening, globbing |
| `tests/tftp.rs` | 21 | TFTP over real UDP, and what a failed bind must not cost |
| `src/format/xml.rs` | 18 | the XML tree — pairing, entities, fidelity |
| `src/merge.rs` | 11 | the TOML deep merge |
| `tests/guards.rs` | 7 | the answer token, and the lockout that deliberately is not there |
| `src/log.rs` | 4 | level parsing, and the timestamp arithmetic |
| `src/boot/*.rs` | 120 | the ISO reader, probing, the catalogue, patch plans, the menu, the loader table, DHCP snippets, cpio and SHA-256 |
| `src/admin.rs`, `src/capture.rs`, `src/store/mod.rs` | 21 | unit-level behaviour |

## `tests/stores.rs` — the conformance suite

Every behavioural case runs **twice**, once per store, and asserts the identical outcome.
That suite is what keeps two backends from drifting.

**A new behaviour belongs there, not in a store-specific test.** A test that covers one
backend proves half of what it claims — and the half it does not cover is exactly where a
divergence hides.

## `tests/cli.rs` — the commands people are told to run

`check` is what [`deploy.sh`](./building.md#deploying-a-build) runs before it ships
anything, and what the documentation tells people to put in CI — so **its exit code is a
contract**, not a convenience. `render`'s stdout/stderr split is another: the document
goes to stdout so `render … > answer.toml` yields a usable file, and the provenance line
goes to stderr so it does not end up inside it.

Also pinned here: the `import` → `export` round trip is **byte-identical**, comments and
formatting included. That is what makes the database safe to adopt *and* safe to leave;
if it ever stops being exact, `export` is no longer a way back out.

## `tests/integration.rs` — against the real binary

It starts the actual binary on an **ephemeral port** and talks HTTP to it. The binary
prints the address it bound, so there is no port race and no sleep-and-hope.

This suite exists because some failures are invisible to unit tests. The clearest example:
hyper **panics at runtime** if `header_read_timeout` is set without `.timer(…)`. It
compiles. Only a real connection finds it.

Explicitly covered:

- a **truncated request**, and one with **no `Content-Length`**;
- an aberrant `Content-Length` — *and* a chunked body that outgrows the cap while
  streaming, which is the other way in and trips the limit mid-read;
- an unknown method, an empty body, a 1 MB body, a body that is not valid UTF-8;
- the connection cap: over it, a prompt `503` rather than a queue — and the permit coming
  back afterwards;
- **and, after each of those, that the server still answers.** That last assertion is the
  one that matters — the abuse is only interesting if the server survives it.

> **`cargo test` does not rebuild `target/debug/rescriptum`.** A manual check against a
> stale binary once "reproduced" a bug that had already been fixed. Rebuild before poking
> at the binary by hand.

## `tests/tftp.rs` — a transfer is a conversation

Nothing here can be proved from inside a function. Blocks, acknowledgements,
retransmission, the empty packet that ends a transfer — every bug worth catching lives in
the turn-taking, and the first run found two of the "works by hand, never after a reboot"
kind. **A file whose length is an exact multiple of the block size must end with an empty
data packet**; leave it out and the client waits forever for a final block that never
comes.

It also owns the one listener failure in this server that is *not* fatal. A TFTP port that
cannot be bound must not take answers and media down with it — measured on DSM, where the
capability is granted outside the package and an upgrade drops it — so the test holds the
port with a squatter, then asserts three things at once: the server came up, it warned and
said what still works, and `boot check` still exits non-zero.

That last one first passed for the wrong reason: three missing loaders were already
failing the command. The fixture now writes every loader the table names, and a control
run with TFTP off proves the directory is otherwise clean.

## `tests/media.rs` — boot media against the real binary

Both listeners up, and every abuse case ends by proving the server still answers. One case
proves the property the separate socket exists for: **answers keep succeeding while four
image transfers are in flight.**

There is deliberately **no binary ISO fixture in this repository**. `boot::iso::build`
writes images in memory, behind the `test-support` feature so it never reaches a release
binary.

## The boot chain belongs in the rig

`packaging/boot-rig/run.sh` is not Rust and `cargo test` does not run it. It boots a
claimed and an unclaimed machine in QEMU under TCG, on a private bridge with no uplink,
and asserts four markers: the DHCP handoff answered from our own generated snippet, a
loader fetched over TFTP, the unclaimed machine fell through to its local disk, and the
claimed machine reached its own answer. CI runs the same thing plus a deliberate break.

**A QEMU guest bridged into a container has a MAC of its own, and Docker Desktop's virtual
switch does not forward frames from a MAC it did not assign** — measured, which is why the
primary rig is one container rather than four on a Docker network.

## Check that a test can fail

A test that passes for the wrong reason is worse than no test: it reports coverage that
does not exist. Before trusting a new one, break the thing it guards and watch it go red.

One in this suite did not survive that check. It claimed to protect the `version.is_some()`
clause in the listing cache; removing the clause left it green, because with either store a
version is unreadable only when the store is also empty, so the clause cannot currently
fire at all. The test proves something real — a directory that appears after startup is
served on the very next request — and now says so instead.

## Assertions worth copying

- **Assert on parsed values, not on formatting.** Replacing a table with a scalar leaves
  the key's original decor, so the output can read `value= 3` — valid TOML, different
  text. A string comparison there fails for the wrong reason, or passes for one.
- **Cache-invalidation tests must share one `Answers` instance.** A test that constructs a
  fresh one per call bypasses the cache entirely and silently proves nothing.
- **`Config::from_lookup` takes a closure**, so configuration tests never touch the
  process environment — and therefore never race each other under a parallel test runner.
- **Assert the old text was found before writing.** Two `python`/`sed` patches in this
  project's history silently matched nothing and were only caught by checking test counts
  afterwards.

## The example answers are a test too

```bash
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
```

[`examples/`](https://github.com/z29k/rescriptum/tree/main/examples) holds a worked example
of every format, and it is the only place they are shown composing together. Two of them
caught real bugs — a missing doctype and an unpaired `pass` attribute. Keep them working.

## The package is tested too, in three places

`cargo test` does not touch the DSM package, because none of it is Rust. Three harnesses
do, and each proves something the others cannot.

| | Proves | Cost |
|---|---|---|
| [`packaging/dsm/check-spk.sh`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/check-spk.sh) | the archive is structurally what DSM expects — uncompressed outer tar, six `INFO` fields, an all-numeric version, `os_min_ver` at least 7.1, 64×64 and 256×256 icons, executable scripts with no CRLF, **the packaged binary's own `--version`**, and the desktop application: `dsmappname` naming a class its `ui/config` actually declares, a JavaScript filename that carries the version, and a backend that still checks the DSM session and `administrators` | seconds, **on every push** |
| [`packaging/dsm/lifecycle-test.sh`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/lifecycle-test.sh) | everything the package's *scripts* decide, against a fake `/var/packages` tree: the env file written once and only once, the wizard's values **and their absence**, the service surviving its own start script and answering `/health`, the exit codes Package Center reads, an upgrade that must not touch a hand-edited configuration, an uninstall that must not touch the answers — **and the desktop application's backend**, driven with a stubbed authenticator: refusing no session, refusing a non-administrator, refusing a write with no intent header, refusing one that would stop the server starting, and never handing a token to the browser | seconds, **on every push** |
| [`packaging/dsm/vm/on-dsm.sh`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/vm/on-dsm.sh) | DSM's own machinery — the `data-share` worker and its ACL, the `port-config` worker, the generated systemd unit, logrotate against a live descriptor, whether Package Center accepts the archive — **and that a machine asking for its configuration gets one**: a POST with hardware in the body, answered by that machine's file merged over the group claiming it. It also owns **the only route to port 69**: that `69/udp` survives into the acquired firewall entry, that the package still answers without the capability, and that `setcap cap_net_bind_service=+ep` plus a restart binds `udp/69` as the unprivileged package process | minutes, on a DSM 7 VM — and then on the DS416j |

```bash
packaging/dsm/lifecycle-test.sh                     # the first .spk in dist/ that runs here
docker compose -f packaging/dsm/vm/docker-compose.yml up -d   # a DSM 7.2 machine
packaging/dsm/vm/on-dsm.sh admin@<host> -p 2222     # against it
packaging/dsm/vm/on-dsm.sh admin@nas                # the verdict
```

The VM is `vdsm/virtual-dsm`, which installs Synology's own Virtual DSM release — no loader
image to find. KVM makes it fast rather than possible: without `/dev/kvm` it emulates, about
ten times slower, which is what `docker-compose.emulated.yml` is for. It does want **14 GiB
free** for the storage, hardcoded in the image.

The last one is **destructive on purpose** — it upgrades over a hand-edited env file and a
canary in the shared folder, then uninstalls, then checks both survived. Those two guards
are the most expensive things in the package to get wrong, and the first published `.spk`
is the one whose uninstall scripts will run during everybody's first upgrade.
[`packaging/dsm/vm/README.md`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/vm/README.md)
is the rig: what it is evidence about, and what it is not.

The same rule as everywhere else applies to these: **break the thing they guard and watch
them go red.** Reverting the `postinst` upgrade guard, making `postuninst` delete the
share, returning `1` for a stopped package and refusing `prestart` turns 33 green checks
into 25 green and 8 red — which is how we know the harness is testing anything at all.
Today it is **58** checks in `lifecycle-test.sh`, 26 in `check-spk.sh` and **47** on the
machine; the three most recently added were each watched red the same way — by putting
`RESCRIPTUM_TFTP_ADDR=off` back, by deleting the panel's report of the TFTP state, and by
making it claim to be serving with nothing bound.

## CI

`.github/workflows/ci.yml`, on every push to `main` and `develop` and on every pull
request:

| Job | Runs |
|---|---|
| **gates** | `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -D warnings`, `cargo test --all-features`, `cargo build --release --no-default-features` |
| **docs** | builds the public site and runs `notabene lint` |
| **audit** | `cargo audit --deny warnings` over the dependency tree |
| **cross** | a full ARMv7 build against the glibc floor DSM has, asserting it needs nothing newer, then assembles both `.spk`s, checks them structurally and drives the package lifecycle |

The cross job is not redundant. **SQLite is compiled from source, and `armv7-musl` is the
least forgiving target shipped** — it is where a C dependency breaks first. Catching that
on a push beats catching it while cutting a release.

The **audit** job is the other half of the rule that adding a dependency needs a reason: a
reason to add one is not a reason to keep it. `--deny warnings` fails on an unmaintained or
yanked crate too, not only on a vulnerability. When something appears with no fix, add
`--ignore RUSTSEC-…` with a line saying why rather than dropping the flag.

Every action used is an official `actions/*` one, and both Zig and `cargo-audit` are
installed directly rather than through a third-party action. That is deliberate: this
toolchain vets and links a binary people run as root.

The docs site has its own gate — see [the docs site](./docs-site.md#the-ci-gate).
