#!/usr/bin/env bash
# Freeze and restore the rig's DSM machine — entirely inside Docker.
#
#   packaging/dsm/vm/snapshot.sh save            # after the wizard, once
#   packaging/dsm/vm/snapshot.sh restore         # back to that state, any time
#   packaging/dsm/vm/snapshot.sh list
#   packaging/dsm/vm/snapshot.sh save clean-7.2  # a name, if you want several
#
# DSM's setup wizard cannot be scripted — the image offers no admin-account variable — so
# the machine is worth exactly one manual setup. Snapshot it immediately afterwards and the
# rig becomes repeatable: `remote-check.sh` is destructive on purpose, and getting back to a
# known-good DSM has to cost one command rather than an afternoon.
#
# The copy is **volume to volume**, not a tarball on the host, and that is deliberate: on a
# Mac, Docker's disk image is already allocated, so a copy inside it costs no host disk at
# all — which is the same reason the machine's storage is a named volume in the first place.
# `--file` writes a tarball instead, when you want it somewhere you can see.
#
# **It copies with GNU cp and --sparse=always, and that is not a detail.** DSM's storage is
# a handful of raw disk images — a 16 GiB data.img holding 4 GiB — and busybox's cp writes
# every hole out as real zeroes. Done that way a 3.9 GB machine snapshots to 26 GB and the
# restore fills Docker's disk before it finishes, leaving a broken machine and no snapshot
# worth the name. Measured, the hard way.

set -euo pipefail

VOLUME="${DSM_VOLUME:-}"
if [ -z "$VOLUME" ]; then
    VOLUME=$(docker volume ls --format '{{.Name}}' | grep -m 1 'dsm-storage$' || true)
fi
[ -n "$VOLUME" ] || { echo "no DSM storage volume found — start the machine first, or set DSM_VOLUME" >&2; exit 1; }

CONTAINER="${DSM_CONTAINER:-rescriptum-dsm}"
ACTION="${1:-}"
NAME="${2:-clean}"
SNAP="dsm-snapshot-$NAME"
FILE=""
case "${3:-}" in --file) FILE="$NAME.tar" ;; esac

# A snapshot taken while DSM is writing is a snapshot of a half-written filesystem.
stopped() { [ -z "$(docker ps -q --filter "name=^${CONTAINER}$")" ]; }
require_stopped() {
    if ! stopped; then
        echo "==> stopping $CONTAINER first (a running machine is still writing)"
        docker stop "$CONTAINER" >/dev/null
    fi
}

case "$ACTION" in
save)
    require_stopped
    if [ -n "$FILE" ]; then
        docker run --rm -v "$VOLUME":/from -v "$PWD":/to debian:stable-slim \
            tar -cSf "/to/$FILE" -C /from .
        echo "==> $PWD/$FILE"
    else
        docker volume rm "$SNAP" >/dev/null 2>&1 || true
        docker volume create "$SNAP" >/dev/null
        docker run --rm -v "$VOLUME":/from -v "$SNAP":/to debian:stable-slim \
            cp -a --sparse=always /from/. /to/
        echo "==> saved $VOLUME to the volume $SNAP"
    fi
    ;;
restore)
    require_stopped
    docker volume inspect "$SNAP" >/dev/null 2>&1 || { echo "no snapshot named $NAME (try: $0 list)" >&2; exit 1; }
    docker run --rm -v "$VOLUME":/to -v "$SNAP":/from debian:stable-slim \
        sh -c 'rm -rf /to/..?* /to/.[!.]* /to/* 2>/dev/null; cp -a --sparse=always /from/. /to/'
    echo "==> $VOLUME restored from $SNAP — start the machine again"
    ;;
list)
    docker volume ls --format '{{.Name}}' | grep '^dsm-snapshot-' | sed 's/^dsm-snapshot-/  /' || echo "  (none)"
    ;;
*)
    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
