# CLAUDE.md — packaging/dsm

Guidance for working on the Synology package. It loads only when Claude touches files under
`packaging/dsm/`; the root `CLAUDE.md` keeps the one rule that has to be visible everywhere —
**changing anything here means running the machine**, not just the local harness.

## The DSM package

`packaging/dsm/` wraps an already-built binary as a DSM 7 `.spk`. It is a **release
format**, exactly like the `.tar.gz` archives — no DSM-specific build, no feature flag,
nothing in `src/`. The **four** places DSM pressed back are answered in packaging: log
rotation by a `copytruncate` stanza, a CLI that cannot find its configuration by a
three-line wrapper (`rescriptum-cli`, which names `RESCRIPTUM_ENV_FILE`), no settings panel
by the desktop application below, and **a privileged port by one root command**. DSM 7
does not let an unsigned package run as root — measured, four routes, in
`docs/development/traps.md` with the error codes — but `setcap cap_net_bind_service=+ep`
on the installed binary works, after which the package binds `udp/69` as its own
unprivileged user alongside 8000 and 8001. All three are registered with the firewall.
**The package ships the loaders**, so the share's `boot` folder arrives filled and `start`
refreshes it when the stamp does not name this version; a TFTP server with nothing to hand
out boots nothing, and a second download is how a working appliance becomes a support
thread. Verified on the machine by fetching `ipxe-undionly.kpxe` over TFTP with an
independent client and comparing it byte for byte.
**The capability belongs to the file, so an upgrade drops it**; the env file says so and
points at a Task Scheduler boot-up task. `RESCRIPTUM_TFTP_ADDR` is therefore left unset —
its default *is* port 69, which is what every loader and every generated snippet expects.
An earlier version shipped `off` and sent operators to DSM's own TFTP server: that traded
the product's first principle for a packaging constraint that turned out not to exist, and
it is not a precedent. `RESCRIPTUM_USER`/`_GROUP` stay documented as unusable — the package
already is its own unprivileged user. If this ever seems to need a `#[cfg]`, the design has gone wrong.

```bash
./build.sh --spk x86_64-unknown-linux-musl   # build, then wrap
packaging/dsm/make-spk.sh armv7              # wrap an existing build
packaging/dsm/check-spk.sh                   # structural check          ⎫ both run
packaging/dsm/lifecycle-test.sh              # drive the scripts         ⎭ by ci.yml
packaging/dsm/vm/on-dsm.sh admin@nas         # what only DSM can answer
```

**The package is tested in three places, and none of it is Rust** — `cargo test` does not
touch it. `check-spk.sh` asserts the archive's shape; `lifecycle-test.sh` unpacks an `.spk`
into a fake `/var/packages` tree and drives the real scripts through install (with a wizard
and without), start, `/health`, the exit codes, an upgrade over a hand-edited env file and
a canary — with `etc/` surviving and with it wiped — and an uninstall; both run on every
push. `vm/on-dsm.sh` runs the rest on a DSM 7 VM and then on the DS416j: `data-share`'s
ACL, `port-config`, the generated unit, `logrotate -f` against a live descriptor, and
whether Package Center accepts the archive at all. **Nothing ships on VM evidence alone**,
and `lifecycle-test.sh` was watched failing — reintroducing one defect turns 54 green into
46 green and 8 red. **It earns its keep:** its first run over the boot-media package caught
a live `RESCRIPTUM_MEDIA_ADDR` with `RESCRIPTUM_MEDIA_DIR` still commented, which is a
startup error — the package would not have started at all.

### The desktop application

`packaging/dsm/payload/ui/` is a **real DSM application** — `SYNO.SDS.AppWindow`,
`syno_formpanel`, `syno_textfield`, `syno_combobox`, `syno_button` — not a page of ours in a
frame. `dsmuidir="ui"` makes DSM symlink it into
`/usr/syno/synoman/webman/3rdparty/rescriptum`, and `dsmappname` names the class `ui/config`
declares. It manages the server's configuration, shows its status and tails its log.

**ExtJS, not Vue, and the machine decided that.** DSM 7.2 ships a Vue framework and
Synology's current guide documents only that one — the first version of this was written
against it. The DS416j is capped at **DSM 7.1.1**, where `Vue` is undefined. ExtJS is on both
(7.1.1 and 7.2.2, measured), so one application covers every DSM this package supports;
`os_min_ver` is **7.1**, and 7.0 is not claimed because nothing has run there. The API is
documented in the ExtJS reference Synology generated for DSM, mirrored at
<https://github.com/DigitalBox98/SimpleExtJSApp> as `docs/synoextjsdocs.tar.gz`.

The design rule holds: nothing in `src/` knows any of this exists. What the server gained is
a *generic* `config` subcommand, and the application's backend — `ui/api.cgi` — is a hundred
lines of shell that authenticate and then shell out to `rescriptum-cli config` and `media`. **The panel never grows a rule of its own**: it starts a download by calling `media add`, which is where the digest rules are tested, and it follows one by watching the `.part` file that command already writes — a CGI cannot hold a request open for 1.5 GB, and nothing about progress had to be invented for the browser. The env-file
semantics stay in Rust where they are tested rather than being written a second time in `sh`.

**Four things were measured on the machine and every one of them is load-bearing. None is in
the developer guide** (they are in `docs/development/traps.md` at length):

- **A CGI there runs as the owner of the script**, which for a package tree is the package
  user. Not `http`, not root. That is what lets it read the `0600` env file it owns, and why
  it cannot start or stop anything — restarting goes through DSM's own
  `SYNO.Core.Package.Control`, from the application, with the administrator's session.
- **DSM does not authenticate that path.** An unauthenticated request gets `200`. So
  `authenticate.cgi` plus an `administrators` check *is* the door, and a write additionally
  needs a header a cross-origin page cannot make a browser send. Losing any of them would be
  silent, which is why `check-spk.sh` greps for them **with the comments stripped** — the
  first version of that check passed because the word appeared in a comment.
- **No `su`, ever.** It hangs a CGI outright without `</dev/null` (it reads the web server's
  stdin and waits forever) and then fails anyway as a non-root process. The script already is
  the user in question.
- **The JavaScript is named after the version.** `make-spk.sh` fixes every mtime for
  reproducibility, nginx serves that as `Last-Modified: 2019`, and a browser's heuristic
  freshness is then years: an upgraded package kept running the old application through a
  reinstall and a hard reload. Everything the app fetches itself carries `?v=` for the same
  reason.
- **The guide's own ExtJS example does not run**: `Ext.define` + `callParent` throws against
  `SYNO.SDS.AppInstance`. Declare with `Ext.define` (DSM's launcher finds the class that way)
  and chain with `superclass.constructor.call`. It is ExtJS **3.4.1** under an `Ext.define`
  shim.
- **Never add a method named `show` to the window.** `Ext.Window.prototype.show()` is what
  DSM calls to display it, so a tab-switching `show()` silently overrode it — the window
  built, laid out, rendered its taskbar thumbnail, and never appeared, **without throwing on
  either DSM version**. It cost a bisect from the guide's minimal example upwards. The same
  hazard applies to every other name on that prototype. Also: the taskbar requires
  `getWindowTitle()`, and `fieldLabel` only renders inside a form layout.

Bilingual through **DSM's own** text files (`ui/texts/{enu,fre}/strings`, `_S('lang')`
choosing) — but the app loads them itself, because DSM does not load them for a package
built without Synology's toolchain. The format and the locale directories stay Synology's;
only the loading is ours. `ui/config`'s `title` and `desc` are literals for the same reason:
an unresolved `section:key` renders as that literal text under the icon.

**Verified on a DSM 7.2.2 machine** (`vdsm/virtual-dsm` under emulation — `packaging/dsm/vm/`),
which found two bugs no fake-tree harness could: `ROOT` derived from `SYNOPKG_PKGDEST` (a
symlink target, so the env file landed where nothing reads it and the service never started),
and a *fresh* install restoring a removed installation's configuration out of a stale
`$SYNOPKG_TEMP_UPGRADE_FOLDER`. 47 checks green end to end. **The DS416j run has since happened too**, and found what the
VM could not: the ARMv7 musl build cannot run on Synology's 3.10 kernels (hence the glibc
target), and a Mac editing the answers share over SMB drops AppleDouble files that hijack a
machine's answer (hence hidden entries being skipped).

Load-bearing, and each one is a trap somebody has paid for:

- **A new setting never reaches an existing installation on its own.** The live env file
  is written only when absent, so boot media arrived on a DS416j with the folders made,
  the loaders seeded and 69/udp registered — and `RESCRIPTUM_BOOT_DIR` missing, which
  `boot check` reported as "boot assets are off". `etc/` surviving an uninstall means a
  reinstall does not fix it either. `postinst` appends keys the file has **never heard
  of** and touches nothing present; **a commented-out key counts as present**, which is
  how an operator says no.
- **`postinst` runs on an upgrade too.** It writes the env file **only when absent** —
  guarding on the file, not only on `SYNOPKG_PKG_STATUS` — and `preupgrade`/`postupgrade`
  carry it through `$SYNOPKG_TEMP_UPGRADE_FOLDER` as well. Unguarded, the obvious
  implementation replaces the user's port and tokens with defaults on every upgrade.
- **`postuninst` touches the package tree only.** Never the share, never the database,
  under any status — it runs during an upgrade as well. DSM deliberately does not remove
  the share; we do not do its restraint for it.
- **The scripts are not root.** `run-as: package` governs the scripts, not only the
  service. That is why the `0600` env file comes out owned correctly for free, and equally
  why the package cannot chown a user-supplied answers directory or call `synopkghelper`.
- **`start-stop-status` answers every verb.** `prestart` runs at boot and a non-zero exit
  stops the package from ever starting — the symptom is "works by hand, never after a
  reboot". `status` returns **3** for stopped; `1` means "crashed, stale pidfile".
- **The share does not exist during `postinst`** (`data-share` runs at package *start*), so
  creating the answers directory inside it belongs in `start`, and may never abort it.
- **`arch` takes family names.** `x86_64` covers every Intel platform, including ones
  Synology has not shipped; the family shorthand does *not* reach the Marvell ARMv7
  platforms, so the DS416j is `armada38x` by name. Widen only after the binary has run on
  an ABI's oldest-kernel member.
- **The outer tar is uncompressed**, `ustar`, with fixed mtimes and `0:0` ownership; the
  inner `package.tgz` is the gzipped part. A gzipped outer archive is rejected with
  "invalid file format" and nothing else.
- **`SYNOPKG_PKGDEST` resolves to `/volume1/@appstore/<pkg>`**, so the package root is the
  fixed `/var/packages/<pkg>`, never `dirname "$SYNOPKG_PKGDEST"`. `RESCRIPTUM_PKG_ROOT` is
  the seam that lets `lifecycle-test.sh` drive the scripts against a writable tree.
- **`etc/` and `var/` survive an uninstall** (they are symlinks into `@appconf`/`@appdata`),
  so the env file and its tokens outlive the package — said plainly in the Synology page.
- **`$SYNOPKG_TEMP_UPGRADE_FOLDER` outlives its upgrade**, so restoring from it requires
  `SYNOPKG_PKG_STATUS = UPGRADE` or a fresh install resurrects a removed configuration.
- **The firewall directory is `/usr/local/etc/services.d/`** (plural; the guide is wrong),
  and `port-config` acquires *after* `postinst` — the wizard's port does reach it. Both
  `port-config` and `usr-local-linker` acquire when the package is **enabled**, not at
  `postinst`.
- **The generated unit has no `Restart=`**: DSM does not restart the process if it dies.

**Changing anything under `packaging/dsm/` means running the machine**, not just the local
harness — the procedure is in `packaging/dsm/vm/README.md` (*Changing the package? This is
the procedure*), and `AGENTS.md` points at it. A DSM 7.2.2 VM already exists in Docker on
the maintainer's machine with a `clean` snapshot; `bootstrap.sh` sets one up from scratch,
`on-dsm.sh` drives it, and the run is destructive on purpose. It asks the server for a real
answer — a machine file merged over the group that claims it — rather than settling for
`/health`.

The harnesses catch a broken archive and broken scripts; only Package Center catches a
broken package. **A tag must not be the first time an `.spk` meets a DSM machine** — the
rig is `packaging/dsm/vm/`: `docker-compose.yml` runs Synology's own Virtual DSM (DSM 7.2,
close to the DS416j's 7.2.1). KVM makes it fast, not possible — without `/dev/kvm` the image
falls back to emulation on its own, about ten times slower, which is what
`docker-compose.emulated.yml` is for. What does stop a host is **14 GiB free**, hardcoded in
the image and not derived from `DISK_SIZE`. `run-vm.sh` is the loader-image fallback.
## Traps already hit (do not re-discover these)

These are the DSM-specific half of the root file's trap list. The long form of all of them is
in `docs/development/traps.md`.

- **There is exactly one route to port 69 on DSM 7, and it is `setcap`.** `run-as: root`
  in `conf/privilege` is refused with `synopkg` error **319**, `invalid package privilege
  content` — in `defaults` *and* as a per-action `ctrl-script`, the shape Synology's own
  packages use. A `security.capability` xattr in `package.tgz` installs and **Package
  Center strips it**. `setcap cap_net_bind_service=+ep` after install works;
  `net.ipv4.ip_unprivileged_port_start` does not exist on that kernel. Measured on a 7.2.2
  machine, all four.
- **Root on DSM 7 is gated on the *signature*, and `libsynopkg.so.1` says so.** Its
  strings carry the whole rule: a package failing `verifyPackageSignature` may not have a
  `ctrl-script` or `executable` section, must have `defaults.run-as` = `package`, and
  — the line that matters — `tool capabilities should not exist`. DSM's privilege format
  has a native `capabilities` field (documented since 7.0-40656), so a **signed** package
  declares `cap_net_bind_service` and never needs `setcap`. Synology's guide states it
  plainly — *"you are not able to install that package unless it is signed by synology"* —
  so it is their signature, not a trusted publisher's. The one documented bypass, a
  *development token*, is valid only on the NAS that generated its `debug.dat`, so it is
  not a distribution path. **The manual `setcap` is settled, not provisional**; no
  packaging change removes it.
- **`setcap` holds on the DS416j's volume, measured there.** The four routes to port 69
  were measured on an x86_64 VM whose `/volume1` is btrfs, `nodev` but not `nosuid`; a
  `nosuid` mount makes the kernel ignore file capabilities outright, which would have
  closed the last open route on the one machine this exists for. On the DS416j (ARMv7,
  `armada38x`) the package binds `udp/69` and `boot check` says
  `0.0.0.0:69 handed over ipxe-arm64.efi`.
- **A file capability does not survive an upgrade** — the new binary is a different file.
  That is why a failed TFTP bind is the **one** listener failure here that is not fatal:
  when it was, an upgrade took the answer endpoint down with it, failing every install in
  flight to report that a second port could not be opened. It warns, `boot check` exits
  non-zero, and the DSM panel shows a `tftp:` line.
- **A default computed at runtime must be computed in `settings()` too.** The DSM panel
  renders a variable's default as the field's value, so a default living only where the
  server consumes it shows as an empty box while the server runs on a value it derived.
  `RESCRIPTUM_PUBLIC_HOST` shipped that way. Two `KNOWN` entries are special-cased there
  — the worker count and the public host — and nothing in the type system says a third
  would need it.
