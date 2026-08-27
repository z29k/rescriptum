#!/usr/bin/env bash
# Build the branded iPXE loaders rescriptum ships.
#
#   ./build.sh                 # every loader the table names, into ./out
#   ./build.sh --out DIR
#
# Needs a C toolchain, GNU make, perl, and — for the EFI targets — the cross binutils
# for that architecture. On Debian:
#
#   apt install build-essential liblzma-dev mtools gcc-aarch64-linux-gnu
#
# ## Why we build iPXE at all
#
# Three independent reasons converged on it, and any one would have been enough:
#
#   1. **The entry point.** A stock loader either re-loads itself forever (the
#      documented chainloading loop) or chains to the public boot.netboot.xyz. Only an
#      embedded script gets a machine talking to *this* server, and only a build we
#      control can carry one.
#   2. **The name on the first line**, before anything else is on screen, and a
#      framebuffer console that a stock binary may not have compiled in at all.
#   3. **The feature set.** We choose what is compiled in — PNG, the menu commands,
#      sanboot, the console — rather than discovering at a customer site that a variant
#      lacks one.
#
# ## The GPL obligation, met by construction
#
# iPXE is GPLv2 (with the UBDL exception) and rescriptum is MIT. What makes that a
# non-conversation is that the loaders are **separate files, never linked into our
# binary**: mere aggregation, obvious and auditable. This script, `branding.h`,
# `embed.ipxe` and `PINNED` are the written offer — everything needed to reproduce what
# we ship, in the same repository as the thing that serves it.

set -euo pipefail
# Resolved before the `cd`, so `--help` can read the script itself however it was called.
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
cd "$(dirname "$SELF")"

OUT="$PWD/out"
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    -h|--help) sed -n '2,12p' "$SELF" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unexpected argument: $1" >&2; exit 2 ;;
  esac
done

# shellcheck disable=SC1091
. ./PINNED

WORK="${WORK:-$PWD/.work}"
mkdir -p "$OUT" "$WORK"

if [ ! -d "$WORK/ipxe/.git" ]; then
  echo "cloning iPXE into $WORK/ipxe"
  git clone --quiet "$IPXE_REPO" "$WORK/ipxe"
fi

# Pinned by SHA. A fetch first, because a shallow or stale clone may not have it yet.
git -C "$WORK/ipxe" fetch --quiet --tags origin
git -C "$WORK/ipxe" checkout --quiet "$IPXE_COMMIT"
echo "iPXE at $IPXE_COMMIT (${IPXE_TAG:-no tag})"

cp branding.h "$WORK/ipxe/src/config/local/branding.h"

# What has to be compiled in, and why each one is here rather than a default:
#   IMAGE_PNG        the logo behind the menu
#   CONSOLE_FRAMEBUFFER  the console that can show it
#   IMAGE_TRUST_CMD  so a site that wants signed images can have them
#   PARAM_CMD/NSLOOKUP_CMD/PING_CMD/REBOOT_CMD/POWEROFF_CMD  the diagnostics menu
#   VLAN_CMD         a boot VLAN is the recommendation that actually works
cat > "$WORK/ipxe/src/config/local/general.h" <<'CONFIG'
/* rescriptum: what the menu and the diagnostics entries need. */
#define IMAGE_PNG
#define CONSOLE_FRAMEBUFFER
#define IMAGE_TRUST_CMD
#define PARAM_CMD
#define NSLOOKUP_CMD
#define PING_CMD
#define REBOOT_CMD
#define POWEROFF_CMD
#define VLAN_CMD
#define NTP_CMD
#define CONSOLE_CMD
CONFIG

build() {
  local target="$1" output="$2"
  echo "building $target"
  make -C "$WORK/ipxe/src" -j"$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)" \
    "bin/$target" EMBED="$PWD/embed.ipxe" >/dev/null 2>&1 ||
    make -C "$WORK/ipxe/src" "bin/$target" EMBED="$PWD/embed.ipxe"
  cp "$WORK/ipxe/src/bin/$target" "$OUT/$output"
}

# ARM64 needs its own toolchain, and the failure without one is a wall of
# `unrecognized command-line option '-mlittle-endian'` from the *host* gcc — which
# reads like a broken Makefile rather than a missing cross-compiler. Naming it here is
# what turns that into "install gcc-aarch64-linux-gnu".
cross_for() {
  case "$1" in
    arm64) echo "aarch64-linux-gnu-" ;;
    *)     echo "" ;;
  esac
}

build_efi() {
  local arch="$1" target="$2" output="$3"
  local cross; cross="$(cross_for "$arch")"
  if [ -n "$cross" ] && ! command -v "${cross}gcc" >/dev/null 2>&1; then
    echo "skipping $arch/$target: ${cross}gcc is not installed" >&2
    return 0
  fi
  echo "building $arch/$target"
  make -C "$WORK/ipxe/src" -j"$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)" \
    ARCH="$arch" CROSS_COMPILE="$cross" "bin-$arch-efi/$target" \
    EMBED="$PWD/embed.ipxe" >/dev/null 2>&1 ||
    make -C "$WORK/ipxe/src" ARCH="$arch" CROSS_COMPILE="$cross" \
      "bin-$arch-efi/$target" EMBED="$PWD/embed.ipxe"
  cp "$WORK/ipxe/src/bin-$arch-efi/$target" "$OUT/$output"
}

# The names here are the ones `src/boot/loaders.rs` hands out and
# `boot dhcp-snippet` writes into somebody's DHCP server. **They must not drift**:
# `boot check` compares this directory against that table, and a snippet naming a file
# that is not here fails silently at the ROM.
build undionly.kpxe ipxe-undionly.kpxe
build ipxe.pxe ipxe.kpxe

build_efi x86_64 ipxe.efi ipxe-x86_64.efi
build_efi x86_64 snp.efi ipxe-x86_64-snp.efi
build_efi x86_64 snponly.efi ipxe-x86_64-snponly.efi

build_efi arm64 ipxe.efi ipxe-arm64.efi
build_efi arm64 snp.efi ipxe-arm64-snp.efi
build_efi arm64 snponly.efi ipxe-arm64-snponly.efi

# The media a machine with no usable PXE ROM can still boot from: an ISO for IPMI virtual
# media, and a USB image for a stick. **These live in the BIOS build directory**, not the
# EFI one — `bin/ipxe.iso`, not `bin-x86_64-efi/ipxe.iso` — which is the mistake the first
# version of this script made, and it failed into the `||` below rather than saying so.
# They are Phase 5 of the plan and nothing depends on them yet, so a failure here is a
# note rather than an error.
build ipxe.iso ipxe.iso || echo "note: the ISO target needs xorriso or mkisofs"
build ipxe.usb ipxe.usb || echo "note: the USB target needs mtools"

( cd "$OUT" && sha256sum ./* > SHA256SUMS 2>/dev/null || shasum -a 256 ./* > SHA256SUMS )

cat > "$OUT/NOTICE" <<NOTICE
These loaders are iPXE, built from $IPXE_REPO at $IPXE_COMMIT (${IPXE_TAG:-no tag}),
with rescriptum's branding.h and embed.ipxe applied and nothing else patched.

iPXE is free software under the GNU General Public License version 2 (with the UBDL
exception). rescriptum itself is MIT and does not link against it: these are separate
files served alongside, which is mere aggregation.

The complete corresponding source is the commit above plus packaging/ipxe/ in
https://github.com/z29k/rescriptum — branding.h, embed.ipxe, PINNED and build.sh.
NOTICE

echo
echo "loaders in $OUT:"
ls -la "$OUT"
