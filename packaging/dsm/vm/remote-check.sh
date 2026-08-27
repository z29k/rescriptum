#!/bin/sh
# Runs **on a DSM 7 machine, as root** — a VM for iteration, the DS416j for the verdict.
#
# on-dsm.sh copies this here with two .spk files (build 1 and build 2 of the same version)
# and runs it. Everything the lifecycle harness cannot reach is here: the data-share
# worker, the port-config worker, the generated systemd unit, logrotate, and whether
# Package Center accepts the archive at all.
#
# Two rules it is written to:
#
#   * **assert on effects, not on exit codes.** DSM's own CLI differs between builds, so
#     every step says what it ran and then checks what changed on the machine.
#   * **destroy things on purpose.** The canary file and the hand-edited env line exist to
#     be attacked by the upgrade and the uninstall. A guard that was never watched failing
#     proves nothing.
#
# It leaves the machine clean: the package is uninstalled at the end, and the share it
# created is reported rather than removed — removing it is the one thing this must never do.

set -u

# A non-login shell on DSM gets PATH=/usr/bin:/bin:/usr/sbin:/sbin, and every tool this
# script needs — synopkg, synogetkeyvalue, synopkghelper — lives outside it. Found by
# running this over ssh rather than by reading anything.
PATH="$PATH:/usr/syno/bin:/usr/syno/sbin:/usr/local/bin:/usr/local/sbin"
export PATH

PKG=rescriptum
ROOT=/var/packages/$PKG
SHARE=$ROOT/shares/$PKG
RIG=$(cd "$(dirname "$0")" && pwd)
SPK1="$RIG/build1.spk"
SPK2="$RIG/build2.spk"
PORT=8000

pass=0
fails=0
ok() { echo "  ✓ $*"; pass=$((pass + 1)); }
bad() { echo "  ✗ $*"; fails=$((fails + 1)); }
note() { echo "  · $*"; }
section() { echo; echo "== $*"; }
run() { echo "  \$ $*"; "$@" 2>&1 | sed 's/^/    /'; }

[ "$(id -u)" = 0 ] || { echo "run this as root"; exit 2; }
[ -f "$SPK1" ] || { echo "no $SPK1 — on-dsm.sh copies it here"; exit 2; }

# ── 0. what machine is this ────────────────────────────────────────────────────
section "the machine"
note "$(cat /etc.defaults/VERSION 2>/dev/null | tr '\n' ' ')"
note "platform: $(synogetkeyvalue /etc.defaults/synoinfo.conf unique 2>/dev/null)"
note "arch: $(uname -m), kernel $(uname -r)"
case "$(sed -n 's/^majorversion="\(.*\)"$/\1/p' /etc.defaults/VERSION 2>/dev/null)" in
7 | 8 | 9) ok "DSM 7 or newer" ;;
*) bad "this is not DSM 7; nothing below applies" ; exit 1 ;;
esac
# The desktop application is built on DSM's ExtJS framework, present on 7.1.1 and 7.2.2.
# Below 7.1 the package refuses to install at all and the checks that follow would be
# misleading. **This gate said 7.2 for a while** — written when the application was still
# built on DSM 7.2's Vue framework, and not moved when it went back to ExtJS. It would have
# failed the DS416j, which is the one machine this rig exists to satisfy.
case "$(sed -n 's/^productversion="\(.*\)"$/\1/p' /etc.defaults/VERSION 2>/dev/null)" in
7.0*) bad "DSM $(sed -n 's/^productversion="\(.*\)"$/\1/p' /etc.defaults/VERSION) is below the package's os_min_ver of 7.1" ;;
*) ok "at or above the 7.1 the desktop application needs" ;;
esac

# ── 1. install ─────────────────────────────────────────────────────────────────
# Our own leftovers from a previous run, and only those: this script must never remove
# anything else from the share. Without it a stale canary or test answer makes the next
# run fail for a reason that has nothing to do with the package.
rm -rf /volume1/*/answers/canary.txt /volume1/*/answers/canary.toml \
       /volume1/*/answers/default.toml /volume1/*/answers/98-fa-9b-50-d8-10.toml \
       /volume1/*/answers/groups 2>/dev/null

# **etc/ and var/ survive an uninstall.** /var/packages/<pkg>/etc and /var/packages/<pkg>/var
# are symlinks into /volume1/@appconf/<pkg> and /volume1/@appdata/<pkg>, and DSM leaves both
# behind — so the env file, tokens included, outlives the package. A first install on a
# fresh NAS has neither, and a rig that keeps them is not testing a first install: it spent
# three runs failing on an env file poisoned by an earlier one.
rm -rf /volume1/@appconf/$PKG /volume1/@appdata/$PKG 2>/dev/null

section "install"
synopkg uninstall "$PKG" >/dev/null 2>&1
run synopkg install "$SPK1"
if [ -d "$ROOT/target" ]; then
    ok "the package installed"
else
    bad "no $ROOT/target — Package Center refused it. /var/log/synopkg.log says why:"
    tail -n 20 /var/log/synopkg.log 2>&1 | sed 's/^/    /'
    exit 1
fi

id "$PKG" >/dev/null 2>&1 && ok "the '$PKG' user exists ($(id "$PKG"))" || bad "no '$PKG' user — conf/privilege did not take"
[ -f "$ROOT/etc/$PKG.env" ] && ok "postinst wrote the env file" || bad "no env file"
[ "$(stat -c '%a' "$ROOT/etc/$PKG.env" 2>/dev/null)" = 600 ] && ok "it is mode 600" || note "mode is $(stat -c '%a%U' "$ROOT/etc/$PKG.env" 2>/dev/null)"
[ "$(stat -c '%U' "$ROOT/etc/$PKG.env" 2>/dev/null)" = "$PKG" ] && ok "and owned by the package user, for free" || note "owner is $(stat -c '%U' "$ROOT/etc/$PKG.env" 2>/dev/null)"
[ -f "$ROOT/target/etc/$PKG.env.example" ] && ok "the example env file is there too" || bad "no example env file"

note "port-config and usr-local-linker are checked after start: their windows open when"
note "the package is *enabled*, not when postinst runs — checked here they are always absent"

# ── 2. the share, which is the part that silently 404s when it is wrong ─────────
section "the shared folder"
if [ -d "$SHARE" ]; then
    ok "data-share created it, and the symlink is at $SHARE"
    note "→ $(readlink -f "$SHARE")"
else
    bad "no $SHARE — data-share runs at package *start*, so start it and look again"
fi

# ── 3. start ───────────────────────────────────────────────────────────────────
section "start"
run synopkg start "$PKG"
sleep 3
PORT=$(sed -n 's/^RESCRIPTUM_LISTEN_ADDR=.*:\([0-9]*\)$/\1/p' "$ROOT/etc/$PKG.env" | tail -n 1)
[ -n "$PORT" ] || PORT=8000

[ -d "$SHARE/answers" ] && ok "start created the answers directory inside the share" || bad "no $SHARE/answers"
# The media and boot folders, made whether or not the env file names them yet: a folder
# that only appears once a setting is enabled is one nobody discovers, and the boot one
# is what DSM's own TFTP server gets pointed at. The package cannot serve TFTP itself —
# port 69 is privileged and DSM 7 does not let an unsigned package run as root.
for extra in media boot; do
    [ -d "$SHARE/$extra" ] && ok "and the $extra folder, ready to be filled" || bad "no $SHARE/$extra"
    sudo -u "$PKG" test -w "$SHARE/$extra" 2>/dev/null &&
        ok "  which the package user can write" ||
        bad "  but the package user cannot write it"
done
if sudo -u "$PKG" test -w "$SHARE/answers" 2>/dev/null; then
    ok "the package user can write it — the ACL landed on the right name"
else
    bad "the package user cannot write $SHARE/answers: data-share's permission list and conf/privilege's username disagree"
fi

PID=$(cat "$ROOT/var/$PKG.pid" 2>/dev/null)
if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    ok "it survived its own start script (pid $PID)"
else
    bad "the service was reaped when start returned — look at KillMode below"
    tail -n 10 "$ROOT/var/startup.log" 2>/dev/null | sed 's/^/    /'
fi

[ "$(curl -fsS "http://127.0.0.1:$PORT/health" 2>/dev/null)" = OK ] && ok "/health answers on $PORT" || bad "/health did not answer on $PORT"

section "the resource workers, now that the package is enabled"
# The developer guide says the port-config worker copies the file to
# /usr/local/etc/service.d/. On DSM 7.2.2 the directory is **/usr/local/etc/services.d/**,
# plural — SMBService.sc and ScsiTarget.sc live there — and service.d does not exist at all.
# Look in both rather than trusting either.
SC=""
for d in /usr/local/etc/services.d /usr/local/etc/service.d; do
    [ -f "$d/$PKG.sc" ] && SC="$d/$PKG.sc" && break
done
if [ -n "$SC" ]; then
    ok "the firewall entry was acquired: $SC"
    note "it says:        $(grep dst.ports "$SC")"
    note "postinst wrote: $(grep dst.ports "$ROOT/target/port_conf/$PKG.sc")"
    if grep -q "$(grep dst.ports "$ROOT/target/port_conf/$PKG.sc")" "$SC"; then
        note "→ the worker acquired AFTER postinst: the wizard's port reaches the firewall"
    else
        note "→ the worker acquired BEFORE postinst: the .sc ships 8000, and only"
        note "  'synopkghelper update $PKG port-config' moves it afterwards"
    fi
else
    bad "no $PKG.sc in /usr/local/etc/services.d or service.d — the firewall entry will never appear"
    note "what is there: $(ls /usr/local/etc/services.d 2>/dev/null | tr '\n' ' ')"
fi

if [ -e /usr/local/bin/$PKG-cli ]; then
    ok "rescriptum-cli is on PATH"
else
    bad "usr-local-linker did not link the CLI into /usr/local/bin"
fi

# ── the whole point: does it actually answer a machine? ────────────────────────
section "answering a machine, which is what the package exists to do"
# /health proves the process is up. It does not prove that a machine asking for its
# configuration gets one — selection, group membership, merging and the format/endpoint
# binding all sit between the two, and all of them read files from the share as the package
# user. This is the assertion that covers the actual product on the actual machine.
mkdir -p "$SHARE/answers/groups"
cat >"$SHARE/answers/groups/rack.toml" <<'GROUP'
members = ["98:fa:9b:50:d8:10"]

[global]
keyboard = "fr"
GROUP
cat >"$SHARE/answers/98-fa-9b-50-d8-10.toml" <<'MACHINE'
[global]
fqdn = "rig-machine.example.com"
MACHINE
cat >"$SHARE/answers/default.toml" <<'DEFAULT'
[global]
fqdn = "should-not-be-served.example.com"
DEFAULT
chown -R "$PKG" "$SHARE/answers" 2>/dev/null
chmod -R u+rwX "$SHARE/answers" 2>/dev/null

# The way a Proxmox installer asks since PVE 8.2: POST, hardware in the body, answer in the
# response. The MAC is only in the body — nothing in the URL says which machine this is.
BODY='{"dmi":{"system":{"serial":"RIG-0001"}},"network_interfaces":[{"mac":"98:FA:9B:50:D8:10","link":true}]}'
ANSWER=$(curl -fsS -m 20 -X POST --data "$BODY" "http://127.0.0.1:$PORT/answer" 2>/dev/null)
if [ -z "$ANSWER" ]; then
    bad "a POST with a machine's hardware got nothing back"
else
    echo "$ANSWER" | sed 's/^/    | /'
    case "$ANSWER" in
    *rig-machine.example.com*) ok "the machine's own file was chosen over default.toml" ;;
    *) bad "the answer is not this machine's — selection did not work" ;;
    esac
    case "$ANSWER" in
    *'keyboard = "fr"'* | *"keyboard = 'fr'"*) ok "and the group it belongs to was merged in" ;;
    *) bad "the group's value is missing — members/merge did not work" ;;
    esac
    case "$ANSWER" in
    *should-not-be-served*) bad "default.toml leaked into a machine's answer" ;;
    *members*) bad "the control key 'members' was served to the installer" ;;
    *) ok "no default fallback and no control keys in what the installer receives" ;;
    esac
fi

# The other half of the protocol: everything that is not Proxmox GETs, with the machine's
# identity in the query string, because iPXE substitutes it into the URL.
GET=$(curl -fsS -m 20 "http://127.0.0.1:$PORT/answer?mac=98-fa-9b-50-d8-10" 2>/dev/null)
case "$GET" in
*rig-machine.example.com*) ok "a GET with ?mac= gets the same machine's answer" ;;
*) bad "the query-string route did not resolve the machine" ;;
esac

# An identity nobody claims must fall back to default.toml, not to nothing.
UNKNOWN=$(curl -fsS -m 20 "http://127.0.0.1:$PORT/answer?mac=00-00-00-00-00-01" 2>/dev/null)
case "$UNKNOWN" in
*should-not-be-served*) ok "an unknown machine falls back to default.toml" ;;
*) bad "an unknown machine got no default" ;;
esac

section "what DSM generated for us (open questions, answered by looking)"
run systemctl cat pkgctl-$PKG
note "Restart= above decides whether DSM restarts the process if it dies."
run sh -c "$ROOT/scripts/start-stop-status status; echo \"  exit=\$?\""

# ── the desktop application ────────────────────────────────────────────────────
# Only a machine can answer these: whether DSM made the symlink, whether it serves the
# files, and — the one that matters — whether the backend's door is actually shut. That
# path is **not** authenticated by DSM, measured here on 7.2.2, so an open CGI would be an
# unauthenticated root-adjacent configuration editor on the network.
section "the desktop application"

LINK=/usr/syno/synoman/webman/3rdparty/rescriptum
if [ -L "$LINK" ] && [ -d "$LINK" ]; then
    ok "DSM linked $LINK to the package's ui/"
else
    bad "no $LINK — DSM did not pick up dsmuidir"
fi

JSFILE=$(sed -n 's/^[[:space:]]*"\([^"]*\.js\)"[[:space:]]*:.*/\1/p' "$LINK/config" 2>/dev/null | head -n 1)
if [ -n "$JSFILE" ] && [ -f "$LINK/$JSFILE" ]; then
    ok "the application is $JSFILE"
else
    bad "ui/config names no JavaScript that is there"
fi

# The CGI runs as the *owner of the script*, which for a package tree is the package user —
# the same identity that owns the 0600 env file. If DSM ever changes that, everything the
# application does stops working, and this is where it would show.
if [ "$(stat -c '%U' "$LINK/api.cgi" 2>/dev/null)" = "$PKG" ]; then
    ok "the backend is owned by $PKG, so it runs as $PKG"
else
    bad "api.cgi is owned by $(stat -c '%U' "$LINK/api.cgi" 2>/dev/null), not $PKG"
fi

# **The door.** No session, no answer.
for path in "api.cgi?action=config" "api.cgi?action=status"; do
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 "http://127.0.0.1:5000/webman/3rdparty/rescriptum/$path" 2>/dev/null)
    if [ "$code" = "403" ]; then
        ok "unauthenticated $path is refused ($code)"
    else
        bad "unauthenticated $path answered $code — that is an open configuration editor"
    fi
done

# And the JavaScript itself is public by design; it must carry no secret and no placeholder.
if grep -q '@VERSION@\|@JSFILE@' "$LINK/$JSFILE" "$LINK/config" 2>/dev/null; then
    bad "the application still has a build placeholder in it"
else
    ok "no build placeholders left in the application"
fi

# ── 4. the CLI, as the package user ────────────────────────────────────────────
section "the CLI on PATH"
echo 'global.keyboard = "fr"' >"$SHARE/answers/default.toml"
chown "$PKG" "$SHARE/answers/default.toml" 2>/dev/null
run sudo -u "$PKG" /usr/local/bin/$PKG-cli check
# Run as root it succeeds whatever the ACL says, which is what makes the sudo -u form the
# real test.
if sudo -u "$PKG" /usr/local/bin/$PKG-cli check >/dev/null 2>&1; then
    ok "sudo -u $PKG rescriptum-cli check passes — the permissions are real"
else
    bad "the package user cannot check its own answers"
fi

# ── 5. logrotate, and the descriptor that must not move ────────────────────────
section "logrotate"
STANZA=$(find /usr/local/etc/logrotate.d /etc/logrotate.d /usr/syno/etc/logrotate.d -name "*$PKG*" 2>/dev/null | head -n 1)
if [ -z "$STANZA" ]; then
    bad "no logrotate stanza installed — syslog-config did not take"
else
    ok "installed at $STANZA"
    curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1
    before=$(stat -c '%i' "$ROOT/var/$PKG.log" 2>/dev/null)
    run logrotate -v -f "$STANZA"
    after=$(stat -c '%i' "$ROOT/var/$PKG.log" 2>/dev/null)
    # `find` without -L stops at /var/packages/<pkg>/var, which DSM makes a symlink — the
    # rotated file is there, and this reported "nothing was rotated" for two runs.
    # DSM's logrotate compresses with xz, so do not look for .gz either.
    if ls "$ROOT/var/$PKG".log.* >/dev/null 2>&1; then
        ok "the log was rotated ($(ls "$ROOT/var/$PKG".log.* | head -n 1 | xargs basename))"
    else
        bad "nothing was rotated"
    fi
    # The assertion that matters: the first would pass even under a broken configuration.
    if [ "$before" = "$after" ] && [ -n "$after" ]; then
        ok "the file kept its inode, so the server's open descriptor still points at it"
    else
        bad "the inode changed ($before → $after) — the server is now writing to a file with no name"
    fi
    curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1
    sleep 1
    grep -q health "$ROOT/var/$PKG.log" 2>/dev/null && ok "and new requests still land in it" || bad "requests stopped reaching the log after a rotation"
fi

# ── 6. the upgrade, adversarially ──────────────────────────────────────────────
section "an upgrade must not destroy configuration or answers"
# Create the canary rather than assume start already made the directory: if the package
# failed to start, asserting that a file we never wrote "did not survive" reports a
# destroyed answer set where there was none. A test that fails for the wrong reason is
# worse than one that does not run.
mkdir -p "$SHARE/answers" 2>/dev/null
# **Not canary.toml.** A .toml file holding "do not delete me" is a *candidate answer*, and
# `rescriptum-cli check` rightly fails on it — which poisoned the next run, since the share
# is deliberately never cleaned. `txt` is not on the format allowlist, so this file can
# never be mistaken for an answer, which is exactly the property being relied on.
if echo "do not delete me" >"$SHARE/answers/canary.txt" 2>/dev/null; then
    CANARY=yes
else
    CANARY=no
    bad "could not write a canary into $SHARE/answers — the share is not usable"
fi
if [ -f "$ROOT/etc/$PKG.env" ]; then
    # Once, not once per run: a duplicate key is a startup error, by design.
    grep -q '^RESCRIPTUM_ANSWER_TOKEN=' "$ROOT/etc/$PKG.env" ||
        printf 'RESCRIPTUM_ANSWER_TOKEN=a-token-nobody-should-lose\n' >>"$ROOT/etc/$PKG.env"
    cp "$ROOT/etc/$PKG.env" "$RIG/env.before"
fi

if [ -f "$SPK2" ]; then
    run synopkg install "$SPK2"
    sleep 3
    if [ ! -f "$RIG/env.before" ]; then
        bad "there was no env file to carry through the upgrade"
    elif diff -q "$RIG/env.before" "$ROOT/etc/$PKG.env" >/dev/null 2>&1; then
        ok "the hand-edited env file came through the upgrade untouched"
    else
        bad "the upgrade rewrote the user's configuration:"
        diff "$RIG/env.before" "$ROOT/etc/$PKG.env" | sed 's/^/    /'
    fi
    if [ "$CANARY" = yes ]; then
        [ -f "$SHARE/answers/canary.txt" ] && ok "the canary in the share survived the upgrade" || bad "the upgrade destroyed a file in the share"
    fi
    note "installed version is now $(synopkg version "$PKG" 2>/dev/null)"
    note "etc/ and var/ live in /volume1/@appconf and /volume1/@appdata and outlive both"
    note "an upgrade and an uninstall — the env file, tokens included, stays on the volume"
    if ! curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
        bad "the service does not answer after the upgrade"
        tail -n 8 "$ROOT/var/startup.log" 2>/dev/null | sed 's/^/    /'
        tail -n 8 "$ROOT/var/$PKG.log" 2>/dev/null | sed 's/^/    /'
    else
        ok "and the service came back up on its own"
    fi
else
    note "no build2.spk — skipping the upgrade (this is the most valuable test here)"
fi

# ── 7. uninstall ───────────────────────────────────────────────────────────────
section "uninstall must leave the answers alone"
KEEP=$(readlink -f "$SHARE" 2>/dev/null)
run synopkg uninstall "$PKG"
[ -d "$ROOT/target" ] && bad "the package tree is still there" || ok "the package was removed"
if [ "$CANARY" != yes ]; then
    note "no canary was ever written, so the uninstall proves nothing about the answers"
elif [ -n "$KEEP" ] && [ -f "$KEEP/answers/canary.txt" ]; then
    ok "the shared folder and the canary in it are untouched: $KEEP"
else
    bad "the answers did not survive the uninstall — this is the worst outcome available here"
fi
note "the '$PKG' shared folder is left behind on purpose; remove it by hand when you are done"

echo
if [ "$fails" -gt 0 ]; then
    echo "$pass passed, $fails failed"
    exit 1
fi
echo "$pass checks passed"
