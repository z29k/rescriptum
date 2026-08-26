---
title: Synology DSM 7
description: The original target — an ARMv7 DS416j with 512 MB and no Docker. A Package Center install, what it does and does not do for you, and the manual route if you prefer it.
sidebar:
  label: Synology DSM 7
  order: 2
---

# Synology DSM 7

A Synology DS416j is why this project exists: ARMv7, 512 MB of RAM, DSM 7, no Docker. A
static binary with no runtime is not an aesthetic preference there — it is the only thing
that fits.

DSM 7 does run systemd, but it offers no supported place for a unit of your own: files in
`/usr/lib/systemd/system` are Synology's, and a DSM update is free to replace them. The
supported route to a service is a **package** — install one and DSM generates
`pkgctl-rescriptum.service` from it. That is what this page leads with; the older
[Task Scheduler route](#without-the-package) still works and is kept at the bottom.

## Install the package

Download the `.spk` for your model from the
[releases page](https://github.com/z29k/rescriptum/releases):

| File | For |
|---|---|
| `rescriptum-<version>-armv7.spk` | DS416j and other Marvell `armada38x` models |
| `rescriptum-<version>-x86_64.spk` | every Intel model |

Not sure which? Ask the machine:

```console
$ ssh admin@nas synogetkeyvalue /etc.defaults/synoinfo.conf unique
synology_armada38x_ds416j
```

Then **Package Center → Manual Install**, pick the file, and click through the warning that
the package is not verified by Synology. That warning is not about this package in
particular: DSM 7 removed third-party signing altogether and no longer offers a trust-level
setting, so every non-Synology package shows it. Our verification is the SHA-256 sum
published beside the `.spk`:

```console
$ shasum -a 256 -c rescriptum-0.1.0-1-armv7.spk.sha256
```

The wizard asks two things — **where the answers live** and **which port to listen on** —
and then the package:

- creates a **`rescriptum` shared folder** and grants itself read/write access to it (if
  you already have one by that name, it is kept and simply gains the grant);
- creates the `answers` directory inside it at every start;
- **registers the port** with the DSM firewall, so the service is selectable by name;
- links **`rescriptum-cli`** into `/usr/local/bin`;
- starts at boot, and stops and starts from Package Center like anything else.

## What the package does not do

Four things worth knowing before they surprise you.

- **It does not open the firewall.** Registering the port makes *rescriptum* appear by name
  in the rule editor instead of you typing a number. If your firewall is on with a
  default-deny rule, you still have to create the rule.
- **It does not tell you about updates.** There is no package source to poll — the
  distribution model is: download the new `.spk` from the releases page and install it by
  hand, for an upgrade as much as for a first install. Watch the releases.
- **A custom answers path is yours to permission.** The package runs unprivileged and
  cannot grant itself access to a folder you name; if you point it outside the `rescriptum`
  share, give the `rescriptum` user read access yourself.
- **The share's permission is reapplied at every start.** If you deliberately narrow it,
  you will find it restored the next time the package starts.

## Where everything lives

| What | Where | Survives an upgrade | Survives uninstall |
|---|---|---|---|
| binary, `rescriptum-cli`, the example env file | `/var/packages/rescriptum/target/` | no — replaced | no |
| **the env file** | `/var/packages/rescriptum/etc/rescriptum.env` | **yes** | **yes** — see below |
| log, pidfile, captures | `/var/packages/rescriptum/var/` | yes | **yes** |
| **answers** | `/var/packages/rescriptum/shares/rescriptum/answers/` | **yes** | **yes — always** |
| **the SQLite database**, if you use one | beside the answers, in the same share | **yes** | **yes — always** |

Use the `shares/` path rather than `/volume1/…`: it is a symlink DSM maintains, so it keeps
working on a NAS whose data is not on volume 1.

Uninstalling leaves the shared folder and everything in it alone. That is both DSM's own
behaviour and ours: when the store is SQLite, the database *is* your answers.

**Uninstalling also leaves your configuration behind, and that is worth knowing.**
`etc/` and `var/` are symlinks into `/volume1/@appconf/rescriptum` and
`/volume1/@appdata/rescriptum`, which DSM keeps — so the env file stays on the volume after
the package is gone, **with whatever tokens are in it**. Reinstalling picks it back up,
which is usually what you want. If you are removing rescriptum for good and it held a
token, delete `/volume1/@appconf/rescriptum` yourself.

## The desktop application

The package installs an application on the DSM desktop — the icon is in the main menu, and
Package Center's **Open** button leads to it. It is a real DSM application, built on the
desktop's own UI framework, so it is in the DSM theme and in the DSM language; the French
of a French DSM is the application's French too.

It has three tabs:

- **Settings** — every configuration variable, as a form. Each field says where its value
  comes from, and a value the *environment* sets is shown but locked, because editing the
  file would not change it. Saving writes the file and offers to restart the package,
  since the server reads its configuration once, at startup.
- **Status** — the version, whether the package is running, the answers folder and whether
  it is really readable *by the service's own user*, and the output of `check`.
- **Log** — the last lines of the request log and of `startup.log`.

Three properties are worth knowing rather than discovering:

- **It edits the file, not the running server.** So it still works when the server will not
  start, which is exactly when a settings panel earns its place. A change that would leave
  the server unable to start is refused before anything is written, with the reason shown.
- **It never shows you a token.** `RESCRIPTUM_ANSWER_TOKEN` and `RESCRIPTUM_ADMIN_TOKEN`
  appear as *set* or *not set*, and an empty box means "leave it alone" rather than "clear
  it". Typing a new one replaces it.
- **It requires a DSM administrator.** Being signed in to DSM is not enough. See
  [security](./security.md#the-desktop-application) for why that check is the whole door.

*Restart now* stops and starts the package through DSM itself, so **DSM closes the window
while it does** — open it again to see the new state. The application says so next to the
button rather than letting it surprise you.

It needs **DSM 7.1 or newer** (`os_min_ver="7.1-42661"`). It is built on DSM's ExtJS
framework, which is present on 7.1.1 and on 7.2.2 — both measured. DSM 7.2 ships a newer
Vue framework and Synology's current guide documents only that one; the DS416j this project
exists for is capped at 7.1.1, where `Vue` is undefined, so ExtJS is what covers every DSM
this package supports rather than only the recent ones. 7.0 is not claimed because nothing
has been run there.

## Configuring it

The application above is the comfortable way. Everything it does can also be done from a
shell, and on a machine where the desktop is not to hand that is the faster route:

```console
$ sudo rescriptum-cli config
env file: /var/packages/rescriptum/etc/rescriptum.env

  RESCRIPTUM_STORE            files                             default
  RESCRIPTUM_ANSWERS_DIR      /var/packages/rescriptum/shares/rescriptum/answers   file
  RESCRIPTUM_LISTEN_ADDR      0.0.0.0:8000                      file
  …

$ sudo rescriptum-cli config set RESCRIPTUM_LOG=problems
wrote /var/packages/rescriptum/etc/rescriptum.env
```

`config set` keeps the file's comments, uncomments a setting rather than duplicating it,
and **refuses a change that would stop the server starting**. Its exit code says whether
the configuration is one the server would start on, which makes it usable from a script.

Underneath both is the same file, and editing it by hand is still perfectly reasonable:

```console
$ sudo vi /var/packages/rescriptum/etc/rescriptum.env
```

`postinst` writes it complete on a fresh install, with the variables in use uncommented and
the rest commented with a line saying what they do. **Stop and start the package from
Package Center to apply a change** — the server reads the file at every start.

An upgrade never touches it. The complete example for the version you have is at
`/var/packages/rescriptum/target/etc/rescriptum.env.example`, rewritten on every install
and upgrade, which is how a new variable becomes visible without disturbing your live file.
Every variable is in the [configuration reference](../reference/configuration.md).

The file is `chmod 600` and owned by the package user. It is where
`RESCRIPTUM_ANSWER_TOKEN` and `RESCRIPTUM_ADMIN_TOKEN` live, and — being under `etc/` — it
is a plausible passenger in a DSM configuration backup. Worth knowing rather than
discovering.

The [admin API](./admin-api.md) is off by default and, when you enable it, should stay on
loopback and be reached through an SSH tunnel; it is deliberately *not* registered with the
firewall. It also requires `RESCRIPTUM_STORE=sqlite` and a token of at least 16 characters,
both of which are **startup errors** — so getting them wrong shows up as a package that
will not start, with the reason in `/var/log/packages/rescriptum.log`.

## Putting answers in place

Drop files into the `rescriptum` shared folder's `answers` directory, over File Station or
over SSH, exactly as you would anywhere else — see [writing answers](../answers/index.md).
Then validate them **as the package user**:

```console
$ sudo -u rescriptum rescriptum-cli check
```

The `sudo -u` matters. Run as root it succeeds whatever the shared folder's permissions
say, which makes a successful run meaningless. `rescriptum-cli` is the packaged wrapper: it
names the env file, so `check` and `render` look at this machine's answers rather than at
`/srv/answers`.

## The firewall

**Control Panel → Security → Firewall** — create a rule allowing *rescriptum* from your
provisioning network. The service appears by name because the package registered its port.

DSM's firewall is the single most common reason a machine "never contacts the server".

If you change the port later, edit `RESCRIPTUM_LISTEN_ADDR` in the env file and then move
the firewall entry, which does not follow by itself:

```console
$ sudo /usr/syno/sbin/synopkghelper update rescriptum port-config
```

## The log

`RESCRIPTUM_LOG_FILE` points the server at `/var/packages/rescriptum/var/rescriptum.log`,
and the package installs a logrotate stanza for it — weekly, eight kept, `copytruncate`
(the server opens its log once and never reopens it, so anything else would silently end
logging). Beside it, `var/startup.log` holds what the server says before it knows where its
log lives: a configuration error, a malformed env file.

Once a rollout is routine, `RESCRIPTUM_LOG=problems` keeps the failures and drops the
successful answers, which are the only high-volume thing in there.

## When it will not start

Three places say why, in this order:

```console
$ cat /var/log/packages/rescriptum.log        # the package scripts' own output
$ cat /var/packages/rescriptum/var/startup.log  # what the server said before it had a log
$ cat /var/packages/rescriptum/var/rescriptum.log
$ systemctl status pkgctl-rescriptum          # what DSM's service manager saw
```

A **refused configuration** — an admin token under 16 characters, a store that cannot be
opened — is reported *after* the server knows where its log lives, so it lands in
`rescriptum.log`; a malformed env file is reported before, and lands in `startup.log`. The
package's `start` prints the tail of both when the server exits immediately, so Package
Center shows you the reason rather than only the failure.

**DSM does not restart the process if it dies.** The unit it generates is `Type=oneshot`
with `RemainAfterExit=yes` and no `Restart=`, so a server that exits stays stopped until you
start it from Package Center. That is not a regression — the Task Scheduler route did not
restart it either — but it is worth knowing before you rely on it.

A package that installs, starts, and then answers `404` to everything is almost always the
answers directory: check `sudo -u rescriptum rescriptum-cli check`. On a NAS with an
encrypted shared folder, that is also what a boot before the volume is unlocked looks like
— unlock it and restart the package.

## Verify

```console
$ curl http://NAS_IP:8000/health
OK
```

## Without the package

The manual route still works, and is the honest choice if you would rather not install a
package at all.

Use the **`armv7-unknown-linux-gnueabihf`** build (or `x86_64-unknown-linux-musl`, or
`aarch64-unknown-linux-musl` for a newer ARM model) from the
[releases page](https://github.com/z29k/rescriptum/releases), or cross-compile one yourself
(see [building](../../development/building.md)).

```console
$ scp rescriptum admin@nas:/volume1/netboot/rescriptum
$ ssh admin@nas chmod +x /volume1/netboot/rescriptum
$ ssh admin@nas mkdir -p /volume1/netboot/answers
```

If ARMv7 misbehaves, confirm the real architecture before assuming:

```console
$ ssh admin@nas uname -m
armv7l
```

**Take the ARMv7 build, not a musl one you built yourself.** The published `armv7` binary
is linked against glibc 2.17, which DSM has; a musl build of the same code installs, answers
`--version`, and then dies the moment it wants the time. Synology's 3.10 kernels answer the
*time64* syscalls with `EINVAL` rather than `ENOSYS`, and musl 1.2 only falls back on
`ENOSYS` — the [build page](../../development/building.md#why-armv7-is-the-one-target-that-is-not-musl)
has the measurement. The x86_64 and aarch64 builds are static musl and unaffected.

```console
$ file rescriptum
ELF 32-bit LSB pie executable, ARM, EABI5 version 1 (SYSV), dynamically linked, ...
```

`RESCRIPTUM_ANSWERS_DIR` defaults to `/srv/answers`, which does not exist on DSM, so set it
explicitly. The env file below is the tidiest place to do that.

**Control Panel → Task Scheduler → Create → Triggered Task → User-defined script**

| Field | Value |
|---|---|
| Event | **Boot-up** |
| User | `root` |
| Command | see below |

If you use a token, **do not put it in that box.** Anything in a process's arguments — and
in DSM's case, in the task definition — is readable by every user on the machine through
`ps`. Put the configuration in a root-only file and name it instead:

```sh
# /volume1/netboot/rescriptum.env   (chmod 600, owned by root)
RESCRIPTUM_ANSWERS_DIR=/volume1/netboot/answers
RESCRIPTUM_LOG_FILE=/volume1/netboot/rescriptum.log
RESCRIPTUM_STORE=sqlite
RESCRIPTUM_DB_PATH=/volume1/netboot/answers.db
RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
RESCRIPTUM_ADMIN_TOKEN=…
RESCRIPTUM_ANSWER_TOKEN=…
```

```sh
# the Task Scheduler entry runs this
RESCRIPTUM_ENV_FILE=/volume1/netboot/rescriptum.env exec /volume1/netboot/rescriptum
```

**Prefer this to sourcing it.** The older form —
`. /volume1/netboot/rescriptum.env && exec …` — works, and still does, but it fails
*silently*: drop the leading `.`, mistype a line, or get the permissions wrong, and the
shell sources nothing while the server comes up on its **defaults** — the default answers
directory, no admin token, and not a word about it in the log. With `RESCRIPTUM_ENV_FILE`
the binary reads the file itself and **refuses to start** if it cannot. It also warns if
the file is readable by anyone but root, and names any key it does not recognise, so a
`RESCRIPTUM_ADMIN_TOKENN` is caught rather than quietly ignored.

Details of the format are in the
[configuration reference](../reference/configuration.md#the-env-file).

Run the task once by hand from the Task Scheduler rather than waiting for a reboot to find
out it does not work. Then open the port in the firewall by number, and rotate the log
yourself — the server does not, and nothing else will either.

### Replacing a running instance

```console
$ ./deploy.sh admin@nas
```

It builds for ARMv7, [checks the answers first](../answers/validating.md), copies the
binary under a temporary name so a half-copied file is never executed, restarts it, and
confirms `/health` responds. Details in
[deployment](./deployment.md#replacing-a-running-instance).

The Task Scheduler entry is still what starts it after a reboot — `deploy.sh` only replaces
what is running now. On a packaged install, use Package Center instead.

## Shutdown

Both routes send `SIGTERM`, which the server handles: it stops accepting and exits. There
is no state to lose either way.

## What to expect from a DS416j

512 MB and an ARMv7 core is not much, and it does not need to be. Measured on a DS416j
running the package, over the LAN: **3–4 ms to compose and serve an answer**, network round
trip included, for a machine claimed by a group and merged with its own file. A connection
costs kilobytes rather than a thread, the directory listing is cached and invalidated by mtime
rather than walked per request, and a group with no per-machine overrides is rendered once
at load and served afterwards as a prepared string.

The one thing worth knowing: filesystem work happens on a blocking thread pool, because
`read_dir` on a NAS with a sleeping disk is not a fast call, and blocking an async worker
would stall every other connection it was driving.
