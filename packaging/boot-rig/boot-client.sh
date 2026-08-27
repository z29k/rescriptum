#!/usr/bin/env bash
# Boot one QEMU machine on the rig's network and record what its serial console said.
#
#   boot-client.sh <mac> <name> [seconds] [bios|uefi]
#
# **The NIC is bridged into the container's own network, not QEMU's user-mode stack.**
# That distinction is the whole point: user-mode networking carries its own DHCP server,
# so a rig built on it would test everything except the handoff it exists to test. The
# guest has to see the rig's dnsmasq, and dnsmasq has to see a real DHCP broadcast.
#
# The serial log is the entire assertion surface. Two markers, both deterministic and
# neither a screenshot:
#
#   * an **unclaimed** machine falls through the menu to its own disk, whose boot sector
#     prints RESCRIPTUM-RIG-LOCAL-DISK-REACHED and halts;
#   * a **claimed** machine runs its own unattended answer, which ends by fetching a
#     sentinel — so "the whole chain worked" is a line in the *server's* log rather than
#     something this script has to interpret.

set -uo pipefail

MAC="${1:?usage: boot-client.sh <mac> <name> [seconds] [bios|uefi]}"
NAME="${2:?usage: boot-client.sh <mac> <name> [seconds] [bios|uefi]}"
LIMIT="${3:-180}"
FIRMWARE="${4:-bios}"
OUT="/out/${NAME}.serial.log"
mkdir -p /out

# Put eth0 on a bridge and hang a tap off it, so the guest is a peer of the other
# containers rather than a NAT client of this one.
setup_bridge() {
  ip link add br0 type bridge 2>/dev/null || true
  ip link set br0 up
  ip addr flush dev eth0 || true
  ip link set eth0 master br0
  ip tuntap add dev tap0 mode tap 2>/dev/null || true
  ip link set tap0 master br0
  ip link set tap0 up
  # Bridges default to forwarding delay; the guest's first DHCP would land in it.
  ip link set br0 type bridge forward_delay 0 2>/dev/null || true
}

if ! setup_bridge; then
  echo "cannot bridge eth0 — the client container needs cap_add: NET_ADMIN" | tee "${OUT}"
  exit 1
fi

# A scratch copy, so a run cannot alter the image the next one boots.
cp /rig/local-disk.img "/tmp/${NAME}.img"

FIRMWARE_ARGS=()
if [ "${FIRMWARE}" = "uefi" ]; then
  cp /usr/share/OVMF/OVMF_VARS.fd "/tmp/${NAME}.vars.fd"
  FIRMWARE_ARGS=(
    -drive "if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE.fd"
    -drive "if=pflash,format=raw,file=/tmp/${NAME}.vars.fd"
  )
fi

# `-boot order=nc`: network first, then the disk. **That ordering is the fallthrough
# being tested** — a machine that gets no answer must reach the disk, not stop.
#
# `-nographic` puts the serial console on stdout. KVM is never requested: the rig has to
# pass under TCG, because the development machine is a Mac.
timeout "${LIMIT}" qemu-system-x86_64 \
  -machine q35 \
  -m 1024 \
  -nographic \
  -no-reboot \
  -boot order=nc \
  "${FIRMWARE_ARGS[@]}" \
  -netdev tap,id=n0,ifname=tap0,script=no,downscript=no \
  -device e1000,netdev=n0,mac="${MAC}" \
  -drive file="/tmp/${NAME}.img",format=raw,if=ide \
  > "${OUT}" 2>&1

status=$?
echo "--- ${NAME} (${MAC}, ${FIRMWARE}) exited ${status} after at most ${LIMIT}s ---" >> "${OUT}"
# `timeout` returning 124 is the normal end of a run that halted at the marker: a halted
# machine does not exit on its own, and waiting for one that never will is a failure this
# deliberately does not have. The markers decide, not the exit code.
exit 0
