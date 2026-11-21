#!/usr/bin/env bash
# The captured help forms (crates/k5-cli/src/help_captured.txt: what bash
# printed for every help form of the router that tests/cli/help_native.sh
# proves bash only prints, the same everywhere) print the same from the
# CLI under test as from this checkout's bash: stdout, stderr and the exit.
#
# When the CLI under test is the k5 binary (K5_CLI_UNDER_TEST set), its
# hand-over to bash is broken (K5_HOME_OVERRIDE set to a relative path,
# which bash never reads and the binary refuses before it runs bash), so a
# pass is the binary's own answer:
#   - every form with one page, in the owner's terminal (a port and an owner token on
#     disk);
#   - every fifth form in an agent's terminal (a passport, K2_PORT and
#     K2_HOOK_SOCK a live socket);
#   - every tenth form with K2_HOOK_SOCK set and no passport: an id tool's
#     help is refused (exit 3), an open one's printed, by both;
#   - a form with two pages (the first of each pair of pages and every
#     tenth after it): one the permission cards answer decides (a C row:
#     its page goes through the cards filter) in an agent's terminal whose
#     daemon says cards are on, and one whose daemon says off: a stub daemon on
#     two sockets, curl real for a request to a Unix socket; each side
#     asks it once, the probe (GET /cli/permission/enabled), with its own
#     passport;
#     one that prints chat's usage (an M row) with
#     _CHAT_HELP_MANAGE=0 in the owner's terminal, and =yes in an agent's;
#   - a zen form (a Z row: the Garden changes hint, then the page) as the
#     owner with no project path, and every tenth with a workspace as its
#     project path (the hint's first run prints its line and writes the
#     marker, the second prints none) and with the home as it (no hint).
# (crates/k5-cli/tests/help_captured_parity.rs runs the binary on every
# form in both terminals against the list; help_native.sh proves the list
# is bash's.) With no port and no token the binary hands a form to bash
# (the connection gate): a few such forms must fail with the hand-over
# broken. On bash alone there is nothing to compare: it says so and passes.
#
# Each run gets a scratch HOME and no K2_/K5_ settings from the caller
# (env -i); curl, launchctl, systemctl, sudo, security, open, osascript,
# ssh and k2 itself are stubs that log and fail: a run that calls one
# fails the test (but curl to the stub daemon's sockets, for the cards
# rows). Never touches ~/.k2.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
NEW="$CLI_UNDER_TEST"
LIST="$ROOT/crates/k5-cli/src/help_captured.txt"
[ -f "$LIST" ] || { echo "FAIL: no $LIST" >&2; exit 1; }

if [ "$NEW" = "$CLI_BASH_SOURCE" ]; then
    echo "help_captured: the CLI under test is bash: nothing to compare (run it with K5_CLI_UNDER_TEST)"
    exit 0
fi

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

SANDBOX="$(mktemp -d -t k2-help-captured-XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT
mkdir -p "$SANDBOX/home/.k2" "$SANDBOX/empty-home" "$SANDBOX/cwd" "$SANDBOX/out" "$SANDBOX/stub"
printf '1\n' > "$SANDBOX/home/.k2/heartbeat.port"
printf 'test-token\n' > "$SANDBOX/home/.k2/heartbeat.token"
for t in curl wget nc launchctl systemctl sudo security open osascript pkill killall ssh k2 k5; do
    printf '#!/bin/sh\necho "%s $*" >> "%s/stub.log"\nexit 97\n' "$t" "$SANDBOX" > "$SANDBOX/stub/$t"
    chmod +x "$SANDBOX/stub/$t"
done
python3 -c 'import socket, sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "$SANDBOX/live.sock"
[ -S "$SANDBOX/live.sock" ] || { echo "FAIL: no socket at $SANDBOX/live.sock" >&2; exit 1; }
BROKEN="K5_HOME_OVERRIDE=not/absolute"

# For the cards rows: curl runs for real only for a request to a Unix
# socket (the stub daemon's), and the stub answers the cards probe, on on
# on.sock and off on off.sock, logging each request's route and passport.
mkdir -p "$SANDBOX/stub-cell"
cp "$SANDBOX"/stub/* "$SANDBOX/stub-cell/"
REAL_CURL="$(command -v curl)" || { echo "FAIL: no curl on PATH" >&2; exit 1; }
printf '#!/bin/sh\nfor a in "$@"; do [ "$a" = --unix-socket ] && exec %s "$@"; done\necho "curl $*" >> "%s/stub.log"\nexit 97\n' \
    "$REAL_CURL" "$SANDBOX" > "$SANDBOX/stub-cell/curl"
chmod +x "$SANDBOX/stub-cell/curl"
python3 -I - "$SANDBOX" <<'PY' &
import http.server, json, os, socketserver, sys, threading
base = sys.argv[1]
lock = threading.Lock()
class Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
def handler(on):
    class Cards(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        def do_GET(self):
            with lock, open(os.path.join(base, "asked.log"), "a") as f:
                f.write("%s %s\n" % (self.path.split("?", 1)[0], self.headers.get("Authorization", "")))
            body = json.dumps({"enabled": on}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.close_connection = True
        do_POST = do_GET
        def address_string(self):
            return "cell"
        def log_message(self, *a):
            pass
    return Cards
for name, on in (("on", True), ("off", False)):
    srv = Server(os.path.join(base, name + ".sock"), handler(on))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
open(os.path.join(base, "stub.ready"), "w").close()
threading.Event().wait()
PY
STUB_PID=$!
trap 'kill "$STUB_PID" 2>/dev/null; rm -rf "$SANDBOX"' EXIT
for _ in $(seq 1 100); do [ -e "$SANDBOX/stub.ready" ] && break; sleep 0.1; done
[ -e "$SANDBOX/stub.ready" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
STUBS="$SANDBOX/stub"

# run <prog> <out file> <settings words> -- <args...>: stdout to the file,
# stderr to <out>.err, the exit code to <out>.code. A stub call fails.
run() {
    local prog="$1" out="$2" code=0; shift 2
    local -a set=()
    while [ "$1" != "--" ]; do set+=("$1"); shift; done
    shift
    : > "$SANDBOX/stub.log"
    (cd "$SANDBOX/cwd" && env -i HOME="$SANDBOX/home" PATH="$STUBS:$PATH" LC_ALL=C \
        ${set[@]+"${set[@]}"} "$prog" "$@") > "$out" 2> "$out.err" < /dev/null || code=$?
    printf '%s\n' "$code" > "$out.code"
    if [ -s "$SANDBOX/stub.log" ]; then
        bad "k2 $* ran $(tr '\n' ';' < "$SANDBOX/stub.log")"
    fi
}

# native <settings words> -- <args...>: the binary with its hand-over
# broken prints what bash prints, both with the settings.
native() {
    local -a set=()
    while [ "$1" != "--" ]; do set+=("$1"); shift; done
    shift
    run "$CLI_BASH_SOURCE" "$SANDBOX/out/bash" ${set[@]+"${set[@]}"} -- "$@"
    run "$NEW" "$SANDBOX/out/bin" ${set[@]+"${set[@]}"} "$BROKEN" -- "$@"
    if cmp -s "$SANDBOX/out/bash" "$SANDBOX/out/bin" && cmp -s "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" \
        && cmp -s "$SANDBOX/out/bash.code" "$SANDBOX/out/bin.code"; then
        ok
    else
        bad "k2 $* (${set[*]:-}): the binary's own answer differs from bash"
        diff "$SANDBOX/out/bash" "$SANDBOX/out/bin" | head -6 >&2 || true
        diff "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" | head -6 >&2 || true
    fi
}

# handed <settings words> -- <args...>: the binary hands it to bash, so
# with the hand-over broken it fails, saying why, with stdout empty.
handed() {
    local -a set=()
    while [ "$1" != "--" ]; do set+=("$1"); shift; done
    shift
    run "$NEW" "$SANDBOX/out/bin" ${set[@]+"${set[@]}"} "$BROKEN" -- "$@"
    if [ "$(cat "$SANDBOX/out/bin.code")" != 0 ] && [ ! -s "$SANDBOX/out/bin" ] \
        && grep -q K5_HOME_OVERRIDE "$SANDBOX/out/bin.err"; then
        ok
    else
        bad "k2 $* (${set[*]:-}) did not go to bash (exit $(cat "$SANDBOX/out/bin.code"))"
    fi
}

# cards <on|off> <args...>: native in an agent's terminal whose daemon
# says cards are <on|off>, each side with its own passport, and each side
# asked the stub once, the probe.
cards_runs=0
cards() {
    local which="$1" side passport n code=0; shift
    cards_runs=$((cards_runs + 1))
    STUBS="$SANDBOX/stub-cell"
    : > "$SANDBOX/asked.log"
    for side in bash bin; do
        passport="sess${side}.c${cards_runs}"
        if [ "$side" = bash ]; then
            run "$CLI_BASH_SOURCE" "$SANDBOX/out/bash" "K2_PORT=1" "K2_HOOK_TOKEN=$passport" \
                "K2_HOOK_SOCK=$SANDBOX/$which.sock" -- "$@"
        else
            run "$NEW" "$SANDBOX/out/bin" "K2_PORT=1" "K2_HOOK_TOKEN=$passport" \
                "K2_HOOK_SOCK=$SANDBOX/$which.sock" "$BROKEN" -- "$@"
        fi
        n="$(grep -c "^/cli/permission/enabled Bearer $passport\$" "$SANDBOX/asked.log" || true)"
        [ "$n" = 1 ] || { bad "k2 $* (cards $which): $side asked the probe $n times"; code=1; }
    done
    STUBS="$SANDBOX/stub"
    if [ "$(($(wc -l < "$SANDBOX/asked.log")))" -ne 2 ]; then
        bad "k2 $* (cards $which): requests $(tr '\n' ';' < "$SANDBOX/asked.log")"
        code=1
    fi
    if cmp -s "$SANDBOX/out/bash" "$SANDBOX/out/bin" && cmp -s "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" \
        && cmp -s "$SANDBOX/out/bash.code" "$SANDBOX/out/bin.code"; then
        if [ "$code" = 0 ]; then ok; fi
    else
        bad "k2 $* (cards $which): the binary's own answer differs from bash"
        diff "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" | head -6 >&2 || true
    fi
}

# zen_hint <args...>: a Z row (a zen form: the router arm's Garden changes
# hint, then the page) in an owner's terminal whose project path is a
# workspace with a .k2 folder, each side in a workspace of its own. The
# first run prints the hint line on stderr and writes the marker
# (0.46.3); the second prints no line and leaves the marker as it is. Both
# sides the same each time, the markers the same bytes.
HINT_LINE='Gardens changed in 0.46.3: see `k2 study garden-changes` (update before 0.47.0)'
zen_hint() {
    local side round
    for side in bash bin; do
        rm -rf "$SANDBOX/ws-$side"
        mkdir -p "$SANDBOX/ws-$side/.k2"
    done
    for round in first second; do
        run "$CLI_BASH_SOURCE" "$SANDBOX/out/bash" "K2_PROJECT_PATH=$SANDBOX/ws-bash" -- "$@"
        run "$NEW" "$SANDBOX/out/bin" "K2_PROJECT_PATH=$SANDBOX/ws-bin" "$BROKEN" -- "$@"
        if ! { cmp -s "$SANDBOX/out/bash" "$SANDBOX/out/bin" && cmp -s "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" \
            && cmp -s "$SANDBOX/out/bash.code" "$SANDBOX/out/bin.code"; }; then
            bad "k2 $* (the zen hint, $round run): the binary's own answer differs from bash"
            diff "$SANDBOX/out/bash.err" "$SANDBOX/out/bin.err" | head -6 >&2 || true
            continue
        fi
        if [ "$round" = first ] && [ "$(head -n 1 "$SANDBOX/out/bin.err")" != "$HINT_LINE" ]; then
            bad "k2 $* (the zen hint, first run): no hint line first on stderr"
            continue
        fi
        if [ "$round" = second ] && grep -qF 'Gardens changed' "$SANDBOX/out/bin.err"; then
            bad "k2 $* (the zen hint, second run): the hint line again"
            continue
        fi
        if [ "$(cat "$SANDBOX/ws-bin/.k2/.garden-changes-hint" 2>&1)" != 0.46.3 ] \
            || ! cmp -s "$SANDBOX/ws-bash/.k2/.garden-changes-hint" "$SANDBOX/ws-bin/.k2/.garden-changes-hint"; then
            bad "k2 $* (the zen hint, $round run): the markers differ or are not 0.46.3"
            continue
        fi
        ok
    done
}

AGENT=("K2_PORT=1" "K2_HOOK_TOKEN=sessT.notreal1" "K2_HOOK_SOCK=$SANDBOX/live.sock")
SOCK="K2_HOOK_SOCK=$SANDBOX/no.sock"
EMPTY="HOME=$SANDBOX/empty-home"
n=0
nc=0
nm=0
nz=0
ran_pairs=0
seen_pairs=" "
while IFS=$'\t' read -r kind argv p1 p2; do
    case "$kind" in F|C|M|Z) ;; *) continue ;; esac
    n=$((n + 1))
    if [ "$kind" = Z ]; then
        # A zen form: the page with no project path (no hint), and every
        # tenth with the hint's first and second run, and with the project
        # path the home itself (no hint, no marker).
        nz=$((nz + 1))
        # shellcheck disable=SC2086
        native -- $argv
        if [ $(((nz - 1) % 10)) = 0 ]; then
            # shellcheck disable=SC2086
            zen_hint $argv
            # shellcheck disable=SC2086
            native "K2_PROJECT_PATH=$SANDBOX/home" -- $argv
            if [ -e "$SANDBOX/home/.k2/.garden-changes-hint" ]; then
                bad "k2 $argv: a hint marker in the home"
                rm -f "$SANDBOX/home/.k2/.garden-changes-hint"
            fi
        fi
    elif [ "$kind" = F ]; then
        # shellcheck disable=SC2086  # argv is words
        native -- $argv
        if [ $((n % 5)) = 0 ]; then
            # shellcheck disable=SC2086
            native "${AGENT[@]}" -- $argv
        fi
    else
        # A form with two pages: the first form of each pair of pages and
        # every tenth after it, each way its switch goes.
        [ "$kind" = C ] && nc=$((nc + 1))
        [ "$kind" = M ] && nm=$((nm + 1))
        pair="$kind:$p1:$p2"
        case "$seen_pairs" in
            *" $pair "*) [ $(((nc + nm) % 10)) = 0 ] || continue ;;
            *) seen_pairs="$seen_pairs$pair " ;;
        esac
        ran_pairs=$((ran_pairs + 1))
        # shellcheck disable=SC2086
        native -- $argv
        if [ "$kind" = C ]; then
            # shellcheck disable=SC2086
            cards on $argv
            # shellcheck disable=SC2086
            cards off $argv
        else
            # shellcheck disable=SC2086
            native "_CHAT_HELP_MANAGE=0" -- $argv
            # shellcheck disable=SC2086
            native "${AGENT[@]}" "_CHAT_HELP_MANAGE=yes" -- $argv
        fi
    fi
    if [ $((n % 10)) = 0 ]; then
        # shellcheck disable=SC2086
        native "$SOCK" -- $argv
    fi
    if [ $((n % 200)) = 0 ]; then
        # shellcheck disable=SC2086
        handed "$EMPTY" -- $argv
    fi
done < "$LIST"
[ "$n" -gt 1000 ] || bad "only $n forms in $LIST"
[ "$nc" -gt 100 ] || bad "only $nc forms the cards answer decides in $LIST"
[ "$nm" -gt 20 ] || bad "only $nm forms that print chat's usage in $LIST"
[ "$ran_pairs" -gt 30 ] || bad "only $ran_pairs forms with two pages run"
[ "$nz" -gt 100 ] || bad "only $nz zen forms (the Garden changes hint) in $LIST"
for cmd in "agent team help x" "app nope --help" "zen nope --help" "agent team --help x"; do
    # shellcheck disable=SC2086
    handed -- $cmd
done

echo "help_captured: $pass passed, $fail failed ($n forms: $nc the cards answer decides, $nm print chat's usage; $ran_pairs of those run each way; $nz zen forms after the hint)"
[ "$fail" = 0 ]
