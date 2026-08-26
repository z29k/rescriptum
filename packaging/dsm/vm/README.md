# The test rig

Three places run the same checks, and each one proves something the others cannot.

| Where | What it proves | Cost |
|---|---|---|
| **A fake package tree**, anywhere — `../lifecycle-test.sh` | everything the *scripts* decide: the env file written once, the wizard's values and their absence, the service starting and answering, the exit codes Package Center reads, an upgrade that must not touch a hand-edited configuration, an uninstall that must not touch the answers | seconds, no DSM, **runs in CI on every push** |
| **A DSM 7 VM** — `run-vm.sh`, then `on-dsm.sh` | DSM's own machinery: the `data-share` worker and its ACL, the `port-config` worker, the generated systemd unit, logrotate against a live descriptor, and whether Package Center accepts the archive at all | minutes per cycle, and a snapshot to roll back to |
| **The DS416j** — the same `on-dsm.sh` | that all of the above is true on ARMv7, on the machine this project exists for | slow, and it is somebody's NAS |

**Nothing ships on VM evidence alone.** The VM is x86_64: it tests the *packaging*, and
tells you nothing about the ARMv7 binary — which is the one thing already covered, since
the cross-build and its statically-linked assertion are CI gates. The DS416j is the
verdict.

## Getting a machine: Docker, on a Linux host with KVM

[`docker-compose.yml`](docker-compose.yml) is the short route. The `vdsm/virtual-dsm` image
downloads **Synology's own Virtual DSM release**, so there is no loader to find and nothing
to patch, and it installs **DSM 7.2** by default — close enough to the DS416j's own 7.2.1
that the rig resembles the target.

```console
$ docker compose -f packaging/dsm/vm/docker-compose.yml up -d
$ open http://<host>:5000        # DSM's setup wizard, once
```

Then, in DSM: **Control Panel → Terminal & SNMP → Enable SSH service**. The rig drives the
machine over SSH — the compose file publishes it on 2222 — and copies two `.spk` files and
`remote-check.sh` to it.

Three things about the host, and only the last one is a wall:

- **KVM makes it fast; it is not what makes it possible.** With `/dev/kvm` (a Linux x86_64
  host — on Proxmox, set the guest's CPU type to `host`) the machine runs at near-native
  speed. Without it, QEMU emulates and the image says so itself: *"about 10 times slower"*.
  On an ARM host it works that out on its own — `init.sh` compares the host architecture
  against the guest's and disables acceleration rather than refusing — so a Mac runs
  [`docker-compose.emulated.yml`](docker-compose.emulated.yml), which binds no device. A
  long first boot, not a wall.
- **14 GiB free where the storage lives**, hardcoded in the image (`minSpace` in its
  `install.sh`) and *not* derived from `DISK_SIZE`, so a smaller disk does not lower it.
  This is the one that actually stops a machine.
- **Synology's EULA for Virtual DSM does not permit installation on non-Synology
  hardware.** That is the operator's call, not this repository's.

Two host ports collide often enough to be worth naming: **5000 is AirPlay Receiver on
macOS**, and 8000 is whatever you already run there. `DSM_WEB_PORT`, `DSM_SSH_PORT` and
`DSM_APP_PORT` move them; `DSM_STORAGE` moves the disk.

There is no snapshot command: the machine's whole state is the `storage/` directory beside
the compose file, so `docker compose down && cp -a storage storage.clean` is the snapshot,
and copying it back is the restore. Take one as soon as the wizard is done, because the
next thing the rig does is try to break the package on purpose.

### Or a loader image, by hand

`run-vm.sh` boots a DSM image you supply under plain QEMU — the fallback when Docker with
KVM is not available, or when the machine has to be something other than Virtual DSM. It
takes the loader image and gives it the hardware, the disks and the port forwards that
matter; **finding that image is not something this repository automates.**

Whatever you boot, ask it what it thinks it is rather than assuming — `on-dsm.sh` starts by
printing exactly that:

```console
$ ssh admin@nas cat /etc.defaults/VERSION
$ ssh admin@nas synogetkeyvalue /etc.defaults/synoinfo.conf unique
```

**`kvmx64` is not what a QEMU-booted DSM reports.** That is Synology's own VirtualDSM
platform, which runs under Virtual Machine Manager on a NAS — something the DS416j cannot
host. A loader-booted DSM presents whatever model it emulates.

## What running it actually taught us

Everything below was found by pointing this at a DSM 7.2.2 machine, not by reading the
developer guide — and several of them contradict it.

| | |
|---|---|
| `SYNOPKG_PKGDEST` is **`/volume1/@appstore/<pkg>`** | `/var/packages/<pkg>/target` is only a symlink to it, so `dirname "$SYNOPKG_PKGDEST"` is *not* the package root. Deriving it that way put the env file somewhere nobody reads and the service never started |
| **`etc/` and `var/` outlive an uninstall** | they are symlinks into `/volume1/@appconf/<pkg>` and `/volume1/@appdata/<pkg>`, which DSM leaves behind. The env file — tokens included — stays on the volume |
| `$SYNOPKG_TEMP_UPGRADE_FOLDER` **outlives its upgrade** | a later *fresh* install found the removed installation's configuration there and restored it. The restore now requires `SYNOPKG_PKG_STATUS = UPGRADE` |
| The firewall directory is **`/usr/local/etc/services.d/`** | plural. The guide says `service.d`, which does not exist on the machine |
| `port-config` acquires **after `postinst`** | so the wizard's port does reach the firewall entry on a fresh install — this answers a question the plan left open |
| The generated unit has **no `Restart=`** | `Type=oneshot`, `RemainAfterExit=yes`, `TimeoutStartSec=3600`. DSM does not restart the process if it dies |
| A non-login shell has **`PATH=/usr/bin:/bin:/usr/sbin:/sbin`** | `synopkg`, `synogetkeyvalue` and `synopkghelper` are all outside it |
| **SFTP is off**, so `scp` fails | `subsystem request failed on channel 0`. The rig copies with `ssh 'cat >'` |
| DSM's logrotate compresses with **xz** | the rotated file is `rescriptum.log.1.xz`, and `find` will not see it through the `var` symlink without `-L` |
| **Auto Block locks the rig out** | a few failed connections during a reboot are enough; every login is then refused, the web API with `"code":407`. `bootstrap.sh` turns it off |
| **Never name the rig's admin after the package user** | DSM's package user shadows a DSM account of the same name and takes it with it on uninstall |

## The loop

```console
$ ./build.sh x86_64-unknown-linux-musl              # what the VM runs
$ export RIG_SSH_OPTS="-o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$PWD/packaging/dsm/vm/known_hosts"
$ packaging/dsm/vm/on-dsm.sh rescriptum@127.0.0.1 -p 2222 -i ~/.ssh/rescriptum-rig
```

`RIG_SSH_OPTS` exists because a VM you rebuild gets a new host key every time, and ssh is
right to refuse it — the default has to suit the *real* NAS. It keeps the rig's keys in a
file of their own rather than in yours. Against the DS416j, drop it:

```console
$ ./build.sh armv7-unknown-linux-gnueabihf
$ packaging/dsm/vm/on-dsm.sh admin@nas
```

`on-dsm.sh` builds both `.spk` files it needs (build 1 and build 2 of the same version —
the upgrade is the most valuable test here), copies them and `remote-check.sh` to the
machine, and runs it as root.

Against the real thing, only the target changes:

```console
$ ./build.sh armv7-unknown-linux-gnueabihf
$ packaging/dsm/vm/on-dsm.sh admin@nas
```

## Changing the package? This is the procedure

Anything under `packaging/dsm/` — a lifecycle script, `conf/resource`, the wizard, the env
file's contents — is **not proven by the local harness alone**. `lifecycle-test.sh` drives
the scripts against a tree it built itself, and that is exactly why it cannot see the class
of bug that matters here: two of the three real defects found so far were invisible to it
(a package root derived from a symlink target, and a stale upgrade folder resurrecting a
removed configuration). Run the machine.

1. **Cheap gates first** — they take seconds and catch most mistakes:

   ```console
   $ ./packaging/dsm/make-spk.sh x86_64 --bin target/release/rescriptum --out /tmp/spk
   $ ./packaging/dsm/lifecycle-test.sh /tmp/spk/rescriptum-*-x86_64.spk
   $ ./packaging/dsm/check-spk.sh /tmp/spk/rescriptum-*-x86_64.spk
   ```

   The `--bin` is not optional on a machine that is not x86_64 Linux: the harness runs the
   packaged binary, so give it one this host can execute.

2. **Restore the machine**, so the run starts from a known state rather than from whatever
   the last one left:

   ```console
   $ packaging/dsm/vm/snapshot.sh restore clean
   $ docker compose -f packaging/dsm/vm/docker-compose.emulated.yml up -d
   ```

   Then wait for it — under emulation a boot is minutes, and `ssh … 'sudo -n true'`
   succeeding is the signal.

3. **Run the machine checks**, which build both `.spk` files, install, start, ask for a real
   answer, upgrade and uninstall:

   ```console
   $ ./build.sh x86_64-unknown-linux-musl
   $ export RIG_SSH_OPTS="-o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$PWD/packaging/dsm/vm/known_hosts"
   $ packaging/dsm/vm/on-dsm.sh rigadmin@127.0.0.1 -p 2222 -i ~/.ssh/rescriptum-rig
   ```

4. **Open the application, with your eyes.** No script can assert that a window renders.
   Sign in to DSM, open the main menu, and click **rescriptum**:

   - the icon is there, and the window opens;
   - the labels are words, not `ui:settings` — that is the sign the text files loaded;
   - **Réglages** shows this machine's real configuration, **État** says the package is
     running and the answers folder is readable, **Journal** shows actual log lines;
   - change one value, *Enregistrer*, then *Redémarrer maintenant* — DSM closes the window
     while it restarts, which is expected. Reopen it and the value is still there;
   - after an **upgrade**, check the labels again. If the window looks like the previous
     version's, the JavaScript is being served from the browser's cache and the versioned
     filename has stopped working.

   `SYNO.SDS.AppLaunch('SYNO.SDS.App.Rescriptum.Instance', {}, false, null, null)` in the
   browser console opens it without the menu, which is quicker when iterating.

5. **Read the `·` lines, not only the `✓`.** They are where the machine tells you things no
   assertion covers: which directory the firewall entry landed in, what the generated unit
   says, whether a worker ran before or after `postinst`.

6. **A failure is a question, not a verdict.** Three of the six things that have gone red
   here were the rig's own fault — checking a resource worker before its window opened, a
   canary file that was a valid answer document, `find` not following a symlink. Read
   `/var/log/packages/rescriptum.log` and `startup.log` before changing the package.

7. **Green on the VM is not green.** It is x86_64 and says nothing about the ARMv7 binary.
   `on-dsm.sh admin@nas` against the DS416j is the verdict, and nothing is released without
   it.

## It is destructive on purpose

`remote-check.sh` hand-edits the env file, drops a canary in the shared folder, upgrades
over both, and then uninstalls. That is not incidental — those two guards are the most
expensive things in this package to get wrong, the first published `.spk` is the one whose
uninstall scripts will run during everybody's first upgrade, and a guard that was never
watched failing proves nothing.

So: **point it at a machine whose answers nobody cares about until it has passed once.**
It leaves the shared folder behind deliberately, and says so — removing it is the one
thing this must never do.

## What it writes down

Several of the plan's open questions are answered by watching rather than by reading, so
the script prints them instead of asserting them:

- whether the **`port-config` worker acquires before or after `postinst`** — it compares
  what `postinst` wrote with what landed in `/usr/local/etc/service.d/`, which decides
  whether the wizard's port ever reaches the firewall entry on a fresh install;
- what `systemctl cat pkgctl-rescriptum` says about **`Type=`, `KillMode=` and `Restart=`**
  — the last one decides whether DSM restarts the process if it dies, which is worth
  documenting either way;
- whether **`etc/` and `var/` survive an upgrade** on their own;
- where DSM installed the **logrotate stanza**.

## When it fails

Three places say why, and knowing they exist is most of the debugging:

```console
$ cat /var/log/packages/rescriptum.log   # the package scripts' own output
$ cat /var/log/synopkg.log               # Package Center's view — why an archive was refused
$ systemctl status pkgctl-rescriptum
```
