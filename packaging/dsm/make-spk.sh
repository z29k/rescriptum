#!/usr/bin/env bash
# Assemble a Synology DSM 7 package around an already-built rescriptum binary.
#
#   ./packaging/dsm/make-spk.sh x86_64
#   ./packaging/dsm/make-spk.sh armv7 --bin dist/armv7/rescriptum --spk-build 2
#
# There is nothing to compile here: the binary is statically linked against musl and comes
# out of cargo-zigbuild. An `.spk` is two nested tar archives and a handful of text files,
# which is what the official toolkit's payload ends up being anyway — and doing it this
# way keeps the release job hermetic, reviewable, and free of a per-platform chroot.
#
# The mechanics are easy to get wrong and cheap to write down:
#
#   * the outer archive is an *uncompressed* tar. A gzipped one is rejected with "invalid
#     file format" and no further detail;
#   * --format=ustar explicitly, or bsdtar writes PaxHeader entries and GNU tar writes its
#     own format. Neither is worth discovering inside Package Center's error message;
#   * COPYFILE_DISABLE=1, or macOS puts ._INFO-style AppleDouble members in the archive;
#   * a pre-sorted file list rather than --sort=name, which is GNU-only;
#   * fixed ownership and mtimes, and `gzip -n` so the inner tarball carries no timestamp.
#     Same inputs and the same tar give a byte-identical .spk, which is what makes the
#     published checksum worth something. (GNU tar and bsdtar do not agree with each
#     other byte for byte; the release always runs on the same one.)
#   * scripts executable, #!/bin/sh, LF endings. A CRLF in a lifecycle script fails with
#     an error that names neither the file nor the reason.

set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)

usage() {
    sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

# What a build claims, and it is never the platform name. `arch` is a space-separated list
# and accepts *family* names — the guide's own example is arch="x86_64 alpine" — so an
# Intel package covers every Intel platform, including the ones Synology has not shipped
# yet. That is on purpose: the appendix's enumeration is already missing r1000 (DS723+,
# DS923+, DS1522+) and epyc7002, and enumerating it would exclude some of the most common
# models sold today until someone edited a table.
#
# The family shorthand does **not** reach the Marvell ARMv7 platforms: `armv7` as a family
# covers only alpine and alpine4k, so the DS416j's armada38x has to be named. The others
# (armada375, monaco, alpine, alpine4k) join at M6, each once it is confirmed DSM 7-capable
# *and* the binary has been run on it — armada370, armadaxp and comcerto2k never got DSM 7
# at all, and os_min_ver="7.0-40000" would make claiming them a lie.
abi_arch() {
    case "$1" in
    x86_64) echo "x86_64" ;;
    armv7) echo "armada38x" ;;
    aarch64) echo "armv8" ;;
    *) return 1 ;;
    esac
}

abi_target() {
    case "$1" in
    x86_64) echo "x86_64-unknown-linux-musl" ;;
    armv7) echo "armv7-unknown-linux-gnueabihf" ;;
    aarch64) echo "aarch64-unknown-linux-musl" ;;
    *) return 1 ;;
    esac
}

ABI=""
BIN=""
VERSION=""
SPK_BUILD=1
OUT="$REPO/dist"

while [ $# -gt 0 ]; do
    case "$1" in
    -h | --help) usage 0 ;;
    --bin) BIN="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --spk-build) SPK_BUILD="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    -*) echo "unknown option: $1" >&2; usage 2 ;;
    *) ABI="$1"; shift ;;
    esac
done

[ -n "$ABI" ] || usage 2
ARCH=$(abi_arch "$ABI") || { echo "unknown ABI: $ABI (x86_64, armv7, aarch64)" >&2; exit 2; }
TARGET=$(abi_target "$ABI")
[ -n "$BIN" ] || BIN="$REPO/target/$TARGET/release/rescriptum"

if [ ! -f "$BIN" ]; then
    echo "no binary at $BIN — build it first (./build.sh $TARGET)" >&2
    [ "$ABI" = armv7 ] && echo "(armv7 is glibc, not musl: Synology's 3.10 kernels break musl's time64 fallback)" >&2
    exit 1
fi

[ -n "$VERSION" ] || VERSION=$(grep -m1 '^version = ' "$REPO/Cargo.toml" | cut -d'"' -f2)
VERSION=${VERSION#v}

# Every segment of an SPK version must be numeric, each within 0…2^31-1. 0.1.0-1 is fine;
# 0.2.0-rc1 is not — a prerelease simply does not produce an .spk, the .tar.gz archives are
# the prerelease channel. The trailing segment is a package build number, for a packaging
# fix that has to ship without a code change.
case "$VERSION" in
*[!0-9.]*) echo "version $VERSION has a non-numeric segment — a prerelease does not produce an .spk" >&2; exit 1 ;;
esac
case "$SPK_BUILD" in
'' | *[!0-9]*) echo "--spk-build must be a number" >&2; exit 2 ;;
esac
FULL_VERSION="$VERSION-$SPK_BUILD"

STAGE=$(mktemp -d "${TMPDIR:-/tmp}/rescriptum-spk.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT
PAYLOAD="$STAGE/payload"
SPKDIR="$STAGE/spk"
mkdir -p "$PAYLOAD" "$SPKDIR"

# ── the payload, unpacked into /var/packages/rescriptum/target ──────────────────
mkdir -p "$PAYLOAD/bin"
cp "$BIN" "$PAYLOAD/bin/rescriptum"
chmod 755 "$PAYLOAD/bin/rescriptum"
cp -R "$HERE/payload/." "$PAYLOAD/"
chmod 755 "$PAYLOAD/bin/rescriptum-cli"
find "$PAYLOAD" -name '.DS_Store' -delete

# Kilobytes of unpacked payload. Left unset, extractsize does not mean "unknown": it means
# "the SPK's own byte size", which understates a compressed payload.
EXTRACTSIZE=$(du -sk "$PAYLOAD" | awk '{ print $1 }')

# ── the outer archive's members ────────────────────────────────────────────────
sed "s|@VERSION@|$FULL_VERSION|; s|@ARCH@|$ARCH|; s|@EXTRACTSIZE@|$EXTRACTSIZE|" "$HERE/INFO.in" |
    grep -v '^#' | grep -v '^$' >"$SPKDIR/INFO"

cp "$HERE/PACKAGE_ICON.PNG" "$HERE/PACKAGE_ICON_256.PNG" "$SPKDIR/"
cp "$REPO/LICENSE" "$SPKDIR/LICENSE"
cp -R "$HERE/conf" "$HERE/scripts" "$HERE/WIZARD_UIFILES" "$SPKDIR/"
chmod 755 "$SPKDIR"/scripts/*
chmod 644 "$SPKDIR"/conf/* "$SPKDIR"/WIZARD_UIFILES/* "$SPKDIR"/INFO "$SPKDIR"/LICENSE "$SPKDIR"/*.PNG

# Package Center shows this, and it is the only version history a DSM user ever sees.
{
    echo "$FULL_VERSION"
    if git -C "$REPO" rev-parse --git-dir >/dev/null 2>&1; then
        prev=$(git -C "$REPO" tag --list 'v*' --sort=-v:refname | grep -vx "v$VERSION" | head -n 1 || true)
        if [ -n "$prev" ]; then
            git -C "$REPO" log --no-merges --pretty=format:'- %s' "$prev..HEAD" | head -n 50
            echo
        else
            git -C "$REPO" log --no-merges --pretty=format:'- %s' -n 20
            echo
        fi
    fi
} >"$SPKDIR/CHANGELOG"

# ── the two archives ───────────────────────────────────────────────────────────
export COPYFILE_DISABLE=1
MTIME=${SPK_MTIME:-202001010000.00}

flavour=unknown
if tar --version 2>/dev/null | head -n 1 | grep -qi bsdtar; then
    flavour=bsd
elif tar --version 2>/dev/null | head -n 1 | grep -qi 'gnu tar'; then
    flavour=gnu
fi

# Directory entries come out before their contents under LC_ALL=C, which is what a tar
# reading the archive back wants.
listing() { (cd "$1" && find . -mindepth 1 | LC_ALL=C sort | sed 's|^\./||'); }

archive() { # <dir> <output>   — uncompressed, ustar, owned by 0:0, no recursion
    local dir="$1" out="$2" list
    list=$(mktemp "${TMPDIR:-/tmp}/spk-list.XXXXXX")
    listing "$dir" >"$list"
    find "$dir" -exec touch -t "$MTIME" {} +
    case "$flavour" in
    bsd) tar --format ustar --uid 0 --gid 0 --uname '' --gname '' --numeric-owner -n -C "$dir" -T "$list" -cf "$out" ;;
    *) tar --format=ustar --owner=0 --group=0 --numeric-owner --no-recursion -C "$dir" -T "$list" -cf "$out" ;;
    esac
    rm -f "$list"
}

archive "$PAYLOAD" "$STAGE/package.tar"
gzip -n -9 -c "$STAGE/package.tar" >"$SPKDIR/package.tgz"
rm -f "$STAGE/package.tar"

mkdir -p "$OUT"
SPK="$OUT/rescriptum-$FULL_VERSION-$ABI.spk"
archive "$SPKDIR" "$SPK"

# sha256sum is coreutils and is everywhere on Linux; shasum is a Perl script that a
# minimal image does not have, and macOS has only the second. Ubuntu runners happen to
# carry both, which is exactly how a script like this ships broken.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    else
        shasum -a 256 "$1"
    fi
}
( cd "$OUT" && sha256_of "$(basename "$SPK")" >"$(basename "$SPK").sha256" )

printf '    %-34s %8s KB installed   arch=%s\n' "$(basename "$SPK")" "$EXTRACTSIZE" "$ARCH"
echo "    $SPK"
