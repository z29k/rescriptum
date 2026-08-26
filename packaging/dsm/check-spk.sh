#!/usr/bin/env bash
# Check an assembled .spk structurally.
#
#   ./packaging/dsm/check-spk.sh dist/rescriptum-0.1.0-1-x86_64.spk
#   ./packaging/dsm/check-spk.sh            # everything in dist/
#
# This cannot prove DSM will accept the package — nothing short of installing it can, which
# is what the milestones on a VM and on the real DS416j are for. What it does catch is the
# entire class of "the release job produced a 0-byte tarball", cheaply, on every push.

set -euo pipefail

REPO=$(cd "$(dirname "$0")/../.." && pwd)

# Family names and platform names both belong in `arch`. The x86_64 row of the guide's
# appendix is already stale — it omits r1000 (DS723+, DS923+, DS1522+) and epyc7002 — which
# is exactly why we ship the family value and why this list is only a sanity check.
KNOWN_ARCH="noarch
x86_64 i686 armv7 armv5 armv8
apollolake avoton braswell broadwell broadwellnk broadwellntb broadwellntbap bromolow
cedarview coffeelake denverton epyc7002 epyc7003 geminilake geminilakenk grantley kvmx64
purley r1000 skylaked v1000
evansport
alpine alpine4k armada370 armada375 armada38x armadaxp comcerto2k monaco
88f6281 88f6282 628x
armada37xx rtd1296 rtd1619 rtd1619b"

REQUIRED_SCRIPTS="preinst postinst preuninst postuninst preupgrade postupgrade start-stop-status"

fails=0
bad() { echo "  ✗ $*"; fails=$((fails + 1)); }
ok() { echo "  ✓ $*"; }
skip() { echo "  – $*"; }

png_size() {
    local hex
    hex=$(od -An -tx1 -j16 -N8 "$1" | tr -d ' \n')
    echo "$((16#${hex:0:8}))x$((16#${hex:8:8}))"
}

info_value() { sed -n "s/^$1=\"\(.*\)\"$/\1/p" "$2"; }

check_one() {
    local spk="$1" work
    echo "$(basename "$spk"):"

    # A .spk whose outer tar is gzipped is rejected with "invalid file format" and no
    # further detail. Worth catching here rather than in Package Center.
    local kind
    kind=$(file -b "$spk")
    case "$kind" in
    *"tar archive"*) ok "outer archive is an uncompressed tar" ;;
    *) bad "outer archive is not an uncompressed tar: $kind" ;;
    esac
    tar -tf "$spk" >/dev/null 2>&1 || bad "the outer archive does not list"

    # bsdtar's pax headers and macOS's AppleDouble members both travel invisibly and both
    # break the package.
    if tar -tf "$spk" | grep -qE '(^|/)(\._|PaxHeader)'; then
        bad "the archive carries PaxHeader or ._ members"
    else
        ok "no PaxHeader or ._ members"
    fi

    work=$(mktemp -d "${TMPDIR:-/tmp}/spk-check.XXXXXX")
    tar -xf "$spk" -C "$work"

    # ── INFO ───────────────────────────────────────────────────────────────────
    if [ ! -f "$work/INFO" ]; then
        bad "no INFO"
        rm -rf "$work"
        return
    fi
    local missing="" field
    for field in package version os_min_ver description arch maintainer; do
        [ -n "$(info_value "$field" "$work/INFO")" ] || missing="$missing $field"
    done
    if [ -n "$missing" ]; then bad "INFO is missing:$missing"; else ok "INFO has the six required fields"; fi

    local version
    version=$(info_value version "$work/INFO")
    case "$version" in
    '' | *[!0-9.-]* | *--*) bad "version \"$version\" has a segment DSM cannot parse" ;;
    *) ok "version $version is all-numeric segments" ;;
    esac

    local arch bad_arch=""
    for arch in $(info_value arch "$work/INFO"); do
        grep -qw -- "$arch" <<<"$KNOWN_ARCH" || bad_arch="$bad_arch $arch"
    done
    if [ -n "$bad_arch" ]; then
        bad "arch names nothing known:$bad_arch"
    else
        ok "arch=\"$(info_value arch "$work/INFO")\""
    fi

    # extractsize left unset does not mean "unknown": it means "the SPK's own byte size".
    local extract
    extract=$(info_value extractsize "$work/INFO")
    case "$extract" in
    '' | *[!0-9]*) bad "extractsize is not a number of kilobytes: \"$extract\"" ;;
    *) ok "extractsize=$extract KB" ;;
    esac

    # ── icons ──────────────────────────────────────────────────────────────────
    local icon want
    for icon in "PACKAGE_ICON.PNG 64x64" "PACKAGE_ICON_256.PNG 256x256"; do
        set -- $icon
        want="$2"
        if [ ! -f "$work/$1" ]; then
            bad "no $1"
        elif [ "$(png_size "$work/$1")" != "$want" ]; then
            bad "$1 is $(png_size "$work/$1"), not $want"
        else
            ok "$1 is $want"
        fi
    done

    # ── conf and wizard ────────────────────────────────────────────────────────
    local f
    for f in conf/privilege conf/resource WIZARD_UIFILES/install_uifile; do
        if [ ! -f "$work/$f" ]; then
            bad "no $f"
        elif command -v python3 >/dev/null 2>&1 && ! python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$work/$f" 2>/dev/null; then
            bad "$f is not valid JSON"
        else
            ok "$f"
        fi
    done
    # DSM 7 requires the package to lower its privilege explicitly, and a username that
    # does not match data-share's permission list creates the share and grants it to
    # nobody — silently.
    if grep -q '"run-as"[[:space:]]*:[[:space:]]*"package"' "$work/conf/privilege" 2>/dev/null; then
        ok "conf/privilege runs as the package user"
    else
        bad "conf/privilege does not set run-as: package"
    fi
    local user
    user=$(sed -n 's/.*"username"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$work/conf/privilege" 2>/dev/null)
    if [ -n "$user" ] && grep -q "\"$user\"" "$work/conf/resource" 2>/dev/null; then
        ok "the package user ($user) is the one data-share grants"
    else
        bad "conf/privilege's username and conf/resource's share permission disagree"
    fi

    # ── lifecycle scripts ──────────────────────────────────────────────────────
    local script before="$fails"
    for script in $REQUIRED_SCRIPTS; do
        if [ ! -f "$work/scripts/$script" ]; then
            bad "no scripts/$script"
            continue
        fi
        [ -x "$work/scripts/$script" ] || bad "scripts/$script is not executable"
        head -n 1 "$work/scripts/$script" | grep -q '^#!/bin/sh' || bad "scripts/$script does not start with #!/bin/sh"
        # A CRLF in a lifecycle script fails with an error that names neither the file nor
        # the reason.
        if grep -q $'\r' "$work/scripts/$script"; then bad "scripts/$script has CRLF line endings"; fi
        sh -n "$work/scripts/$script" || bad "scripts/$script does not parse"
    done
    [ "$fails" -eq "$before" ] && ok "the seven lifecycle scripts are present, executable and parse"

    if command -v shellcheck >/dev/null 2>&1; then
        if shellcheck -s sh -S warning "$work"/scripts/*; then
            ok "shellcheck is happy"
        else
            bad "shellcheck found something"
        fi
    else
        skip "shellcheck not installed"
    fi

    # ── the payload ────────────────────────────────────────────────────────────
    if [ ! -f "$work/package.tgz" ]; then
        bad "no package.tgz"
        rm -rf "$work"
        return
    fi
    mkdir -p "$work/target"
    if ! tar -xzf "$work/package.tgz" -C "$work/target"; then
        bad "package.tgz does not unpack"
        rm -rf "$work"
        return
    fi
    for f in bin/rescriptum bin/rescriptum-cli port_conf/rescriptum.sc logrotate/rescriptum; do
        [ -f "$work/target/$f" ] || bad "the payload has no $f"
    done
    [ -x "$work/target/bin/rescriptum" ] || bad "the payload's binary is not executable"
    ok "the payload carries the binary, the wrapper, the .sc file and the logrotate stanza"

    # The stanza's whole point: log::init opens the file once and never reopens it, so a
    # rotation without copytruncate silently ends logging.
    grep -q '^[[:space:]]*copytruncate' "$work/target/logrotate/rescriptum" &&
        ok "the logrotate stanza uses copytruncate" ||
        bad "the logrotate stanza does not use copytruncate"

    # The firewall entry never appears if the file is not named after the package, and
    # nothing says why.
    grep -q '^\[rescriptum\]' "$work/target/port_conf/rescriptum.sc" &&
        ok "port_conf/rescriptum.sc declares [rescriptum]" ||
        bad "port_conf/rescriptum.sc does not declare [rescriptum]"

    # Not a re-read of the same string: this runs the binary that is actually in the
    # package. It only works where the ABI matches the host, which on CI is the x86_64 one.
    local reported="" want_version
    want_version=${version%-*}
    if reported=$("$work/target/bin/rescriptum" --version 2>/dev/null); then
        if [ "$reported" = "rescriptum $want_version" ]; then
            ok "the packaged binary reports $reported"
        else
            bad "the packaged binary reports \"$reported\", INFO says $version"
        fi
    else
        skip "the packaged binary does not run on this host (wrong ABI) — version not checked"
    fi

    rm -rf "$work"
}

targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
    while IFS= read -r line; do targets+=("$line"); done < <(find "$REPO/dist" -name '*.spk' 2>/dev/null | sort)
fi
if [ ${#targets[@]} -eq 0 ]; then
    echo "no .spk to check (build one with packaging/dsm/make-spk.sh)" >&2
    exit 2
fi

for spk in "${targets[@]}"; do check_one "$spk"; done

if [ "$fails" -gt 0 ]; then
    echo
    echo "$fails problem(s)"
    exit 1
fi
echo
echo "all checks passed"
