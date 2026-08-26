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
#   ./build.sh --spk armv7-unknown-linux-musleabihf   # and wrap it as a DSM package
#
# Cross-compiling needs cargo-zigbuild and Zig:
#   cargo install cargo-zigbuild && brew install zig   (or see the README)

set -euo pipefail
cd "$(dirname "$0")"

# armv7 is the one target that is not musl, and the reason is not a preference. Synology's
# ARMv7 kernels are 3.10, and they answer the *time64* syscalls with EINVAL rather than
# ENOSYS — musl 1.2 only falls back to the 32-bit ones on ENOSYS, so every clock_gettime,
# clock_nanosleep and timed futex fails on the machine this project exists for. Measured on
# a DS416j: the musl build runs `--version` and then panics the moment it wants the time.
# glibc on 32-bit uses the time32 syscalls, and DSM ships its own; targeting the oldest
# floor that covers it keeps the binary running on newer ARMv7 Linux too, since glibc is
# backward compatible.
RELEASE_TARGETS=(
  armv7-unknown-linux-gnueabihf    # the Synology DS416j this was written for — see above
  aarch64-unknown-linux-musl
  x86_64-unknown-linux-musl
  aarch64-apple-darwin
  x86_64-apple-darwin
)

# What cargo-zigbuild is told, when it differs from the Rust target: the glibc floor.
zig_target() {
  case "$1" in
    armv7-unknown-linux-gnueabihf) echo "$1.2.17" ;;
    *)                             echo "$1" ;;
  esac
}

FEATURES=()
TARGETS=()
SPK=no

for arg in "$@"; do
  case "$arg" in
    --all)       TARGETS+=("${RELEASE_TARGETS[@]}") ;;
    --no-sqlite) FEATURES=(--no-default-features) ;;
    --spk)       SPK=yes ;;
    -h|--help)   sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
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
    # Zig links these, which is what keeps cross builds painless — and, for armv7, what
    # lets us name the glibc version DSM actually has instead of the host's.
    # shellcheck disable=SC2046
    cargo zigbuild --release --target "$(zig_target "$target")" $(expand)
    bin="target/$target/release/rescriptum"
  fi

  local bytes
  bytes=$(size_of "$bin")
  printf '    %-38s %10s   %s\n' "${target:-native}" "$(human "$bytes")" "$bin"

  # Two different promises, so two different checks. A musl target must come out static —
  # that is the whole point of it. The armv7 glibc target must not require a glibc newer
  # than the floor we aimed at, or it fails at exec time on the NAS rather than here.
  if command -v file >/dev/null; then
    local kind
    kind=$(file -b "$bin")
    case "$target" in
      *-linux-musl*)
        if ! grep -q 'statically linked' <<<"$kind"; then
          echo "    WARNING: not statically linked — that target is meant to be" >&2
          echo "    $kind" >&2
        fi
        ;;
      armv7-unknown-linux-gnueabihf)
        if command -v readelf >/dev/null; then
          local want
          want=$(readelf --dyn-syms "$bin" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)
          echo "    needs at most ${want:-GLIBC_?} — DSM 7 on armada38x has 2.20"
          if [ -n "$want" ] && [ "$(printf '%s\n' "GLIBC_2.17" "$want" | sort -V | tail -1)" != "GLIBC_2.17" ]; then
            echo "    WARNING: that is newer than the 2.17 floor this target aims at" >&2
          fi
        fi
        ;;
    esac
  fi
}

# A `.spk` is a release format, not a build: the binary is already finished, and this only
# wraps it for one platform's package manager. Which is why it is a flag here rather than a
# second build system — see packaging/dsm/.
spk_abi() {
  case "$1" in
    armv7-unknown-linux-musleabihf) echo armv7 ;;
    aarch64-unknown-linux-musl)     echo aarch64 ;;
    x86_64-unknown-linux-musl)      echo x86_64 ;;
    *)                              return 1 ;;
  esac
}

package_one() {
  local target="$1" abi
  if ! abi=$(spk_abi "$target"); then
    echo "    no DSM package for ${target:-this machine} — a .spk carries a Linux musl build" >&2
    return 0
  fi
  echo "==> packaging $abi for DSM"
  packaging/dsm/make-spk.sh "$abi"
}

if [ ${#TARGETS[@]} -eq 0 ]; then
  build_one ""
  # An `&&` here would be the script's last command, and `set -e` would make its
  # "SPK=no" false the exit status of a successful build. deploy.sh keys on that.
  if [ "$SPK" = yes ]; then package_one ""; fi
else
  for t in "${TARGETS[@]}"; do
    build_one "$t"
    if [ "$SPK" = yes ]; then package_one "$t"; fi
  done
fi
