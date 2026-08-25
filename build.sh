#!/usr/bin/env bash
# Build rescriptum and say how big it came out.
#
# With no arguments, builds for this machine. Give it targets to cross-compile, or
# `--all` for the ones a release ships.
#
#   ./build.sh
#   ./build.sh armv7-unknown-linux-musleabihf
#   ./build.sh --all
#   ./build.sh --no-sqlite armv7-unknown-linux-musleabihf
#
# Cross-compiling needs cargo-zigbuild and Zig:
#   cargo install cargo-zigbuild && brew install zig   (or see the README)

set -euo pipefail
cd "$(dirname "$0")"

RELEASE_TARGETS=(
  armv7-unknown-linux-musleabihf   # the Synology DS416j this was written for
  aarch64-unknown-linux-musl
  x86_64-unknown-linux-musl
  aarch64-apple-darwin
  x86_64-apple-darwin
)

FEATURES=()
TARGETS=()

for arg in "$@"; do
  case "$arg" in
    --all)       TARGETS+=("${RELEASE_TARGETS[@]}") ;;
    --no-sqlite) FEATURES=(--no-default-features) ;;
    -h|--help)   sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*)          echo "unknown option: $arg" >&2; exit 2 ;;
    *)           TARGETS+=("$arg") ;;
  esac
done

# macOS still ships bash 3.2, where expanding an empty array under `set -u` is an
# error. This is the portable way to say "these arguments, if there are any".
expand() {
  if [ ${#FEATURES[@]} -gt 0 ]; then printf '%s\n' "${FEATURES[@]}"; fi
}

size_of() {
  # stat's flags differ between BSD and GNU, and this script runs on both.
  stat -f%z "$1" 2>/dev/null || stat -c%s "$1"
}

human() {
  # LC_ALL so a French locale does not print "1,98 MB" where a script expects a dot.
  LC_ALL=C awk -v b="$1" 'BEGIN { printf (b > 1048576) ? "%.2f MB" : "%.0f KB", (b > 1048576) ? b/1048576 : b/1024 }'
}

build_one() {
  local target="${1:-}"
  local bin

  if [ -z "$target" ]; then
    echo "==> building for this machine"
    # shellcheck disable=SC2046
    cargo build --release $(expand)
    bin="target/release/rescriptum"
  else
    echo "==> building for $target"
    if ! rustup target list --installed | grep -qx "$target"; then
      echo "    adding the target"
      rustup target add "$target"
    fi
    # Zig links these, which is what keeps musl static builds painless.
    # shellcheck disable=SC2046
    cargo zigbuild --release --target "$target" $(expand)
    bin="target/$target/release/rescriptum"
  fi

  local bytes
  bytes=$(size_of "$bin")
  printf '    %-38s %10s   %s\n' "${target:-native}" "$(human "$bytes")" "$bin"

  # A DSM 7 box has a glibc far older than anything we build against, so a dynamic
  # binary would fail at exec time rather than at build time. Say so here instead.
  if command -v file >/dev/null; then
    local kind
    kind=$(file -b "$bin")
    case "$target" in
      *-linux-musl*)
        if ! grep -q 'statically linked' <<<"$kind"; then
          echo "    WARNING: not statically linked — DSM will refuse to run this" >&2
          echo "    $kind" >&2
        fi
        ;;
    esac
  fi
}

if [ ${#TARGETS[@]} -eq 0 ]; then
  build_one ""
else
  for t in "${TARGETS[@]}"; do build_one "$t"; done
fi
