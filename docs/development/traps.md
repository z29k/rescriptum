---
title: Traps already hit
description: Things that cost time once. Reading this is cheaper than rediscovering them.
sidebar:
  label: Traps
  order: 11
---

# Traps already hit

Each of these cost real time. None is obvious from the code alone.

## Runtime, not compile time

**hyper panics if a timeout is set without a timer.** `http1::Builder::header_read_timeout`
requires `.timer(TokioTimer::new())`. Omit it and **every** connection panics at runtime —
it does not fail to compile. The integration tests caught this; unit tests could not have.

**`header_read_timeout` stops at the end of the headers.** hyper has no body-read timeout,
so a client that promises a body and sends nothing would park a connection indefinitely.
The whole-connection `tokio::time::timeout` in `connection()` is what covers that. **Both
are needed; neither is redundant.**

**hyper emits header names lowercased.** That is correct — they are case-insensitive — so
assert on a lowercased copy. See `has_header` in the integration tests.

## Performance

**`fs::metadata` per directory entry is a stat syscall each.** `DirEntry::file_type()`
comes back free with the `readdir` on Unix; only a symlink needs the stat to resolve. That
alone was worth **65% at 2,000 files**, before caching was added.

**Editing a group file's *contents* changes no directory mtime.** Only `RELOAD_BACKSTOP`
(1 s) picks that up, which is why the backstop is not redundant with the mtime check. An
integration test covers it.

**An aberrant `Content-Length` must be refused from the header**, not by letting `Limited`
trip after buffering a megabyte.

**Closing on a peer that is still writing discards the response you just wrote.** The
kernel sends a reset, and the reset throws away the unread bytes — so the client sees a
dropped connection, not your answer. `shed()` had exactly this: it wrote its `503` and
closed immediately, so the installer it was trying to tell *"retry"* got a connection
reset instead. It now drains briefly first, the way the admin API's `put()` already did.
A test at the connection cap pins it.

- **macOS lets an unprivileged process bind UDP port 69; Linux does not.** So a test that
  reaches the *default* TFTP address takes a different branch on each platform — `boot
  check` calls an obtainable-but-silent port a note and an unbindable one a problem, which
  is the right rule and exactly what makes the test platform-dependent. It passed locally
  and failed in CI for a reason that had nothing to do with the change. Any test that sets
  `RESCRIPTUM_BOOT_DIR` must also set `RESCRIPTUM_TFTP_ADDR=off` unless the probe *is* the
  subject; `tests/tftp.rs` covers the unbindable port on a high one.
- **A branch developed entirely offline has never met the CI.** This one accumulated 57
  commits before its first push, and the first run failed on two things no local run could
  see: a clippy five versions newer than the pinned local toolchain, and a Linux-only port
  permission. Push early enough to find out, or expect to.

## Selection and formats

**A Mac editing the answers directory over SMB can hijack a machine's answer.** macOS writes
an AppleDouble `._<name>` beside a file whose extended attributes the filesystem will not
take — `._proxmox.toml` has an extension that *is* on the allowlist. With a directory per
identity it is worse than it was when answers were flat: it is a **second `.toml` in a
directory that may hold only one**, and it sorts *before* the real one, so a rule that took
the first would hand every request a binary body. The machine being configured then receives
a parse error instead of its answer. `.DS_Store` is harmless only by luck (its extension is
not on the list). The file store skips every entry whose name starts with `.`; found on a
real NAS, not by reading anything.

**Normalizing a selector pattern strips `*` and `?`** unless you use `normalize_pattern` —
which turns every glob into a literal, quietly.

**In a text format, a placeholder inside a comment is still a placeholder.** `Kind::Text`
is an opaque string, so substitution runs over the whole document — a `{{ mac }}` written
in a `#` comment to *explain* templating still has to resolve, and fails `check` exactly
like a real one. Found while adding the `.ipxe` and `.cfg` worked examples.

**A GET has no body, so the haystack is empty.** Query values and path segments must feed
it too, or a document named after a MAC can never answer a preseed or kickstart fetch.

**quick-xml emits entity references as their own events.** Ignoring them welds the
surrounding text fragments together: `1 &lt; 2 &amp; 3` came back as `123`.

**Repeated XML siblings are not always a list.** If they carry a discriminating attribute
they are a keyed collection; treating them as a list replaced every `<component>` in an
unattend.xml with the one the overlay happened to mention.

**Two documents with the same stem are not duplicates.** An earlier `put` deleted the other
formats of a stem to avoid "two answers for one machine". That was the wrong model: they
are that machine's answers for two **operating systems**.

**Filter endpoints on the extension, not the `Kind`.** `.ks` and `.preseed` are both
`Kind::Text`; filtering by family would let a preseed answer `/rhel/ks`.

**An alias must be specific enough that nobody reaches it by accident.** `seed` was removed
as an endpoint alias: `s=http://server/seed/` is an ordinary NoCloud seed URL, and it
serves YAML.

## The admin API

**Read the request body before rejecting a request.** Answering and closing while the
client is still writing earns an `ECONNRESET` instead of the response. `put()` drains
first, then validates the identifier.

**Admin responses must set `Connection: close`.** Without it every test client waited out
the connection timeout — the suite took 30 s instead of 0.4 s — and the eventual drop
sometimes arrived as a reset rather than a clean EOF.

**Identifiers become filenames.** `export` and the file store build paths from machine ids
and group names, so `valid_id` is enforced at the API boundary **and** in both stores.

## Testing

**`cargo test` does not rebuild `target/debug/rescriptum`.** A manual check against a stale
binary once "reproduced" a bug that had already been fixed. Rebuild before poking at the
binary by hand.

**Cache-invalidation tests must share one `Answers` instance.** A test that constructs a
fresh one per call bypasses the cache entirely and silently proves nothing.

**Assert on parsed values, not on formatting.** Replacing a table with a scalar leaves the
key's original decor, so the output can read `value= 3` — valid TOML, different text.

**A `python`/`sed` patch that "succeeds" may have matched nothing.** Two edits in this
project's history silently no-opped and were only caught by checking test counts
afterwards. Assert the old text was found before writing.

## Packaging for DSM

**A shell script that works on macOS is not a shell script that works on CI.** Two found by
running the harnesses in a Linux container rather than trusting them: `stat -f '%Lp'` is the
format flag on BSD and *filesystem status* on GNU — where it **succeeds**, printing overlayfs
trivia into a variable that was supposed to hold a file mode, so the fallback never fires.
Ask GNU first (`stat -c '%a' || stat -f '%Lp'`), which fails cleanly on macOS. And `shasum`
is a Perl script that a minimal Debian does not have: `sha256sum` is coreutils and is
everywhere on Linux. Ubuntu runners carry both, which is exactly how a script like that ships
broken to everyone else.

**musl 1.2 cannot run on Synology's ARMv7 kernels, and the symptom names nothing.** Those
kernels are 3.10 and answer the *time64* syscalls with `EINVAL`; musl falls back to the
32-bit ones only on `ENOSYS`, so `clock_gettime`, `clock_nanosleep` and the timed futex all
fail. The binary installs, answers `--version`, and panics at `time.rs:131` with
`Os { code: 22, kind: InvalidInput }` the moment it wants a timestamp — which looks like an
ABI or a too-old-kernel problem and is neither. The armv7 target is glibc with a 2.17 floor
for this reason; 64-bit targets have no time32/time64 split and are unaffected. Proven with
a ten-line C probe on the machine, not by reading anything.

**`SYNOPKG_PKGDEST` is `/volume1/@appstore/<package>`, not `/var/packages/<package>/target`.**
The second is a symlink to the first, so `dirname "$SYNOPKG_PKGDEST"` is `/volume1/@appstore`
and everything hung off it — `etc/`, `var/`, `shares/` — lands where nothing reads it. The
package root is a fixed path. This one costs a service that installs perfectly and never
starts, and a fake-tree harness cannot catch it: in a tree you built yourself, `dirname` is
right by construction.

**`$SYNOPKG_TEMP_UPGRADE_FOLDER` outlives the upgrade that created it.** A *fresh* install
that reads it finds the configuration of an installation the user removed, and silently
restores it — tokens and all. Restoring from it has to require `SYNOPKG_PKG_STATUS = UPGRADE`.

**`etc/` and `var/` survive an uninstall.** They are symlinks into `/volume1/@appconf/<pkg>`
and `/volume1/@appdata/<pkg>`, which DSM keeps. So the env file, tokens included, stays on
the volume after the package is gone — which the documentation has to say, and which makes
a rig that does not clear them fail on the *next* run for reasons belonging to the last one.

**A DSM account named after the package user is destroyed with it.** `conf/privilege`'s
`username` creates a system user at install; an administrator of the same name is shadowed
by it and removed on uninstall.

**The firewall directory is `/usr/local/etc/services.d/`** — plural. The developer guide says
`service.d`, which does not exist. The `port-config` worker acquires **after `postinst`**, so
the wizard's port does reach the firewall entry on a fresh install.

**`port-config` and `usr-local-linker` acquire when the package is *enabled*,** not when
`postinst` runs: checked any earlier they are always absent.

**The generated unit has no `Restart=`** — `Type=oneshot`, `RemainAfterExit=yes`,
`TimeoutStartSec=3600`. DSM does not restart the process if it dies.

**`postinst` runs on an upgrade too, and it runs *before* `postupgrade`.** So "the env
file is absent" is not the same question as "this is a fresh install": on an upgrade where
`etc/` did not survive, writing defaults there destroys the user's port and tokens before
the restore ever runs. `postinst` checks `$SYNOPKG_TEMP_UPGRADE_FOLDER` before it decides.
Found by simulating that exact case, not by reading the documented sequence.

**The old version's `preuninst`/`postuninst` run during an upgrade.** Anything destructive
in them therefore runs every time somebody upgrades — and the *first published* `.spk` is
the one whose uninstall scripts will run during everybody's first upgrade. They cannot be
fixed later.

**`status` returning `1` means "crashed, stale pidfile"**, not "stopped". A cleanly stopped
package is `3`. Returning `1` tells Package Center the service died.

**`prestart` runs at boot**, and DSM calls it whether or not you wrote it —
`precheckstartstop` defaults to `"yes"`. A `case` that exits non-zero on an unrecognised
verb stops the package from ever starting after a reboot, with a symptom ("works by hand,
never after a reboot") that looks like anything but a missing case arm.

**The lifecycle scripts are not root.** `run-as: package` governs them, not only the
service — so a chown outside the package tree, or `synopkghelper`, fails, possibly
silently.

**`data-share` runs at package *start*, not at install**, so nothing in `postinst` may
assume the shared folder exists. And a username that does not match its permission list
creates the share and grants it to nobody, without a word.

**A logrotate stanza without `copytruncate` silently ends logging**: `log::init` opens the
file once and never reopens it, so a rotation moves the inode out from under a server that
carries on writing to a file with no name.

**A `.spk` whose outer tar is gzipped is rejected** with "invalid file format" and no
further detail. So is one carrying macOS `._` members. `check-spk.sh` asserts both.

**There is exactly one route to port 69 on DSM 7, and it is `setcap`.** All four were
tried on a 7.2.2 machine on 2026-08-27, because the claim "DSM 7 does not let an unsigned
package run as root" had sat in `CLAUDE.md` for a while with no measurement behind it —
true, but by luck.

| Route | Result |
|---|---|
| `"defaults": {"run-as": "root"}` in `conf/privilege` | **refused** — `synopkg` error **319**, `invalid package privilege content`, `stage: install_failed` |
| `"ctrl-script": [{"action":"start","run-as":"root"}]` — the shape Synology's *own* packages use (FileStation, QuickConnect and StorageManager all do) | **refused**, same error 319 |
| `cap_net_bind_service` embedded as a `security.capability` xattr in `package.tgz` | installs fine — the pax inner format is accepted — but **Package Center strips the xattr**, and `getcap` comes back empty |
| `setcap cap_net_bind_service=+ep` on the installed binary, as root, after install | **works**; the package then binds `udp/69` as its own unprivileged user alongside 8000 and 8001 |

`net.ipv4.ip_unprivileged_port_start` does not exist on that kernel, so that route is
closed too. `/volume1` is btrfs with `nodev` but **not** `nosuid`, so file capabilities do
work there, and `/usr/bin/setcap` exists at mode `0700`.

**Root on DSM 7 is gated on being a *Synology* package, and `libsynopkg.so.1` says so in
so many words.** Reading its strings on a 7.2.2 machine turns the measurement above into
an explanation. A package that does not pass the signature check (`verifyPackageSignature`
lives in the same library) is refused all of this:

```
Failed to pass privilege check, ctrl-script and executable section should not exist
Failed to pass privilege check, defaults should be provided and defaults.run-as should be package
Failed to pass privilege check, join-groupname should not contains admin group
Failed to pass privilege check, tool capabilities should not exist
Failed to pass privilege check, tool user should be package
Failed to pass privilege check, non-synology package should not use privilege migration
```

Which is why FileStation, StorageManager, QuickConnect and SecureSignIn all carry
`"ctrl-script": [{"action": "start", "run-as": "root"}]` in their own `conf/privilege` and
we cannot: the shape is legal, the signature is what makes it legal *for them*.

**The line that matters most is `tool capabilities should not exist`.** DSM's privilege
format has a native `capabilities` field — documented as
`"capabilities": "cap_chown,cap_net_raw"` on a `tool` entry since 7.0-40656, and
`SYNOPackageTool::Privilege::ChangeCapabilities` is right there in the library. A signed
package declares `cap_net_bind_service` and never needs `setcap` at all. **The mechanism we
want exists, is documented, and is closed to us.**

Synology's developer guide states the rule outright: *"If you are developing a package with
root privilege, you are not able to install that package unless it is signed by synology."*
So it is **their** signature, not any trusted publisher's — which answers what the library
string left open. SynoCommunity hit the same wall
([spksrc#4170](https://github.com/SynoCommunity/spksrc/issues/4170),
[#4215](https://github.com/SynoCommunity/spksrc/issues/4215)).

There is one documented bypass and it is **not a distribution path**: a *development
token*. Generate `debug.dat` from Support Center → Support Services, send it to Synology,
receive a signed token, drop it at `/var/packages/syno_dev_token`. It is valid **only on
the NAS that generated the `debug.dat`**, so shipping this way would mean every single user
doing a round trip with Synology before they could install. `setcap` is one local command
and strictly better for them.

Conclusion, and it is settled rather than provisional: **the manual `setcap` is the price
of not being signed by Synology, and no packaging change removes it.** If the package is
ever signed, the manual step and the boot-up task are both replaced by three lines in
`conf/privilege`.

**`setcap` works on a DS416j too, and that was not a given.** The four routes to port 69
were measured on a 7.2.2 VM, which is x86_64 with `/volume1` on btrfs mounted `nodev` but
not `nosuid` — and a volume mounted `nosuid` makes the kernel ignore file capabilities
entirely, which would have closed the last open route on the one machine this project
exists for. Measured on the DS416j (ARMv7, `armada38x`): the capability holds, the package
binds `udp/69` as its unprivileged user, and `boot check` reports
`0.0.0.0:69 handed over ipxe-arm64.efi` — a real read request answered with real data.

**The capability belongs to the file, so an upgrade drops it.** A new version replaces the
binary and the capability goes with the old one — which is why the package documents a
Task Scheduler boot-up task rather than a one-off command, and why a failed TFTP bind is
not fatal: when it was, that upgrade took the answer endpoint down too.

**Binding is not a health check, and it proves the opposite of what it looks like.** A
bind that *succeeds* on the TFTP port means nothing is listening — the degraded state, not
the healthy one — and a bind that fails cannot tell this server apart from another daemon
squatting the port, because both are `AddrInUse`. `boot check` therefore sends a real read
request and reports what a machine would get. The first version of it reported "already in
use — that is this server, if it is running" and a test with a squatter on the port
immediately showed that to be a guess.

**A new setting never reaches an installation that already exists**, unless something
puts it there. The live env file is written only when absent — correct, because an upgrade
must never replace somebody's port and tokens with defaults — but on its own that makes a
new feature invisible to every install that predates it. Boot media shipped with the
folders created, the loaders seeded and 69/udp registered with the firewall, and
`RESCRIPTUM_BOOT_DIR` never arriving, so `boot check` answered *"boot assets are off"* on a
DS416j that had everything else in place. `etc/` surviving an uninstall means even removing
and reinstalling does not fix it. The `.env.example` was no help, because nothing makes
anybody read it.

`postinst` now appends keys the live file has **never heard of**, touching nothing that is
present. **A commented-out key counts as present**, and that is the safety property: it is
how an operator says "I know about this one and I do not want it". Deleting a line means
"never heard of it" and gets it back; commenting it out means no, and is respected.

## The DSM desktop application

Eight things, measured on a DSM 7.2.2 virtual machine and on a DS416j running 7.1.1, and
none of them in the developer guide.

**A default computed at runtime has to be computed in `settings()` too.** The panel renders
a variable's default as the field's value, so a default that exists only where the server
consumes it shows as an empty box — while the server runs on an address it derived and
never displayed. `RESCRIPTUM_PUBLIC_HOST` shipped that way; the operator had no way to see
which address their machines would be sent to short of reading the startup log. Two entries
in `KNOWN` are like this, and both are special-cased in `settings()`: the worker count and
the public host. A third would need the same treatment, and nothing in the type system says
so.

**A CGI under `/webman/3rdparty/<pkg>/` runs as the owner of the script.** Not as `http`,
and not as root — as whoever owns the file. DSM chowns a package's tree to the package
user, so the application's backend runs as `rescriptum` and can read the `0600` env file it
owns, which is the entire reason the configuration can be edited while the server is
stopped. Proven by chowning the same script two ways and watching `id` change. A script
left owned by root **does** run as root there, so do not leave one lying about.

**That path is not authenticated by DSM.** An unauthenticated request reaches the script
and is answered `200`. Whatever guards a package's CGI, the package wrote it — here that is
`authenticate.cgi` plus an `administrators` check, and losing either would be silent.

**`su` in a CGI hangs the request.** Without `</dev/null` it inherits the CGI's stdin — a
pipe from the web server that nothing will close — reads from it, and never returns. The
status page simply stopped mid-answer. Then, once that was fixed, it failed anyway with
"Permission denied", because a non-root process cannot become anybody. Both were wasted
effort: the script already *is* the user in question, so a plain `test -r` was the answer
all along.

**The framework a package can use is the machine's choice, not Synology's guide's.** DSM
7.2 ships a Vue UI framework and the current guide documents only that one. The DS416j is
capped at DSM 7.1.1, where `Vue` is undefined — so an application built on it installs and
gives that machine an icon that opens nothing. ExtJS is on both (7.1.1 and 7.2.2 measured),
which is why there is one application rather than two.

**The guide's own ExtJS example does not run.** It declares classes with `Ext.define` and
chains with `callParent`; against `SYNO.SDS.AppInstance` that throws `Cannot read properties
of null (reading 'apply')` before the window ever appears. This is **ExtJS 3.4.1** with an
`Ext.define` shim over it: use `Ext.define` for the declaration — DSM's launcher finds the
class that way and it does set `superclass` — and then call
`MyClass.superclass.constructor.call(this, config)` rather than `callParent`.

**DSM's taskbar calls `getWindowTitle()` on the window.** Without a title it throws from
inside DSM's own taskbar bundle, and the application then fails to open at all — with a
stack trace that names Synology's code and not yours.

**Do not name a method `show`.** `Ext.Window.prototype.show()` is what DSM calls to display
the window, so a `show(which)` added for switching tabs silently overrode it: the window was
built, laid out, and rendered a correct thumbnail in the taskbar preview — and never
appeared. **Nothing threw**, on either DSM version, which is what made it expensive: it was
found by bisecting from the guide's minimal example upwards. Everything added to that
prototype shares a namespace with every method of `Ext.Window`, and that is a large
namespace.

**`fieldLabel` is drawn by the form layout, not by the field.** A `syno_displayfield` in a
plain `Ext.Panel` renders its value and silently drops its label, which turned the status
page into a bare column of values with nothing saying what they were. `SYNO.ux.FormPanel`,
or `layout: 'form'`.

**Reproducible builds and browser caches disagree, and the browser wins.** `make-spk.sh`
gives every packaged file a fixed mtime so the same inputs produce a byte-identical `.spk`.
nginx turns that into `Last-Modified: 2019` with no `Cache-Control`, and a browser's
heuristic freshness is a tenth of the file's apparent age — years. An upgraded package went
on running the **old** JavaScript against the new backend, through a reinstall and a hard
reload. The application's file is therefore named after the version and everything it
fetches itself carries `?v=`; `check-spk.sh` asserts the name still moves.

## Behaviour changes worth remembering

**Answer documents must now be valid.** Before merging they were served as opaque bytes, so
a malformed one reached the installer; now it is a `500` with the parse error in the log.
That is the better failure, but it *is* a behaviour change — fixtures written as YAML-ish
text stopped working when it landed.

**`{{ machine }}` is bound only when a machine document matched.** A machine claimed by a
group's `members`, with no document of its own, resolves with `machine: None` — so
`{{ machine }}` in a group fails for exactly the members it was meant to cover. Use a
request fact such as `{{ mac }}` there.
