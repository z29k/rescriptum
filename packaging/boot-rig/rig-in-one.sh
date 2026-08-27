#!/usr/bin/env bash
# Drive the whole boot chain inside one container, on a bridge with no uplink.
#
#   rig-in-one.sh [bios|uefi]
#
# Everything the four-service rig does, minus Docker's network — which on a Mac is what
# stops a QEMU guest being seen at all. See Dockerfile.rig for the measurement.

set -uo pipefail
FIRMWARE="${1:-bios}"
SERVER=10.99.0.2
mkdir -p /out

say() { echo "==> $*"; }

# ---------------------------------------------------------------------------
# A private bridge. No uplink, no route out: the rig's network is its whole world,
# which is a stronger guarantee than `internal: true` and needs nothing from Docker.
# ---------------------------------------------------------------------------
say "building the network"
ip link add br0 type bridge
ip link set br0 type bridge forward_delay 0
ip addr add "${SERVER}/24" dev br0
ip link set br0 up
ip tuntap add dev tap0 mode tap
ip link set tap0 master br0
ip link set tap0 up
ip -brief addr show br0

# ---------------------------------------------------------------------------
# The DHCP configuration comes from the server itself, so the rig tests the snippet too:
# if what we tell operators to paste is wrong, nothing here boots.
# ---------------------------------------------------------------------------
say "generating the DHCP configuration from the server's own snippet"
export RESCRIPTUM_PUBLIC_HOST="${SERVER}"
export RESCRIPTUM_MEDIA_DIR=/srv/media
export RESCRIPTUM_ANSWERS_DIR=/srv/answers
export RESCRIPTUM_BOOT_DIR=/srv/boot
export RESCRIPTUM_LISTEN_ADDR="${SERVER}:8000"
export RESCRIPTUM_MEDIA_ADDR="${SERVER}:8001"
export RESCRIPTUM_TFTP_ADDR="${SERVER}:69"
export RESCRIPTUM_BOOT_TIMEOUT_SECS=5
export RESCRIPTUM_LOG=all

# **The snippet's stderr is not discarded.** Hiding it once cost a run: the binary was
# the wrong architecture, the generator produced nothing, and the configuration below
# came out with no handoff in it at all — which looked like a DHCP server that simply
# did not answer.
rescriptum boot dhcp-snippet --format dnsmasq > /rig/snippet.conf || {
  echo "could not generate the DHCP snippet — see above" >&2
  exit 1
}
if ! grep -q "dhcp-boot" /rig/snippet.conf; then
  echo "the generated snippet names no boot file; refusing to run a rig that tests nothing" >&2
  exit 1
fi
{
  echo "port=0"
  echo "interface=br0"
  echo "bind-interfaces"
  echo "log-dhcp"
  echo "log-facility=-"
  echo "dhcp-range=10.99.0.100,10.99.0.200,1h"
  echo
  cat /rig/snippet.conf
} > /rig/dnsmasq.conf
cat /rig/dnsmasq.conf

say "what the server thinks of its own boot assets"
rescriptum boot check || exit 1

# ---------------------------------------------------------------------------
say "starting the server and the DHCP handoff"
rescriptum > /out/server.log 2>&1 &
SERVER_PID=$!
dnsmasq --keep-in-foreground --conf-file=/rig/dnsmasq.conf > /out/dhcp.log 2>&1 &
DHCP_PID=$!
# tcpdump is the answer to "did the request even arrive", which is the only question
# worth asking when no client boots.
tcpdump -i br0 -n -l "port 67 or port 68 or port 69" > /out/wire.log 2>&1 &
TCPDUMP_PID=$!

# Both have to still be alive: a process that died looks exactly like one still starting.
sleep 3
for pid_name in "SERVER_PID server" "DHCP_PID dhcp"; do
  set -- $pid_name
  if ! kill -0 "${!1}" 2>/dev/null; then
    echo "the $2 process died before a client was booted. Its log:" >&2
    tail -20 "/out/$2.log" >&2
    exit 1
  fi
done

boot() {
  local mac="$1" name="$2" limit="$3"
  say "booting a machine as $name ($mac, ${FIRMWARE})"
  cp /rig/local-disk.img "/tmp/${name}.img"

  local machine=pc
  local firmware=()
  if [ "${FIRMWARE}" = "uefi" ]; then
    machine=q35
    cp /usr/share/OVMF/OVMF_VARS.fd "/tmp/${name}.vars.fd"
    firmware=(
      -drive "if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE.fd"
      -drive "if=pflash,format=raw,file=/tmp/${name}.vars.fd"
    )
  fi

  # `-boot order=nc`: network first, then the disk. **That ordering is the fallthrough
  # being tested** — a machine that gets no answer must reach the disk, not stop.
  timeout "${limit}" qemu-system-x86_64 \
    -machine "${machine}" \
    -m 1024 \
    -nographic \
    -no-reboot \
    -boot order=nc \
    "${firmware[@]}" \
    -netdev tap,id=n0,ifname=tap0,script=no,downscript=no \
    -device e1000,netdev=n0,mac="${mac}" \
    -drive file="/tmp/${name}.img",format=raw,if=ide \
    > "/out/${name}.serial.log" 2>&1
  echo "--- ${name} ended after at most ${limit}s ---" >> "/out/${name}.serial.log"
}

boot 52:54:00:aa:aa:aa unclaimed "${UNCLAIMED_SECONDS:-240}"
boot 98:fa:9b:50:d8:10 claimed "${CLAIMED_SECONDS:-240}"

kill "${SERVER_PID}" "${DHCP_PID}" "${TCPDUMP_PID}" 2>/dev/null
wait 2>/dev/null

# ---------------------------------------------------------------------------
say "results"
fail=0
check() {
  local what="$1" file="$2" needle="$3"
  if grep -qF -- "${needle}" "${file}" 2>/dev/null; then
    echo "  ok   ${what}"
  else
    echo "  FAIL ${what} — ${needle} not in $(basename "${file}")"
    fail=$((fail + 1))
  fi
}

check "the DHCP handoff answered, from our own generated snippet" \
      /out/dhcp.log "DHCPACK"
check "a loader was fetched over TFTP" \
      /out/server.log "tftp:"
check "the unclaimed machine fell through to its local disk" \
      /out/unclaimed.serial.log "RESCRIPTUM-RIG-LOCAL-DISK-REACHED"
check "the claimed machine fetched its sentinel" \
      /out/server.log "/rig/claimed"

echo
if [ "${fail}" = "0" ]; then
  echo "rig: all markers reached"
  exit 0
fi
echo "rig: ${fail} marker(s) missing"
echo
echo "--- what was on the wire ---"
head -20 /out/wire.log
exit 1
