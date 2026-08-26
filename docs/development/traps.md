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

## Selection and formats

**A Mac editing the answers directory over SMB can hijack a machine's answer.** macOS writes
an AppleDouble `._<name>` beside a file whose extended attributes the filesystem will not
take — `._98-fa-9b-50-d8-10.toml` has an extension that *is* on the allowlist, and
normalization strips the leading `._`, so it claims the same identity as the real file with a
body that is binary. The machine being configured then receives a parse error instead of its
answer, and `check` reports the failure against the *group* as well. `.DS_Store` is harmless
only by luck (its extension is not on the list). The file store now skips every entry whose
name starts with `.`; found on a real NAS, not by reading anything.

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

## Behaviour changes worth remembering

**Answer documents must now be valid.** Before merging they were served as opaque bytes, so
a malformed one reached the installer; now it is a `500` with the parse error in the log.
That is the better failure, but it *is* a behaviour change — fixtures written as YAML-ish
text stopped working when it landed.

**`{{ machine }}` is bound only when a machine document matched.** A machine claimed by a
group's `members`, with no document of its own, resolves with `machine: None` — so
`{{ machine }}` in a group fails for exactly the members it was meant to cover. Use a
request fact such as `{{ mac }}` there.
