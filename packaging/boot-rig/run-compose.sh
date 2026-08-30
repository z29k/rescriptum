#!/usr/bin/env bash
# Drive the boot rig and assert on its two markers.
#
#   ./run.sh                 # both clients, BIOS
#   ./run.sh --uefi          # both clients, OVMF
#   ./run.sh --keep          # leave the stack up afterwards, to poke at it
#
# **This is the contract, and CI runs a subset of it.** GitHub's runners are somebody
# else's machines with somebody else's limits, so the dev rig is what decides and CI is
# the tripwire — sized so it cannot fail for capacity reasons.

set -euo pipefail
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
cd "$(dirname "$SELF")"

FIRMWARE=bios
KEEP=0
while [ $# -gt 0 ]; do
  case "$1" in
    --uefi) FIRMWARE=uefi; shift ;;
    --keep) KEEP=1; shift ;;
    -h|--help) sed -n '2,9p' "$SELF" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unexpected argument: $1" >&2; exit 2 ;;
  esac
done

COMPOSE=(docker compose -f docker-compose.yml)

cleanup() {
  if [ "$KEEP" = "0" ]; then
    "${COMPOSE[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  else
    echo "stack left up; 'docker compose -f $PWD/docker-compose.yml down -v' when done"
  fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# The DHCP configuration comes from the server itself, which is what makes the rig a
# test of `boot dhcp-snippet` too. If what we tell operators to paste is wrong, nothing
# here boots — and that is exactly the failure worth catching before they meet it.
# ---------------------------------------------------------------------------
echo "==> generating the DHCP configuration from the server's own snippet"
mkdir -p generated
cargo run --quiet --manifest-path ../../Cargo.toml -- boot dhcp-snippet --format dnsmasq \
  > generated/dnsmasq.conf.snippet 2>/dev/null || {
    echo "could not generate the snippet" >&2; exit 1; }

# Everything above the snippet is the rig's own scaffolding: a range to hand out, an
# interface to listen on, and no upstream DNS — this network has no internet.
{
  echo "# --- rig scaffolding (not generated) ---"
  echo "port=0"
  echo "interface=eth0"
  echo "bind-interfaces"
  echo "log-dhcp"
  echo "dhcp-range=10.99.0.100,10.99.0.200,1h"
  # No `enable-tftp` at all: this dnsmasq only answers DHCP, and the server under
  # test is the one that hands out loaders. (`enable-tftp=no` does not disable it — it
  # names an interface called "no", which dnsmasq then enables TFTP on.)
  echo
  echo '# --- everything below is: rescriptum boot dhcp-snippet --format dnsmasq ---'
  # The generated snippet names RESCRIPTUM_PUBLIC_HOST, which outside a container is
  # this machine. Inside the rig the server is 10.99.0.2, and that substitution is the
  # only edit the rig makes.
  sed 's/[0-9]\{1,3\}\.[0-9]\{1,3\}\.[0-9]\{1,3\}\.[0-9]\{1,3\}/10.99.0.2/g' \
    generated/dnsmasq.conf.snippet
} > generated/dnsmasq.conf

echo "==> building the images (no KVM anywhere: the rig must pass under TCG)"
# **The client is built explicitly.** `docker compose run` reuses whatever image is
# already there, so a client image that is never built is one that never changes — and
# a fix made to its Dockerfile would silently not apply.
"${COMPOSE[@]}" build loaders server dhcp client

echo "==> bringing the stack up"
"${COMPOSE[@]}" up -d loaders server dhcp

# The loaders service exits when it has copied; the others have to be listening.
for _ in $(seq 1 60); do
  if "${COMPOSE[@]}" exec -T server /usr/local/bin/rescriptum boot check >/dev/null 2>&1; then
    break
  fi
  sleep 2
done

# **Every service has to still be running.** A container that died looks exactly like
# one still starting, and the first run of this rig spent four minutes booting a client
# at a DHCP server that had exited 127 a minute earlier.
for service in server dhcp; do
  state=$("${COMPOSE[@]}" ps --format '{{.State}}' "$service" 2>/dev/null | head -1)
  if [ "$state" != "running" ]; then
    echo "the $service container is '$state', not running. Its log:" >&2
    "${COMPOSE[@]}" logs "$service" 2>&1 | tail -20 >&2
    exit 1
  fi
done

echo "==> what the server thinks of its own boot assets"
"${COMPOSE[@]}" exec -T server /usr/local/bin/rescriptum boot check

# ---------------------------------------------------------------------------
# Marker one: a machine nothing claims must reach its own disk.
# ---------------------------------------------------------------------------
echo "==> booting an UNCLAIMED machine (${FIRMWARE})"
"${COMPOSE[@]}" run --rm -T client 52:54:00:aa:aa:aa unclaimed 240 "${FIRMWARE}" || true

# ---------------------------------------------------------------------------
# Marker two: a machine something claims must reach its own answer.
# ---------------------------------------------------------------------------
echo "==> booting a CLAIMED machine (${FIRMWARE})"
"${COMPOSE[@]}" run --rm -T client 98:fa:9b:50:d8:10 claimed 240 "${FIRMWARE}" || true

echo "==> results"
mkdir -p results
"${COMPOSE[@]}" run --rm -T --entrypoint sh client -c 'cat /out/*.serial.log' > results/serial.log 2>&1 || true
"${COMPOSE[@]}" logs server > results/server.log 2>&1 || true
"${COMPOSE[@]}" logs dhcp > results/dhcp.log 2>&1 || true

fail=0
check() {
  local what="$1" file="$2" needle="$3"
  if grep -qF -- "${needle}" "${file}" 2>/dev/null; then
    echo "  ok   ${what}"
  else
    echo "  FAIL ${what} — ${needle} not in ${file}"
    fail=$((fail + 1))
  fi
}

# An unclaimed machine reached its own disk rather than sitting at a menu or stopping.
check "unclaimed machine fell through to its local disk" \
      results/serial.log "RESCRIPTUM-RIG-LOCAL-DISK-REACHED"
# A claimed machine reached its own answer — asserted in the *server's* log, which
# proves the request arrived rather than that the client printed something.
check "claimed machine fetched its sentinel" \
      results/server.log "/rig/claimed"
# And the DHCP handoff itself worked, which is the snippet under test.
check "dnsmasq answered a PXE client from the generated snippet" \
      results/dhcp.log "DHCPACK"

echo
if [ "${fail}" = "0" ]; then
  echo "rig: all markers reached"
else
  echo "rig: ${fail} marker(s) missing — see packaging/boot-rig/results/"
  exit 1
fi
