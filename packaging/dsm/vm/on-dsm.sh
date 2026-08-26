#!/usr/bin/env bash
# Run the on-machine checks against a DSM 7 box over SSH — the VM while iterating, the
# DS416j for the verdict. Same script both times; that is the point.
#
#   packaging/dsm/vm/on-dsm.sh admin@localhost -p 2222      # the VM from run-vm.sh
#   packaging/dsm/vm/on-dsm.sh admin@nas                    # the real thing
#
# It needs two .spk files for the same version — build 1 and build 2 — because the most
# valuable test here is the *upgrade*, and an upgrade needs something to upgrade to. It
# builds them if they are not in dist/ already, from a binary you must have cross-compiled
# for that machine's ABI:
#
#   ./build.sh x86_64-unknown-linux-musl        # the VM
#   ./build.sh armv7-unknown-linux-gnueabihf   # the DS416j
#
# **Supervise the first run.** Every remote step echoes the command it runs, because DSM's
# own CLI differs between builds and the failure that matters is the one you can read.
# It is destructive on purpose — it upgrades over a hand-edited configuration and then
# uninstalls — so point it at a machine whose answers directory nobody cares about until
# it has passed once.

set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)

HOST=""
ABI=""
PORT=""
KEY=""
while [ $# -gt 0 ]; do
    case "$1" in
    -p) PORT="$2"; shift 2 ;;
    -i) KEY="$2"; shift 2 ;;
    --abi) ABI="$2"; shift 2 ;;
    -h | --help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) HOST="$1"; shift ;;
    esac
done
[ -n "$HOST" ] || { echo "usage: on-dsm.sh [-p port] [-i key] [--abi armv7|x86_64] user@host" >&2; exit 2; }

# Options for ssh; there is no scp here on purpose — see push() below.
SSH_OPTS=()
if [ -n "$PORT" ]; then SSH_OPTS+=(-p "$PORT"); fi
if [ -n "$KEY" ]; then SSH_OPTS+=(-i "$KEY"); fi

# A disposable VM gets a new host key every time it is rebuilt, and ssh refuses to talk to
# it — correctly, since the default here has to be the one that suits the *real* NAS. So
# the rig passes its own options rather than this script relaxing anything by itself:
#
#   RIG_SSH_OPTS="-o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$PWD/packaging/dsm/vm/known_hosts"
#
# which keeps the VM's keys out of your own known_hosts as well.
if [ -n "${RIG_SSH_OPTS:-}" ]; then
    # shellcheck disable=SC2206  # word splitting is what turns the string into options
    extra=($RIG_SSH_OPTS)
    SSH_OPTS+=("${extra[@]}")
fi

sshx() { ssh "${SSH_OPTS[@]}" "$HOST" "$@"; }

# Not scp: **DSM does not enable the SFTP subsystem by default**, and modern scp speaks
# SFTP, so it fails with "subsystem request failed on channel 0". `scp -O` falls back to
# the legacy protocol and does work, but it needs OpenSSH 8.6+ on this side; piping through
# ssh needs nothing on either. Found by trying it against a real DSM.
push() { # <local file> <remote path>
    sshx "cat > '$2'" <"$1"
}

# Ask the machine what it is rather than assuming: the whole point of the rig is that the
# VM is x86_64 and the machine this project exists for is not.
if [ -z "$ABI" ]; then
    case "$(sshx uname -m 2>/dev/null)" in
    x86_64) ABI=x86_64 ;;
    armv7l | armv7*) ABI=armv7 ;;
    aarch64) ABI=aarch64 ;;
    *) echo "could not tell what $HOST is — pass --abi" >&2; exit 1 ;;
    esac
fi
VERSION=$(grep -m1 '^version = ' "$REPO/Cargo.toml" | cut -d'"' -f2)
echo "==> $HOST is $ABI, testing rescriptum $VERSION"

# Always rebuild, never reuse what is lying in dist/. Packaging takes under a second, and
# a stale .spk is not a theoretical worry: one built earlier from a *macOS* binary installed
# perfectly here and then died with "cannot execute binary file", which cost a full run to
# diagnose.
for build in 1 2; do
    echo "==> building rescriptum-$VERSION-$build-$ABI.spk"
    "$REPO/packaging/dsm/make-spk.sh" "$ABI" --spk-build "$build" >/dev/null
done

echo "==> copying the rig"
sshx "rm -rf /tmp/rescriptum-rig && mkdir -p /tmp/rescriptum-rig"
push "$REPO/dist/rescriptum-$VERSION-1-$ABI.spk" /tmp/rescriptum-rig/build1.spk
push "$REPO/dist/rescriptum-$VERSION-2-$ABI.spk" /tmp/rescriptum-rig/build2.spk
push "$HERE/remote-check.sh" /tmp/rescriptum-rig/remote-check.sh

echo "==> running it as root"
REMOTE="sh /tmp/rescriptum-rig/remote-check.sh"
if [ "$(sshx id -u 2>/dev/null)" = 0 ]; then
    sshx "$REMOTE"
elif [ -t 1 ]; then
    # DSM's sudo prompts on the tty; -t gives it one. A password on a command line would
    # be readable through ps on the machine, which is the thing this project refuses to do
    # anywhere else.
    sshx -t "sudo $REMOTE"
else
    # No terminal — a CI runner. The rig's account needs passwordless sudo, or be root.
    sshx "sudo -n $REMOTE"
fi
