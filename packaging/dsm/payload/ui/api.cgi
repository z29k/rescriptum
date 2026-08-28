#!/bin/sh
# The DSM application's backend.
#
# DSM links this package's `ui` directory into /usr/syno/synoman/webman/3rdparty/rescriptum,
# so this script is served by DSM's own web server, on DSM's own port, under DSM's own
# certificate. That is what makes the application possible at all: the desktop is normally
# HTTPS, and a page there cannot call a plain-HTTP port — mixed content is a hard block, not
# a warning. Same origin removes that, and CORS, and any second password.
#
# **Two facts about this path were measured on a DSM 7.2.2 machine, and both are
# load-bearing. Neither is in the developer guide.**
#
#   1. A CGI here runs as **the owner of the script**. The package tree is chowned to the
#      package user, so this runs as `rescriptum` — the same identity that owns the 0600 env
#      file and the log, which is the whole reason the configuration can be read and written
#      while the server itself is stopped. It is *not* root: a script left owned by root
#      does run as root here, which is worth knowing and worth never doing.
#   2. This path is **not authenticated by DSM**. An unauthenticated request reaches this
#      script and gets 200. DSM protects its own pages, not ours.
#
# Put together: the only thing standing between the open network and this machine's
# provisioning configuration is the check in `authorize` below. It is not a formality, it is
# the door. authenticate.cgi prints the logged-in user's name and prints nothing at all when
# there is no session; `administrators` membership is then checked separately, because
# "logged in" is not "may set the root password of every machine this NAS installs".
#
# Nothing here can start or stop the package — it has no privilege to. Restarting is the
# application's job, through DSM's own SYNO.Core.Package.Control, with the administrator's
# own session: DSM does its own bookkeeping that way, and a process this script started
# would land outside the package's cgroup where DSM could no longer stop it.
#
# The third guard is CSRF. A browser will not send a cross-site request carrying a header it
# invented without a preflight first, and this script answers no preflight — so requiring
# X-Rescriptum on writes means a page on another origin cannot make the browser do this on
# an administrator's behalf. DSM's own SynoToken is honoured too when the desktop supplies
# it, which is what makes the app keep working with DSM's CSRF protection switched on.

set -u

PKG="rescriptum"
# The same seams the lifecycle scripts use, and safe for the same reason they are: a CGI's
# environment is the web server's, and everything a *client* can influence arrives named
# HTTP_* — there is no request that can set RESCRIPTUM_PKG_ROOT. They exist so
# lifecycle-test.sh can drive this script against a tree it is allowed to write to, with a
# stub for DSM's authenticator and a group the test user is actually in. Every default
# below is the real one.
ROOT="${RESCRIPTUM_PKG_ROOT:-/var/packages/$PKG}"
CLI="$ROOT/target/bin/$PKG-cli"
VAR="$ROOT/var"
AUTH_CGI="${RESCRIPTUM_AUTH_CGI:-/usr/syno/synoman/webman/modules/authenticate.cgi}"
ADMIN_GROUP="${RESCRIPTUM_ADMIN_GROUP:-administrators}"

# The log can be long and this is a browser, not a terminal.
MAX_LINES=2000
DEFAULT_LINES=200

reply() { # <status> <content-type>
    echo "Status: $1"
    echo "Content-Type: $2"
    # This is configuration, and a stale copy of it is a misleading one.
    echo "Cache-Control: no-store"
    echo "X-Content-Type-Options: nosniff"
    echo ""
}

fail() { # <status> <message>
    reply "$1" "text/plain; charset=utf-8"
    echo "$2"
    exit 0
}

# One value out of the query string, by name, without ever building a path or a command
# from it. Percent-decoding is deliberately not done here: every parameter this script
# accepts is matched against a fixed list of literals or checked to be digits.
param() { # <name>
    printf '%s' "${QUERY_STRING:-}" |
        tr '&' '\n' |
        sed -n "s/^$1=//p" |
        head -n 1
}

# **The door.** Nothing above this line touches the filesystem or runs a command.
authorize() {
    user=$("$AUTH_CGI" 2>/dev/null | tr -d '\r\n')
    if [ -z "$user" ]; then
        fail 403 "Sign in to DSM first."
    fi
    # A DSM account is not an administrator account. `id -nG` is in /usr/bin, which a
    # non-login PATH does have — synogroup is not.
    case " $(id -nG "$user" 2>/dev/null) " in
    *" $ADMIN_GROUP "*) ;;
    *) fail 403 "This application is for DSM administrators." ;;
    esac
}

# A write has to prove it was made by our own page rather than by a page somewhere else
# that happens to be open in the same browser.
require_write_intent() {
    [ "${REQUEST_METHOD:-GET}" = "POST" ] || fail 405 "This action is a POST."
    [ -n "${HTTP_X_RESCRIPTUM:-}" ] || fail 403 "Missing X-Rescriptum."
}

# The request body, bounded. An unbounded read here would let anyone who got past the door
# hand the machine a gigabyte to hold in memory.
body() {
    len=${CONTENT_LENGTH:-0}
    case "$len" in
    '' | *[!0-9]*) return 0 ;;
    esac
    [ "$len" -gt 65536 ] && fail 413 "That is too much configuration."
    [ "$len" -eq 0 ] && return 0
    dd bs=1 count="$len" 2>/dev/null
}

authorize

case "$(param action)" in
config)
    # `config --json` already emits JSON, and emits no token in it. Passing it through
    # rather than rebuilding it here means the panel and the command line can never
    # disagree about what is configured.
    out=$("$CLI" config --json 2>/dev/null)
    if [ -z "$out" ]; then
        fail 500 "rescriptum-cli produced nothing — is the package installed?"
    fi
    reply 200 "application/json; charset=utf-8"
    echo "$out"
    ;;

save)
    require_write_intent
    # KEY=VALUE per line. Each line is passed to the CLI **as a single argument**, so a
    # value containing a space, a semicolon or a quote is data and never a command. The
    # CLI is what decides whether the key is one this program reads and whether the value
    # can be represented — those rules live in Rust, where they are tested, rather than
    # being written a second time here in shell.
    set --
    while IFS= read -r line; do
        case "$line" in
        '' | '#'*) continue ;;
        *=*) set -- "$@" "$line" ;;
        *) fail 400 "Expected KEY=VALUE, got: $line" ;;
        esac
    done <<BODY
$(body)
BODY

    [ "$#" -eq 0 ] && fail 400 "Nothing to save."

    if ! out=$("$CLI" config set "$@" 2>&1); then
        # The CLI refuses a write that would leave a server unable to start, and says why.
        # That sentence is the most useful thing the panel can show, so it is passed on.
        reply 409 "text/plain; charset=utf-8"
        echo "$out"
        exit 0
    fi
    out=$("$CLI" config --json 2>/dev/null)
    reply 200 "application/json; charset=utf-8"
    echo "$out"
    ;;

status)
    # `key: value` lines rather than JSON: everything here is either a literal this script
    # chose or a path, and hand-rolling JSON escaping in shell for the sake of a shape is
    # how a stray quote becomes a panel that renders nothing.
    reply 200 "text/plain; charset=utf-8"
    echo "version: $("$CLI" --version 2>/dev/null || echo unknown)"

    # **Not `synopkg status`.** It answers with a page of JSON — and on a machine where the
    # service is plainly running it still said "package is stopped, failed to get unit
    # status". The package's own start-stop-status is both truthful and a contract: 0 is
    # running, 3 is stopped, 1 means it died and left its pidfile behind.
    sh "$ROOT/scripts/start-stop-status" status >/dev/null 2>&1
    case $? in
    0) echo "package: running" ;;
    3) echo "package: stopped" ;;
    1) echo "package: crashed" ;;
    *) echo "package: unknown" ;;
    esac

    # Asked of the filesystem, as the user the service actually runs as. Root can read
    # anything, so checking as root would answer a question nobody asked — and the
    # permissions of the answers directory are the first thing a packaged install gets
    # wrong.
    answers=$("$CLI" config --value RESCRIPTUM_ANSWERS_DIR 2>/dev/null)
    echo "answers: ${answers:-unknown}"
    # No `su` here, and none needed: this script already *is* the user the service runs
    # as, so a plain test answers exactly the question that matters. The first version did
    # try to su, which cost an afternoon — it hung the request outright (su read the CGI's
    # stdin and waited on it forever) and then, once that was fixed, failed with
    # "Permission denied", because a non-root process cannot become anybody.
    if [ -n "$answers" ] && [ -r "$answers" ] && [ -x "$answers" ]; then
        echo "answers_readable: yes"
    else
        echo "answers_readable: no"
    fi

    # **The one state this panel has to surface that nothing else does.** A TFTP port
    # that cannot be bound deliberately does not stop the server — port 69 needs a
    # `setcap` that an upgrade silently drops, and answers must not go down to report
    # that — so the only trace an operator would otherwise have is a startup warning that
    # scrolled past hours ago. `boot check` asks the port for a real loader rather than
    # trying to bind it, because binding proves the opposite of what it looks like: a
    # bind that succeeds means nothing is listening.
    case "$("$CLI" boot check 2>/dev/null)" in
    *"handed over"*) echo "tftp: serving" ;;
    *"BROKEN"*) echo "tftp: broken" ;;
    *"TFTP is off"*) echo "tftp: off" ;;
    *"nothing is listening"*) echo "tftp: silent" ;;
    *) echo "tftp: unknown" ;;
    esac
    ;;

media)
    # Everything the `media` tab reads, as the CLI's own text. Passing it through rather
    # than reformatting means the panel and the command line can never disagree about
    # what is held — the same reason `config` passes `--json` through.
    reply 200 "text/plain; charset=utf-8"
    "$CLI" media list 2>&1
    ;;

sources)
    # With no `source` parameter this lists the catalogues, which is local and instant.
    # With one it fetches that vendor's index over the network, which is not: the browser
    # is told to expect a wait, and a failure here is the vendor's or the uplink's rather
    # than ours.
    which=$(param source)
    reply 200 "text/plain; charset=utf-8"
    if [ -n "$which" ]; then
        "$CLI" media sources "$which" 2>&1
    else
        "$CLI" media sources 2>&1
    fi
    ;;

fetch)
    require_write_intent
    # **A CGI cannot hold a request open for 1.5 GB**, so this starts the download and
    # returns immediately; `progress` below is how the page follows it. Three details are
    # each a trap already paid for elsewhere in this package:
    #
    #   * `</dev/null` — a background child that inherits the CGI's stdin keeps the
    #     request open forever, which is exactly how `su` hung this script once.
    #   * `setsid` — without a session of its own the download is killed when the web
    #     server reaps the CGI, and a 1.5 GB transfer would die a second in.
    #   * the name is passed as a single argument, so it is data and never a command.
    #
    # Two ways in, and the manual one is not a lesser path: `--from` reads the digest from
    # the vendor's index, and a URL with a digest somebody obtained out of band is the
    # stronger of the two. Both end in the same `media add`, which is where the rules
    # about digests live.
    #
    # Every value below reaches the CLI as **one argument**, so a URL containing a
    # semicolon or a quote is data and never a command. The whitelists are about giving a
    # wrong value a clear answer here rather than a puzzle from curl later.
    if [ -f "$VAR/fetch.pid" ] && kill -0 "$(cat "$VAR/fetch.pid" 2>/dev/null)" 2>/dev/null; then
        fail 409 "A download is already running."
    fi
    src=$(param source)
    name=$(param name)
    url=$(param url)
    digest=$(param sha256)
    : >"$VAR/fetch.log"
    if [ -n "$url" ]; then
        case "$url" in
        https://* | http://*) ;;
        *) fail 400 "That is not an http or https URL." ;;
        esac
        # 64 hex characters, checked here so an obvious typo is answered now rather than
        # after a gigabyte. The CLI checks it again, which is where it counts.
        case "$digest" in
        '') fail 400 "A URL needs its SHA-256 — that decides what every machine installs." ;;
        *[!0-9a-fA-F]*) fail 400 "That is not a SHA-256." ;;
        esac
        [ "${#digest}" -eq 64 ] || fail 400 "A SHA-256 is 64 hexadecimal characters."
        setsid "$CLI" media add "$url" --sha256 "$digest" >"$VAR/fetch.log" 2>&1 </dev/null &
        echo $! >"$VAR/fetch.pid"
        reply 200 "text/plain; charset=utf-8"
        echo "started $url"
    else
        case "$src" in
        '' | *[!a-zA-Z0-9._-]*) fail 400 "That is not a source." ;;
        esac
        case "$name" in
        '' | *[!a-zA-Z0-9._-]*) fail 400 "That is not an image name." ;;
        esac
        setsid "$CLI" media add --from "$src" "$name" >"$VAR/fetch.log" 2>&1 </dev/null &
        echo $! >"$VAR/fetch.pid"
        reply 200 "text/plain; charset=utf-8"
        echo "started $name"
    fi
    ;;

progress)
    # **Progress without a progress protocol.** `media add` writes into `<name>.part` and
    # renames it atomically when the digest checks out, so the partial file's size *is*
    # the progress and its disappearance *is* the completion. Nothing had to be invented
    # for the browser, and nothing can disagree with what the CLI actually did.
    reply 200 "text/plain; charset=utf-8"
    pid=$(cat "$VAR/fetch.pid" 2>/dev/null)
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        echo "state: running"
    else
        echo "state: idle"
    fi
    dir=$("$CLI" config --value RESCRIPTUM_MEDIA_DIR 2>/dev/null)
    if [ -n "$dir" ]; then
        for part in "$dir"/*.part; do
            [ -f "$part" ] || continue
            echo "partial: $(basename "$part" .part) $(wc -c <"$part" | tr -d " ")"
        done
    fi
    # The tail rather than the whole thing: curl writes a progress bar, and the last of it
    # is the part that says what went wrong.
    echo "--- log"
    tail -c 2000 "$VAR/fetch.log" 2>/dev/null | tr "\r" "\n" | grep -v "^ *$" | tail -6
    ;;

prepare)
    require_write_intent
    # Proxmox only, and the CLI is what refuses the rest — with the reason, which the
    # panel shows verbatim. Duplicating that rule here would be a second implementation
    # to keep honest.
    id=$(param id)
    case "$id" in
    '' | *[!a-zA-Z0-9._:-]*) fail 400 "That is not an image id." ;;
    esac
    out=$("$CLI" media prepare "$id" 2>&1)
    code=$?
    reply 200 "text/plain; charset=utf-8"
    echo "$out"
    echo "--- exit $code"
    ;;

check)
    # The same command the documentation tells people to run, and the same exit code.
    reply 200 "text/plain; charset=utf-8"
    out=$("$CLI" check 2>&1)
    code=$?
    echo "$out"
    echo "--- exit $code"
    ;;

log)
    lines=$(param lines)
    case "$lines" in
    '' | *[!0-9]*) lines=$DEFAULT_LINES ;;
    esac
    [ "$lines" -gt "$MAX_LINES" ] && lines=$MAX_LINES

    # Two files, because which one holds the answer depends on how far the server got:
    # anything said before it knows where its log lives goes to startup.log.
    reply 200 "text/plain; charset=utf-8"
    for f in "$VAR/$PKG.log" "$VAR/startup.log"; do
        [ -f "$f" ] || continue
        echo "=== $f"
        tail -n "$lines" "$f" 2>/dev/null
        echo ""
    done
    ;;

*)
    fail 400 "Unknown action."
    ;;
esac
