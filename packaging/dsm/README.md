# The Synology DSM package

Everything here assembles a `.spk` around an already-built binary. Nothing in `src/` knows
Synology exists, and nothing here compiles anything: **an `.spk` is a release format**,
exactly like the `.tar.gz` archives — the same artifact, wrapped for one platform's package
manager. If this ever seems to need a `#[cfg]` or a feature flag, the design has gone wrong.

The two places DSM did push back on the program are answered in packaging rather than in
code: log rotation, by a `copytruncate` stanza, and the CLI's need to find its
configuration, by the three-line `payload/bin/rescriptum-cli` wrapper.

```console
$ ./build.sh --spk x86_64-unknown-linux-musl     # build, then wrap
$ packaging/dsm/make-spk.sh armv7                # wrap a build that already exists
$ packaging/dsm/check-spk.sh                     # structural check over dist/*.spk
```

`make-spk.sh` is deterministic — fixed mtimes, ownership `0:0`, `ustar`, `gzip -n`, a
pre-sorted file list — so the same inputs give a byte-identical `.spk` and the published
checksum means something. It runs on GNU tar and on bsdtar; the two do not agree with each
other byte for byte, and the release always runs on the same one.

## What is in the archive

| | |
|---|---|
| `INFO.in` | metadata; `make-spk.sh` fills in the version, the `arch` line and `extractsize`, and strips the comments |
| `conf/privilege` | DSM 7 requires the package to lower its privilege explicitly. `run-as: package`, `username: rescriptum` |
| `conf/resource` | the four resource workers: the shared folder, the firewall entry, the logrotate stanza, the `/usr/local/bin` symlink |
| `scripts/` | the lifecycle. `start-stop-status` is the service; the other six are guards |
| `WIZARD_UIFILES/install_uifile` | two questions, in JSON. (The Vue render-function format is a *second* way, introduced in DSM 7.2.2 — which the DS416j can never run) |
| `payload/` | what lands in `/var/packages/rescriptum/target`, beside the binary |
| `PACKAGE_ICON*.PNG` | 64×64 and 256×256, committed rather than generated, so the build needs no image toolchain and the archive stays byte-stable |

## Decisions worth not re-litigating

- **One name everywhere — `rescriptum`**: the package, the share and the user. A username
  that does not match `data-share`'s permission list creates the share and grants it to
  nobody, silently, which is why `conf/privilege` sets it explicitly instead of letting DSM
  derive it.
- **The env file is the configuration interface, before and after install.** DSM has no
  settings panel for a package: the only wizards are install, upgrade and uninstall. So
  `/var/packages/rescriptum/etc/rescriptum.env` is where settings live, `postinst` writes it
  complete on a fresh install, and stop/start from Package Center is the reconfiguration
  gesture.
- **`postinst` writes that file only when it is absent**, because `postinst` runs on an
  *upgrade* too. `preupgrade`/`postupgrade` carry it through `$SYNOPKG_TEMP_UPGRADE_FOLDER`
  as well; both, deliberately. And `postinst` **consults that folder before deciding the
  file is absent** — it runs *before* `postupgrade`, so on an upgrade where `etc/` did not
  survive, writing defaults there would destroy the user's configuration before the restore
  ever ran. That one was found by simulating exactly that case, not by reading the
  sequence.
- **`postuninst` touches the package tree only** — never the share, never the database,
  under any status. It runs during an upgrade as well as an uninstall, and when the store
  is SQLite the database *is* the answers.
- **Every path is set explicitly.** `RESCRIPTUM_ANSWERS_DIR` defaults to `/srv/answers` and
  `RESCRIPTUM_DB_PATH` to `/srv/answers.db`; neither exists on DSM, and an unopenable store
  is a startup error rather than a warning. The database goes in the share beside the
  answers, everything disposable in `var/`.
- **`arch` claims an ABI, not a platform.** It takes family names, so `x86_64` covers every
  Intel platform including ones Synology has not shipped yet — the guide's own appendix is
  already missing `r1000` and `epyc7002`. The family shorthand does *not* reach the Marvell
  ARMv7 platforms, so the DS416j's `armada38x` is named. The rule for widening it: the
  binary has to have run on the oldest-kernel member of that ABI first.
- **Assembled by hand rather than with `pkgscripts-ng`.** The toolkit exists to *compile*
  inside a DSM environment; the binary is already built and linked. Doing it
  ourselves keeps the release job hermetic and reviewable, which matters for something
  people run as root.
- **No package source, so no update notifications.** Download the `.spk` from the GitHub
  Release and install it by hand, for an upgrade as much as for an install. The
  documentation says so out loud rather than letting it become a bug report.

## How it is tested

Three harnesses, and each proves what the others cannot.

```console
$ packaging/dsm/check-spk.sh                          # the archive is what DSM expects
$ packaging/dsm/lifecycle-test.sh                     # what the scripts decide
$ packaging/dsm/vm/on-dsm.sh admin@localhost -p 2222  # what only DSM can answer
```

The first two run **in CI on every push**, so packaging breaks on the PR that breaks it
rather than at tag time. `lifecycle-test.sh` unpacks an `.spk` into a fake `/var/packages`
tree and drives the real scripts through it: install with a wizard and without one, a
hostile wizard value, start and `/health` on the port the wizard chose, the service still
alive seconds later, the exit codes Package Center reads (`3` for stopped, `1` for a stale
pidfile), a start that cannot succeed, an upgrade over a hand-edited env file and a canary
in the share — twice, once with `etc/` surviving and once with it wiped — and an uninstall
that must leave the answers alone. 33 checks, seconds, no DSM.

It was watched failing: reverting the `postinst` upgrade guard, making `postuninst` delete
the share, returning `1` for a stopped package and refusing `prestart` turns it into 25
green and 8 red.

**The third has now run, green, on a DSM 7.2.2 machine** — 24 checks: installed, started and
still alive seconds later, `/health` answering on the wizard's port, the share created and
writable by the package user, the firewall entry acquired, `sudo -u rescriptum
rescriptum-cli check` passing, `logrotate -f` rotating without moving the inode, an upgrade
carrying a hand-edited env file and a canary through untouched, and an uninstall leaving the
share alone. **And it has now run on the DS416j itself** — installed from Package Center, wizard
rendered, service started, a real answer composed from a group and a machine file in 3–4 ms,
and an upgrade to `-2` that kept both the configuration and the answers. That run is what
found the two defects the VM could not: musl 1.2 cannot work on Synology's 3.10 kernels, and
an AppleDouble file dropped by a Mac over SMB hijacks a machine's answer.

It needs a machine, and [`vm/README.md`](vm/README.md) is the rig — a QEMU launcher
for a DSM 7 VM, and one script that runs the on-machine checks against the VM while you
iterate and against the DS416j for the verdict. It covers the `data-share` worker and its
ACL, the `port-config` worker, the generated systemd unit, `logrotate -f` against a live
descriptor, `sudo -u rescriptum rescriptum-cli check`, and whether Package Center accepts
the archive at all. It is destructive on purpose, and nothing ships on VM evidence alone.

Open, and answered by watching rather than by reading — `on-dsm.sh` prints all four:
whether `etc/` and `var/` survive an upgrade on their own, whether the `port-config` worker
acquires before or after `postinst` runs, what the generated unit says about `Restart=`,
and where DSM installed the logrotate stanza. Two more need a person: whether
`synopkghelper` needs root, and what Package Center says to an `.spk` built for the wrong
`arch`.

When it fails, three places say why: `/var/log/packages/rescriptum.log` (our scripts' own
output), `/var/log/synopkg.log` (Package Center's view) and
`systemctl status pkgctl-rescriptum`.
