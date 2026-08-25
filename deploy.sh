#!/usr/bin/env bash
# Build for the NAS, copy it there, and restart it.
#
#   ./deploy.sh admin@nas
#   ./deploy.sh admin@nas /volume1/netboot
#
# DSM 7 gives you no systemd, so the binary is started detached with nohup and stopped
# by name. Autostart across reboots is a Task Scheduler entry — see the README; this
# script only replaces a running instance.
#
# Environment:
#   TARGET   rust target triple      (default armv7-unknown-linux-musleabihf)
#   ANSWERS  answers directory       (default <remote-dir>/answers)
#   PORT     listen port             (default 8000)

set -euo pipefail
cd "$(dirname "$0")"

HOST="${1:-}"
REMOTE_DIR="${2:-/volume1/netboot}"
TARGET="${TARGET:-armv7-unknown-linux-musleabihf}"
PORT="${PORT:-8000}"
ANSWERS="${ANSWERS:-$REMOTE_DIR/answers}"

if [ -z "$HOST" ]; then
  sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
fi

BIN="target/$TARGET/release/rescriptum"

echo "==> building"
./build.sh "$TARGET"

echo "==> checking the answers before shipping the binary"
# Serving a broken answer set is worse than not deploying: better to find out here.
if [ -d examples ]; then
  RESCRIPTUM_ANSWERS_DIR=examples cargo run --quiet --release -- check || {
    echo "    the local answers do not check out — fix them before deploying" >&2
    exit 1
  }
fi

echo "==> copying to $HOST:$REMOTE_DIR"
# To a temporary name first: replacing a running binary in place is how you get a
# half-copied file executed.
scp -q "$BIN" "$HOST:$REMOTE_DIR/rescriptum.new"

echo "==> restarting"
ssh "$HOST" bash -s <<REMOTE
set -euo pipefail
cd "$REMOTE_DIR"
chmod +x rescriptum.new

if pgrep -f '[r]escriptum' >/dev/null 2>&1; then
  echo "    stopping the running instance"
  pkill -f '[r]escriptum' || true
  sleep 1
fi

mv rescriptum.new rescriptum
mkdir -p "$ANSWERS"

echo "    starting"
RESCRIPTUM_ANSWERS_DIR="$ANSWERS" RESCRIPTUM_LISTEN_ADDR="0.0.0.0:$PORT" \\
  nohup ./rescriptum >> "$REMOTE_DIR/rescriptum.log" 2>&1 &
sleep 1
pgrep -f '[r]escriptum' >/dev/null || { echo "    it did not stay up — see rescriptum.log" >&2; exit 1; }
REMOTE

echo "==> checking it answers"
HOSTNAME_ONLY="${HOST#*@}"
if curl -fsS --max-time 5 "http://$HOSTNAME_ONLY:$PORT/health" >/dev/null; then
  echo "    ok — http://$HOSTNAME_ONLY:$PORT/health"
else
  echo "    it is running but /health is unreachable — check the DSM firewall" >&2
  exit 1
fi
