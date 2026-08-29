#!/usr/bin/env bash
# The boot rig: everything from a DHCP offer to a machine on its own disk, in one
# command, on a network that exists nowhere else.
#
#   ./run.sh                 # BIOS
#   ./run.sh --uefi          # OVMF
#   ./run.sh --rebuild       # force the image to be rebuilt first
#
# One container, holding the loaders, the server, dnsmasq and a QEMU machine on a
# **private bridge with no uplink**. Nothing crosses Docker's network, which means
# nothing can be filtered by it — a stronger guarantee than the `internal: true` the
# four-service variant asks for, and the reason this works on a Mac.
#
# `run-compose.sh` is that four-service variant: the same markers, split across
# containers on an isolated Docker network. It is the honest shape and it works on a
# Linux host; on Docker Desktop the QEMU guest's frames never reach the next container.
# See Dockerfile.rig for the measurement.
#
# **No /dev/kvm.** KVM would make this fast; the rig has to pass without it, because the
# development machine is a Mac.

set -euo pipefail
# Resolved before the `cd`: `$0` is relative when the script is invoked that way, and
# `--help` below reads the script itself.
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
cd "$(dirname "$SELF")"

FIRMWARE=bios
REBUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --uefi) FIRMWARE=uefi; shift ;;
    --bios) FIRMWARE=bios; shift ;;
    --rebuild) REBUILD=1; shift ;;
    -h|--help) sed -n '2,20p' "$SELF" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unexpected argument: $1" >&2; exit 2 ;;
  esac
done

IMAGE=rescriptum-rig:one
if [ "$REBUILD" = "1" ] || ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "==> building $IMAGE (iPXE and the server; the first one takes a while)"
  docker build -f Dockerfile.rig -t "$IMAGE" ../..
fi

# NET_ADMIN to build the bridge, /dev/net/tun for the tap the guest sits on. Nothing
# else: no host network, no published port, no KVM.
exec docker run --rm \
  --cap-add NET_ADMIN \
  --device /dev/net/tun \
  -e "UNCLAIMED_SECONDS=${UNCLAIMED_SECONDS:-240}" \
  -e "CLAIMED_SECONDS=${CLAIMED_SECONDS:-240}" \
  "$IMAGE" "$FIRMWARE"
