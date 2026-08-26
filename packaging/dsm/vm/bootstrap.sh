#!/usr/bin/env bash
# Make a freshly installed DSM machine drivable by the rig: SSH on, a key installed, and
# sudo that does not stop to ask.
#
#   packaging/dsm/vm/bootstrap.sh --user rigadmin --password '…'
#   packaging/dsm/vm/bootstrap.sh --host 192.168.1.50 --web-port 5000 --ssh-port 22 \
#       --user admin --password '…' --key ~/.ssh/rescriptum-rig
#
# **The rig's account must not be named after the package user.** `conf/privilege` makes DSM
# create a `rescriptum` user at install time; a DSM administrator of the same name is
# shadowed by it and then *deleted with it* on uninstall — so the rig loses its own access
# halfway through the first run, with every login refused and nothing saying why. Use a name
# nothing else claims: `rigadmin` here.
#
# **The one thing it cannot do is create the account.** DSM's first-run wizard has no API
# and the image exposes no variable for it, so somebody opens http://<host>:5000 once and
# fills in three fields. Everything after that is here, because everything after that turned
# out to be scriptable through DSM's own web API — which is worth knowing, since the manual
# route through Control Panel is a dozen clicks on a machine that renders slowly.
#
# What it does, and why each one is needed:
#
#   * **enables SSH** (SYNO.Core.Terminal) — the rig drives the machine over it;
#   * **enables user home directories** (SYNO.Core.User.Home, which needs `location`, not
#     just `enable`) — without them there is no ~/.ssh to put a key in, and ssh-copy-id
#     fails with "Could not chdir to home directory";
#   * **installs the public key**, through expect, since DSM has no other way to accept one;
#   * **grants passwordless sudo** — remote-check.sh runs as root, and a CI runner has no
#     terminal to type a password into;
#   * **turns off Auto Block** — DSM blocks a source address after a few failed connections,
#     and a VM that reboots mid-connection produces those by the handful.
#
# It is idempotent: run it again after rebuilding the machine.

set -euo pipefail

HOST=127.0.0.1
WEB_PORT=5050
SSH_PORT=2222
USER_NAME=rigadmin
PASSWORD=""
KEY="$HOME/.ssh/rescriptum-rig"

while [ $# -gt 0 ]; do
    case "$1" in
    --host) HOST="$2"; shift 2 ;;
    --web-port) WEB_PORT="$2"; shift 2 ;;
    --ssh-port) SSH_PORT="$2"; shift 2 ;;
    --user) USER_NAME="$2"; shift 2 ;;
    --password) PASSWORD="$2"; shift 2 ;;
    --key) KEY="$2"; shift 2 ;;
    -h | --help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

if [ -z "$PASSWORD" ]; then
    printf 'password for %s on %s: ' "$USER_NAME" "$HOST" >&2
    read -rs PASSWORD
    echo >&2
fi
command -v expect >/dev/null || { echo "expect is not installed (brew install expect / apt install expect)" >&2; exit 1; }

API="http://$HOST:$WEB_PORT/webapi/entry.cgi"
COOKIES=$(mktemp "${TMPDIR:-/tmp}/dsm-cookies.XXXXXX")
trap 'rm -f "$COOKIES"' EXIT

api() { # <api> <version> <method> [extra curl --data-urlencode args...]
    local name="$1" version="$2" method="$3"
    shift 3
    curl -fsS -m 60 -b "$COOKIES" -X POST "$API" \
        --data-urlencode "api=$name" --data-urlencode "version=$version" \
        --data-urlencode "method=$method" "$@"
}

echo "==> logging in to $HOST:$WEB_PORT as $USER_NAME"
curl -fsS -m 30 -c "$COOKIES" -X POST "$API" \
    --data-urlencode 'api=SYNO.API.Auth' --data-urlencode 'version=6' \
    --data-urlencode 'method=login' --data-urlencode "account=$USER_NAME" \
    --data-urlencode "passwd=$PASSWORD" --data-urlencode 'session=Core' \
    --data-urlencode 'format=cookie' |
    grep -q '"success":true' || { echo "login refused — is the wizard finished?" >&2; exit 1; }

echo "==> enabling SSH"
api SYNO.Core.Terminal 3 set --data-urlencode 'enable_ssh=true' \
    --data-urlencode 'ssh_port=22' --data-urlencode 'enable_telnet=false' >/dev/null

echo "==> enabling user home directories"
# `enable=true` alone is refused with error 3103: it wants to be told which volume.
api SYNO.Core.User.Home 1 set --data-urlencode 'enable=true' \
    --data-urlencode 'location=/volume1' >/dev/null

if [ ! -f "$KEY" ]; then
    echo "==> creating $KEY"
    ssh-keygen -t ed25519 -N '' -C 'rescriptum-dsm-rig' -f "$KEY" >/dev/null
fi

echo "==> installing the key"
KNOWN="$(cd "$(dirname "$0")" && pwd)/known_hosts"
expect -c "
set timeout 90
spawn ssh-copy-id -i [file normalize $KEY.pub] -o StrictHostKeyChecking=accept-new \
    -o UserKnownHostsFile=$KNOWN -p $SSH_PORT $USER_NAME@$HOST
expect {
  -re \"assword:\" { send {$PASSWORD}; send \"\r\"; exp_continue }
  eof
}
" >/dev/null

SSH_ARGS=(-i "$KEY" -o StrictHostKeyChecking=accept-new -o "UserKnownHostsFile=$KNOWN" -o LogLevel=ERROR -p "$SSH_PORT")

echo "==> granting passwordless sudo"
printf '%s\n' "$PASSWORD" | ssh "${SSH_ARGS[@]}" "$USER_NAME@$HOST" \
    "sudo -S -p '' sh -c 'echo \"$USER_NAME ALL=(ALL) NOPASSWD: ALL\" > /etc/sudoers.d/rescriptum-rig && chmod 440 /etc/sudoers.d/rescriptum-rig'" >/dev/null

# A rig locks itself out otherwise, and the failure is unrecognisable: DSM's Auto Block
# counts failed connections per source address, and a VM that reboots mid-connection
# produces them by the handful. The symptom is every login refused at once — ssh keys,
# passwords and the web API alike, the last one with `"error":{"code":407}` — on an account
# that is perfectly fine. Learned by locking myself out of this exact machine.
echo "==> disabling Auto Block (a rig that locks itself out is not a rig)"
ssh "${SSH_ARGS[@]}" "$USER_NAME@$HOST" \
    'sudo -n /usr/syno/bin/synosetkeyvalue /etc/synoinfo.conf autoblock_enable no;
     sudo -n sqlite3 /etc/synoautoblock.db "delete from AutoBlockIP;" 2>/dev/null;
     true'

echo "==> checking"
ssh "${SSH_ARGS[@]}" "$USER_NAME@$HOST" 'sudo -n true' || { echo "passwordless sudo did not take" >&2; exit 1; }
ssh "${SSH_ARGS[@]}" "$USER_NAME@$HOST" 'echo "    $(cat /etc.defaults/VERSION | tr "\n" " ")"; echo "    arch: $(uname -m)"'

cat <<DONE

The rig is ready. From the repository root:

    export RIG_SSH_OPTS="-o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$KNOWN"
    ./build.sh x86_64-unknown-linux-musl
    packaging/dsm/vm/on-dsm.sh $USER_NAME@$HOST -p $SSH_PORT -i $KEY

and freeze it first, so the destructive tests cost one command to undo:

    packaging/dsm/vm/snapshot.sh save clean
DONE
