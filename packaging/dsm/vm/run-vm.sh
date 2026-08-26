#!/usr/bin/env bash
# Boot a DSM 7 virtual machine to test the package against.
#
#   packaging/dsm/vm/run-vm.sh --loader ~/dsm/loader.img
#   packaging/dsm/vm/run-vm.sh --loader ~/dsm/loader.img --snapshot   # discard all writes
#
# **This is the fallback.** The short route is docker-compose.yml beside this file, which
# runs Synology's own Virtual DSM release and needs no image at all — but it needs a Linux
# host with KVM. Use this one when that is not available, or when the machine has to be
# something other than Virtual DSM: it takes whatever loader image you supply and gives it
# the hardware, the disks and the port forwards that matter. See README.md for what the rig
# is evidence about, and what it is not.
#
# What this sets up:
#
#   * hardware acceleration where the host has it (KVM on Linux, HVF on an Intel Mac);
#     on Apple silicon an x86_64 guest is fully emulated and slow, which is a reason to run
#     the rig on a Linux box rather than a reason to skip it;
#   * the loader as a USB device with the boot index, which is how these images expect to
#     be booted, and a SATA data disk created on first run;
#   * user-mode networking with forwards, so no bridge and no root: DSM's web UI on
#     localhost:5000/5001, ssh on 2222, and the answer port on 8000;
#   * --snapshot, so a package that breaks the machine costs one Ctrl-C. Take a real
#     qcow2 snapshot once DSM is installed and configured:
#         qemu-img snapshot -c clean dsm-data.qcow2
#         qemu-img snapshot -a clean dsm-data.qcow2

set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)

LOADER=""
DISK="$HERE/dsm-data.qcow2"
SIZE=32G
MEM=2048
CPUS=2
SSH_PORT=2222
WEB_PORT=5000
WEBS_PORT=5001
APP_PORT=8000
NIC=e1000e
SNAPSHOT=""

while [ $# -gt 0 ]; do
    case "$1" in
    --loader) LOADER="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --size) SIZE="$2"; shift 2 ;;
    --mem) MEM="$2"; shift 2 ;;
    --cpus) CPUS="$2"; shift 2 ;;
    --ssh) SSH_PORT="$2"; shift 2 ;;
    --port) APP_PORT="$2"; shift 2 ;;
    --nic) NIC="$2"; shift 2 ;;
    --snapshot) SNAPSHOT="-snapshot"; shift ;;
    -h | --help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

[ -n "$LOADER" ] || { echo "--loader is required (README.md says where an image comes from)" >&2; exit 2; }
[ -f "$LOADER" ] || { echo "no loader image at $LOADER" >&2; exit 1; }
command -v qemu-system-x86_64 >/dev/null || { echo "qemu-system-x86_64 is not installed" >&2; exit 1; }

if [ ! -f "$DISK" ]; then
    echo "==> creating a $SIZE data disk at $DISK"
    qemu-img create -f qcow2 "$DISK" "$SIZE" >/dev/null
fi

# Accelerate where we can, emulate where we cannot, and say which — a rig that is silently
# running under TCG looks like a rig that is broken.
ACCEL=tcg
CPU=qemu64
case "$(uname -s)/$(uname -m)" in
Linux/x86_64) if [ -w /dev/kvm ]; then ACCEL=kvm; CPU=host; fi ;;
Darwin/x86_64) ACCEL=hvf; CPU=host ;;
esac
if [ "$ACCEL" = tcg ]; then
    echo "==> no hardware acceleration here: the guest is emulated, and it will be slow"
fi

echo "==> DSM will be at http://localhost:$WEB_PORT (https on $WEBS_PORT), ssh on $SSH_PORT"
echo "==> rescriptum's port is forwarded from localhost:$APP_PORT"
echo "==> then: packaging/dsm/vm/on-dsm.sh admin@localhost -p $SSH_PORT"
if [ -n "$SNAPSHOT" ]; then
    echo "==> --snapshot: every write to $DISK is discarded when this exits"
fi

exec qemu-system-x86_64 \
    -machine q35,accel="$ACCEL" \
    -cpu "$CPU" \
    -smp "$CPUS" \
    -m "$MEM" \
    $SNAPSHOT \
    -device qemu-xhci,id=xhci \
    -drive file="$LOADER",format=raw,if=none,id=loader \
    -device usb-storage,bus=xhci.0,drive=loader,bootindex=1 \
    -device ahci,id=ahci \
    -drive file="$DISK",format=qcow2,if=none,id=data \
    -device ide-hd,bus=ahci.0,drive=data \
    -netdev user,id=net0,hostfwd=tcp::"$SSH_PORT"-:22,hostfwd=tcp::"$WEB_PORT"-:5000,hostfwd=tcp::"$WEBS_PORT"-:5001,hostfwd=tcp::"$APP_PORT"-:"$APP_PORT" \
    -device "$NIC",netdev=net0 \
    -display none -serial mon:stdio
