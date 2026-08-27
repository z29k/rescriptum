#!/usr/bin/env bash
# Drive the package's lifecycle scripts against a fake /var/packages tree, and assert every
# outcome. Runs anywhere the packaged binary runs — no NAS, no VM, no root.
#
#   packaging/dsm/lifecycle-test.sh                      # the first .spk in dist/ that runs here
#   packaging/dsm/lifecycle-test.sh dist/rescriptum-0.1.0-1-x86_64.spk
#
# What it covers is everything the *scripts* decide: the env file written once and only
# once, the wizard's values and their absence, the service actually starting and answering,
# the exit codes Package Center reads, an upgrade that must not touch a hand-edited
# configuration, and an uninstall that must not touch the answers. That is where the
# expensive mistakes live, and none of it needs DSM.
#
# What it cannot cover is DSM's own machinery: the data-share worker, the port-config
# worker, the generated systemd unit, and whether Package Center accepts the archive at
# all. Those are packaging/dsm/vm/on-dsm.sh, on a machine.
#
# The scripts under test come out of the .spk itself, not out of the working tree — the
# point is to test what would ship.

set -uo pipefail

REPO=$(cd "$(dirname "$0")/../.." && pwd)

pass=0
fails=0
ok() {
    echo "  ✓ $*"
    pass=$((pass + 1))
}
bad() {
    echo "  ✗ $*"
    fails=$((fails + 1))
}
section() { echo; echo "$*"; }

# ── the package under test ─────────────────────────────────────────────────────
SPK="${1:-}"
if [ -z "$SPK" ]; then
    for candidate in "$REPO"/dist/*.spk; do
        [ -f "$candidate" ] || continue
        SPK="$candidate"
        break
    done
fi
if [ -z "$SPK" ] || [ ! -f "$SPK" ]; then
    echo "no .spk to test — build one with packaging/dsm/make-spk.sh" >&2
    exit 2
fi
echo "$(basename "$SPK"):"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/spk-lifecycle.XXXXXX")
ROOT="$WORK/var/packages/rescriptum"
mkdir -p "$ROOT/target" "$ROOT/etc" "$ROOT/var" "$ROOT/shares/rescriptum"

tar -xOf "$SPK" package.tgz | tar -xzf - -C "$ROOT/target"
mkdir -p "$ROOT/scripts"
tar -xOf "$SPK" scripts/preinst >"$ROOT/scripts/preinst"
for s in postinst preuninst postuninst preupgrade postupgrade start-stop-status; do
    tar -xOf "$SPK" "scripts/$s" >"$ROOT/scripts/$s"
done
chmod 755 "$ROOT/scripts"/*

BIN="$ROOT/target/bin/rescriptum"
if ! "$BIN" --version >/dev/null 2>&1; then
    echo "  – the packaged binary does not run on this host; give this harness an .spk for it" >&2
    rm -rf "$WORK"
    exit 2
fi

cleanup() {
    if [ -f "$ROOT/var/rescriptum.pid" ]; then
        kill -KILL "$(cat "$ROOT/var/rescriptum.pid" 2>/dev/null)" 2>/dev/null
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

export SYNOPKG_PKGDEST="$ROOT/target"
# The scripts locate the package root at /var/packages/<name> — a real DSM resolves
# SYNOPKG_PKGDEST to /volume1/@appstore/<name>, so deriving it from there is wrong. This is
# the seam that lets them be driven against a tree we can actually write to.
export RESCRIPTUM_PKG_ROOT="$ROOT"
ENV_FILE="$ROOT/etc/rescriptum.env"
SHARE="$ROOT/shares/rescriptum"
sss() { sh "$ROOT/scripts/start-stop-status" "$@"; }

# The wizard's port has to be free, or "it did not answer" would mean nothing.
PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()' 2>/dev/null || echo 18123)

value_of() { sed -n "s/^$1=//p" "$ENV_FILE" | tail -n 1; }

# GNU first, and the order is the whole point: on BSD `stat -f` is the format flag, on GNU
# it means "filesystem status" — and it *succeeds*, printing a block of overlayfs trivia
# instead of failing over to the next branch. Asking GNU first fails cleanly on macOS.
file_mode() {
    stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1" 2>/dev/null
}

# ── 1. install ─────────────────────────────────────────────────────────────────
section "install"
out=$(SYNOPKG_PKG_STATUS=INSTALL pkgwizard_port="$PORT" sh "$ROOT/scripts/postinst" 2>&1)
[ -f "$ENV_FILE" ] && ok "postinst wrote the env file" || bad "postinst wrote no env file: $out"
mode=$(file_mode "$ENV_FILE")
[ "$mode" = "600" ] && ok "it is mode 600" || bad "it is mode $mode, not 600"
[ -f "$ROOT/target/etc/rescriptum.env.example" ] && ok "the example was written too" || bad "no example env file"
[ "$(value_of RESCRIPTUM_LISTEN_ADDR)" = "0.0.0.0:$PORT" ] && ok "the wizard's port reached the env file" || bad "listen addr is $(value_of RESCRIPTUM_LISTEN_ADDR)"
[ "$(value_of RESCRIPTUM_ANSWERS_DIR)" = "$SHARE/answers" ] && ok "the answers default to the share" || bad "answers dir is $(value_of RESCRIPTUM_ANSWERS_DIR)"
grep -q "^RESCRIPTUM_DB_PATH=$SHARE/answers.db\$" "$ENV_FILE" && ok "the database path is pre-set in the share" || bad "RESCRIPTUM_DB_PATH is not pre-set — switching stores would be a fatal start"
# All three listeners, registered whether or not each is currently enabled: registering
# a port does not open it, and the alternative is an operator who turns media or TFTP on
# and then cannot find rescriptum in the firewall list. **69/udp is the one that matters
# most** — a PXE ROM asks over UDP, and a firewall that drops it produces a client which
# retries and times out with nothing in any log on this side.
grep -q "dst.ports=\"$PORT/tcp 8001/tcp 69/udp\"" "$ROOT/target/port_conf/rescriptum.sc" && ok "the .sc file carries the answer port, the media one and TFTP" || bad ".sc file: $(tail -1 "$ROOT/target/port_conf/rescriptum.sc")"

# **The package must not ship a configuration that refuses to start.** Naming a media
# address with no media directory is a startup error, and the first version of this
# wrote exactly that — the harness caught a package that could not start at all.
# The folders the start script creates are *named* in the file, not left blank for an
# operator to guess at in a settings panel with no hint of what to type.
grep -q "^RESCRIPTUM_MEDIA_DIR=$SHARE/media\$" "$ENV_FILE" && ok "the media folder is named, not left to be guessed" || bad "RESCRIPTUM_MEDIA_DIR is not set to $SHARE/media"
grep -q "^RESCRIPTUM_BOOT_DIR=$SHARE/boot\$" "$ENV_FILE" && ok "and the boot folder too" || bad "RESCRIPTUM_BOOT_DIR is not set to $SHARE/boot"

# **rescriptum is the TFTP server, and the package must not ship a file that says
# otherwise.** A previous version wrote `RESCRIPTUM_TFTP_ADDR=off` here, trading the
# product's first principle for a packaging constraint; port 69 is reachable on DSM with
# one `setcap`, measured on a 7.2.2 machine. Left unset, the default is 0.0.0.0:69 —
# which is what the generated DHCP snippet and every loader we ship expect.
grep -q "^RESCRIPTUM_TFTP_ADDR=" "$ENV_FILE" && bad "RESCRIPTUM_TFTP_ADDR is live in the file — the default 0.0.0.0:69 is what the snippet and the loaders expect" || ok "TFTP is left at its default, so the package is the TFTP server"
grep -q "setcap cap_net_bind_service" "$ENV_FILE" && ok "and the file says what one root command makes it bind" || bad "nothing in the file explains how port 69 gets bound"
grep -q "Task Scheduler" "$ENV_FILE" && ok "and how to survive an upgrade, which drops the capability" || bad "nothing says the capability does not survive an upgrade"

section "install without a wizard (silent_install, or a reinstall that shows none)"
saved=$(cat "$ENV_FILE")
rm -f "$ENV_FILE"
SYNOPKG_PKG_STATUS=INSTALL sh "$ROOT/scripts/postinst" >/dev/null 2>&1
[ -f "$ENV_FILE" ] && ok "postinst still writes a complete file" || bad "postinst produced nothing without wizard values"
[ "$(value_of RESCRIPTUM_LISTEN_ADDR)" = "0.0.0.0:8000" ] && ok "and falls back to port 8000" || bad "fell back to $(value_of RESCRIPTUM_LISTEN_ADDR)"

section "a wizard port the package could not use"
rm -f "$ENV_FILE"
SYNOPKG_PKG_STATUS=INSTALL pkgwizard_port=80 sh "$ROOT/scripts/postinst" >/dev/null 2>&1
[ "$(value_of RESCRIPTUM_LISTEN_ADDR)" = "0.0.0.0:8000" ] && ok "a privileged port falls back rather than failing to bind later" || bad "kept $(value_of RESCRIPTUM_LISTEN_ADDR) — the package does not run as root"
rm -f "$ENV_FILE"
SYNOPKG_PKG_STATUS=INSTALL pkgwizard_port='8000; rm -rf /' sh "$ROOT/scripts/postinst" >/dev/null 2>&1
[ "$(value_of RESCRIPTUM_LISTEN_ADDR)" = "0.0.0.0:8000" ] && ok "a hostile wizard value is refused, not interpolated" || bad "listen addr became $(value_of RESCRIPTUM_LISTEN_ADDR)"

section "the custom answers path"
rm -f "$ENV_FILE"
out=$(SYNOPKG_PKG_STATUS=INSTALL pkgwizard_answers_custom=true pkgwizard_answers_path=/volume1/netboot/answers sh "$ROOT/scripts/postinst" 2>&1)
[ "$(value_of RESCRIPTUM_ANSWERS_DIR)" = "/volume1/netboot/answers" ] && ok "the path reaches the env file" || bad "answers dir is $(value_of RESCRIPTUM_ANSWERS_DIR)"
case "$out" in
*"read access"*) ok "and the package says it cannot permission it for you" ;;
*) bad "nothing warned that the package cannot grant itself access" ;;
esac

printf '%s\n' "$saved" >"$ENV_FILE"
chmod 600 "$ENV_FILE"

# ── 2. the service ─────────────────────────────────────────────────────────────
section "start, and stay started"
sss status >/dev/null 2>&1
[ $? -eq 3 ] && ok "status is 3 (not running) before the first start" || bad "status before start was $?, not 3"

out=$(sss start 2>&1)
rc=$?
[ $rc -eq 0 ] && ok "start returns 0" || bad "start returned $rc: $out"
[ -d "$SHARE/answers" ] && ok "start created the answers directory inside the share" || bad "no answers directory — DSM creates the share, not this"
# Made whether or not the env file names them yet: a folder that only appears once a
# setting is enabled is one nobody discovers.
[ -d "$SHARE/media" ] && ok "and the media folder, ready for an ISO" || bad "no $SHARE/media"
[ -d "$SHARE/boot" ] && ok "and the boot folder, which is what TFTP hands loaders out of" || bad "no $SHARE/boot"

answered=no
for _ in 1 2 3 4 5 6 7 8 9 10; do
    if [ "$(curl -fsS "http://127.0.0.1:$PORT/health" 2>/dev/null)" = "OK" ]; then
        answered=yes
        break
    fi
    sleep 0.5
done
[ "$answered" = yes ] && ok "/health answers on the port the wizard chose" || bad "/health never answered on $PORT"

sleep 2
sss status >/dev/null 2>&1
[ $? -eq 0 ] && ok "it is still alive seconds later (not reaped when start returned)" || bad "the service did not survive its own start script"

section "the exit codes Package Center reads"
sss stop >/dev/null 2>&1
[ $? -eq 0 ] && ok "stop returns 0" || bad "stop returned non-zero"
sss status >/dev/null 2>&1
[ $? -eq 3 ] && ok "a cleanly stopped package is 3" || bad "stopped status was $?, and 1 would tell Package Center it crashed"
echo 999999 >"$ROOT/var/rescriptum.pid"
sss status >/dev/null 2>&1
[ $? -eq 1 ] && ok "a dead process with a pidfile left behind is 1" || bad "stale pidfile reported $?"
sss stop >/dev/null 2>&1
[ $? -eq 0 ] && ok "stop over a stale pidfile succeeds and clears it" || bad "stop over a stale pidfile failed"
sss prestart >/dev/null 2>&1
[ $? -eq 0 ] && ok "prestart says yes — it runs at boot, and a no is permanent" || bad "prestart refused"
sss wibble >/dev/null 2>&1
[ $? -eq 0 ] && ok "an unrecognised verb exits 0 rather than blocking boot" || bad "an unknown verb exited non-zero"

section "a start that cannot succeed"
cp "$ENV_FILE" "$WORK/env.good"
printf 'RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8199\nRESCRIPTUM_ADMIN_TOKEN=short\nRESCRIPTUM_STORE=sqlite\n' >>"$ENV_FILE"
out=$(sss start 2>&1)
rc=$?
[ $rc -eq 1 ] && ok "start reports failure instead of a success that is already gone" || bad "start returned $rc for a configuration the server refuses"
case "$out" in
*"RESCRIPTUM_ADMIN_TOKEN"*) ok "and prints the reason, from the log the server actually used" ;;
*) bad "the reason was not shown: $out" ;;
esac
cp "$WORK/env.good" "$ENV_FILE"

# ── 3. the upgrade, adversarially ──────────────────────────────────────────────
section "an upgrade must not touch a hand-edited configuration"
printf 'RESCRIPTUM_LOG=problems\nRESCRIPTUM_ANSWER_TOKEN=a-token-nobody-should-lose\n' >>"$ENV_FILE"
cp "$ENV_FILE" "$WORK/env.handedited"
echo "do not delete me" >"$SHARE/answers/canary.toml"
UPG="$WORK/upgrade"
export SYNOPKG_TEMP_UPGRADE_FOLDER="$UPG"

upgrade() { # <wipe-etc yes|no>
    rm -rf "$UPG"
    mkdir -p "$UPG"
    # The documented order: preupgrade (new) → preuninst/postuninst (old) → preinst,
    # postinst (new) → postupgrade.
    SYNOPKG_PKG_STATUS=UPGRADE sh "$ROOT/scripts/preupgrade" >/dev/null 2>&1
    SYNOPKG_PKG_STATUS=UPGRADE sh "$ROOT/scripts/preuninst" >/dev/null 2>&1
    SYNOPKG_PKG_STATUS=UPGRADE sh "$ROOT/scripts/postuninst" >/dev/null 2>&1
    [ "$1" = yes ] && rm -f "$ENV_FILE"
    SYNOPKG_PKG_STATUS=UPGRADE sh "$ROOT/scripts/preinst" >/dev/null 2>&1
    SYNOPKG_PKG_STATUS=UPGRADE pkgwizard_port=9999 sh "$ROOT/scripts/postinst" >/dev/null 2>&1
    SYNOPKG_PKG_STATUS=UPGRADE sh "$ROOT/scripts/postupgrade" >/dev/null 2>&1
}

upgrade no
diff -q "$WORK/env.handedited" "$ENV_FILE" >/dev/null && ok "etc/ survives: the file is untouched, wizard values and all" || bad "the upgrade rewrote the user's env file"
[ -f "$SHARE/answers/canary.toml" ] && ok "the canary in the share survived" || bad "the upgrade destroyed a file in the share"

upgrade yes
diff -q "$WORK/env.handedited" "$ENV_FILE" >/dev/null && ok "etc/ wiped: postinst restored it from the upgrade folder rather than writing defaults" || bad "the user's configuration was replaced by defaults — postinst runs BEFORE postupgrade"
mode=$(file_mode "$ENV_FILE")
[ "$mode" = "600" ] && ok "and it came back mode 600" || bad "the restored file is mode $mode"

section "a fresh install must not resurrect a removed installation"
# The temp folder outlives the upgrade that created it. A later fresh install that found a
# copy of the old configuration there used to restore it — tokens and all — which is how a
# removed-and-reinstalled package came back up with somebody's old answer token.
rm -f "$ENV_FILE"
SYNOPKG_PKG_STATUS=INSTALL pkgwizard_port="$PORT" sh "$ROOT/scripts/postinst" >/dev/null 2>&1
if grep -q 'a-token-nobody-should-lose' "$ENV_FILE"; then
    bad "a fresh install restored the previous installation's configuration"
else
    ok "a fresh install writes defaults even with a stale upgrade folder in place"
fi
cp "$WORK/env.handedited" "$ENV_FILE"

section "postupgrade on its own is still a backstop"
rm -rf "$UPG"
mkdir -p "$UPG"
SYNOPKG_PKG_STATUS=UPGRADE sh "$ROOT/scripts/preupgrade" >/dev/null 2>&1
rm -f "$ENV_FILE"
SYNOPKG_PKG_STATUS=UPGRADE sh "$ROOT/scripts/postupgrade" >/dev/null 2>&1
diff -q "$WORK/env.handedited" "$ENV_FILE" >/dev/null && ok "restored" || bad "postupgrade did not restore the file"

# ── 4. the desktop application's backend ───────────────────────────────────────
# The CGI DSM serves at /webman/3rdparty/rescriptum/api.cgi. Two things measured on a DSM
# 7.2.2 machine make this section matter more than it looks: a CGI there runs as **root**,
# and that path is **not authenticated by DSM** — an unauthenticated request reaches the
# script and gets 200. So the checks inside it are the only door, and this is where they
# are proved to be shut.
section "the DSM application's CGI"

CGI="$ROOT/target/ui/api.cgi"
AUTH_STUB="$WORK/authenticate.cgi"
MY_GROUP=$(id -gn)

# DSM's own authenticator prints the logged-in user's name, and prints nothing at all when
# there is no session. Both halves are what the real script keys on.
signed_in_as() {
    printf '#!/bin/sh\nprintf %%s "%s"\n' "$1" >"$AUTH_STUB"
    chmod 755 "$AUTH_STUB"
}

# One request. Everything a client could actually influence — the method, the query, the
# body, a header — goes in as CGI variables; everything else is the seam the harness needs.
cgi() { # <method> <query> <body> <x-rescriptum> [admin-group]
    REQUEST_METHOD="$1" QUERY_STRING="$2" CONTENT_LENGTH="${#3}" \
        HTTP_X_RESCRIPTUM="$4" \
        RESCRIPTUM_PKG_ROOT="$ROOT" RESCRIPTUM_AUTH_CGI="$AUTH_STUB" \
        RESCRIPTUM_ADMIN_GROUP="${5:-$MY_GROUP}" \
        sh "$CGI" <<CGIBODY
$3
CGIBODY
}

http_status() { sed -n 's/^Status: \([0-9]*\).*/\1/p' <<<"$1" | head -n 1; }

[ -x "$CGI" ] && ok "the application's CGI shipped executable" || bad "$CGI is not executable — DSM would serve its source"

signed_in_as ""
out=$(cgi GET "action=config" "" "")
[ "$(http_status "$out")" = "403" ] && ok "no DSM session is refused" || bad "an unauthenticated request got $(http_status "$out") — that path is open to the network"

signed_in_as "someuser"
out=$(cgi GET "action=config" "" "" "a-group-nobody-is-in")
[ "$(http_status "$out")" = "403" ] && ok "a signed-in non-administrator is refused" || bad "a non-administrator got $(http_status "$out")"

signed_in_as "$(id -un)"
out=$(cgi GET "action=config" "" "")
[ "$(http_status "$out")" = "200" ] && ok "an administrator is served" || bad "an administrator got $(http_status "$out"): $out"
grep -q '"key":"RESCRIPTUM_STORE"' <<<"$out" && ok "and gets the configuration as JSON" || bad "the JSON does not carry the settings: $out"

# The panel renders this. A token reaching it is a token in a browser and a screenshot.
RESCRIPTUM_PKG_ROOT="$ROOT" "$ROOT/target/bin/rescriptum-cli" config set RESCRIPTUM_ANSWER_TOKEN=n0tf0rth3br0ws3r >/dev/null 2>&1
out=$(cgi GET "action=config" "" "")
grep -q 'n0tf0rth3br0ws3r' <<<"$out" && bad "a token reached the application" || ok "no token reaches the application"

# Compared against the whole file rather than one value: an earlier section leaves its own
# settings behind, and "it is still not X" proves nothing when it was never X.
before=$(cat "$ENV_FILE")
out=$(cgi POST "action=save" "RESCRIPTUM_MAX_CONNECTIONS=4096" "")
[ "$(http_status "$out")" = "403" ] && ok "a write without X-Rescriptum is refused" || bad "the CSRF guard let a write through with $(http_status "$out")"
[ "$(cat "$ENV_FILE")" = "$before" ] && ok "and changed nothing" || bad "the refused write was applied anyway"

out=$(cgi POST "action=save" "RESCRIPTUM_MAX_CONNECTIONS=4096" "1")
[ "$(http_status "$out")" = "200" ] && ok "a write from the application is applied" || bad "the write got $(http_status "$out"): $out"
[ "$(value_of RESCRIPTUM_MAX_CONNECTIONS)" = "4096" ] && ok "and reached the env file" || bad "the env file says $(value_of RESCRIPTUM_MAX_CONNECTIONS)"

# The refusal that matters: the panel is reached over the service this would stop.
before=$(cat "$ENV_FILE")
out=$(cgi POST "action=save" "RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001" "1")
[ "$(http_status "$out")" = "409" ] && ok "a write that would stop the server starting is refused" || bad "an unauthenticated admin API was accepted with $(http_status "$out")"
[ "$(cat "$ENV_FILE")" = "$before" ] && ok "and the file is untouched" || bad "the refused write still changed the file"

out=$(cgi GET "action=nonsense" "" "")
[ "$(http_status "$out")" = "400" ] && ok "an unknown action is refused" || bad "an unknown action got $(http_status "$out")"

out=$(cgi GET "action=status" "" "")
grep -q '^version: rescriptum' <<<"$out" && ok "status reports the version" || bad "status: $out"
# It answers, rather than hanging: the first version of this asked `su` to become the
# service's user, which read the CGI's stdin and waited on it forever. The CGI already
# *is* that user, so a plain test is both possible and correct.
grep -q '^answers_readable: yes' <<<"$out" && ok "and can tell that the answers folder is readable" || bad "status says the answers folder is unreadable: $out"
# **A TFTP port that cannot be bound does not stop the server**, so this line is the only
# place an operator sees it after the startup warning has scrolled away. In this harness
# nothing has bound port 69 and nothing could, so the honest answer is one of the two
# not-working states — what must never happen is silence or a claim that it is fine.
grep -qE '^tftp: (serving|broken|silent|off)$' <<<"$out" && ok "and says whether a loader can actually be handed over" || bad "status has no usable tftp line: $out"
grep -q '^tftp: serving' <<<"$out" && bad "status claims TFTP is serving, with nothing bound to port 69" || ok "and does not claim to be serving when nothing is bound"

# ── 5. uninstall ───────────────────────────────────────────────────────────────
section "uninstall must leave the answers alone"
unset SYNOPKG_TEMP_UPGRADE_FOLDER
SYNOPKG_PKG_STATUS=UNINSTALL sh "$ROOT/scripts/preuninst" >/dev/null 2>&1
SYNOPKG_PKG_STATUS=UNINSTALL sh "$ROOT/scripts/postuninst" >/dev/null 2>&1
[ -f "$SHARE/answers/canary.toml" ] && ok "the share and everything in it survived the uninstall" || bad "the uninstall took the user's answers with it"
[ -d "$SHARE" ] && ok "the shared folder itself is still there" || bad "the shared folder was removed"

echo
if [ "$fails" -gt 0 ]; then
    echo "$pass passed, $fails failed"
    exit 1
fi
echo "$pass checks passed"
