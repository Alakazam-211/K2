#!/usr/bin/env bash
# Loading the CLI and asking it for help runs no outside program it
# should not, prints nothing on stderr, and is fast with no daemon.
#
# Regression (0.46.17): a single quote inside _mw_status_print's Python
# (cli/src/mail-watch.sh) closed the single-quoted string, so the rest of
# the line became a command and EVERY run of cli/k2 ran the system `mail`
# program at load time (`mail link relink '...'`). bash -n, the build
# check and the embedded-Python check all passed.
# Also (0.46.17): `k2 app --help` printed the App page on stderr with
# nothing on stdout. It prints on stdout now, as every help page does.
#
# How: PATH starts with fail-loud stubs for programs that send, open or
# speak something (mail, sendmail, curl, ssh, open, osascript, xdg-open,
# say, notify-send) and for python3. Each stub writes its name and argv to
# a log and exits 97, with two narrow exceptions:
#   python3  passes through only for the code markers in PY_ALLOWED (the
#            help path needs none today, so every call fails);
#   curl     passes through only for a GET of one of the probe routes in
#            CURL_ALLOWED, at the daemon address the row sets (loopback).
# HOME is a scratch dir (so the K2 home is too) and the daemon address is
# 127.0.0.1 on a closed port. Then the help rows run again against a
# daemon that accepts and never answers, and one whose listen queue is
# full (a connect that never completes, or on newer macOS is reset), so
# the probes' time limits are what keeps help fast.
#
# Each command must exit as expected, print its page on stdout, print
# NOTHING on stderr, run no stubbed program and finish under MAX_MS.
# Run with: bash tests/cli/cli_load_runs_nothing.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
HELP_TEXT="$SCRIPT_DIR/../../cli/text/help"

# No session's connection or identity reaches the CLI: every K2_, K5_
# and K2SO_ name goes, then the house helper sets a scratch HOME, a dead
# port and a dummy token.
for _v in $(env | sed -nE 's/^((K2|K5|K2SO)_[A-Za-z0-9_]*)=.*/\1/p'); do
    unset "$_v"
done
unset _v
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
export K2_HOST=127.0.0.1
export XDG_CONFIG_HOME="$HOME/.config" XDG_DATA_HOME="$HOME/.local/share" \
    XDG_CACHE_HOME="$HOME/.cache" XDG_STATE_HOME="$HOME/.local/state"

# Help with no daemon answering must take less than this.
MAX_MS=3000

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-cli-load-XXXXXX)"
DAEMON_PIDS=""
cleanup() {
    local p
    for p in $DAEMON_PIDS; do
        kill "$p" 2>/dev/null || true
        wait "$p" 2>/dev/null || true
    done
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

REAL_PY="$(command -v python3)" || { echo "FAIL: python3 is not on PATH" >&2; exit 1; }
REAL_CURL="$(command -v curl)" || { echo "FAIL: curl is not on PATH" >&2; exit 1; }
STUBS="$WORK/bin"
LOG="$WORK/stub.log"
PASSED_LOG="$WORK/passed-through.log"
ALLOW_BASE="$WORK/allow-base"
mkdir -p "$STUBS"
: >"$LOG"
: >"$PASSED_LOG"

# The python3 calls the help path makes on purpose: one marker per line, a
# fixed string from the code passed to python3. None today.
PY_ALLOWED="$WORK/python3-allowed.txt"
: >"$PY_ALLOWED"

# The daemon routes help may ask, one per line. Inside a K2 cell (a token
# in the environment, as here) help asks two switches, with a time limit,
# and prints the page either way:
#   /cli/product-feedback/settings  _sf_agents_off_probe, cli/src/send-feedback.sh:
#                                   may agents report? (the daily tier's
#                                   K2 ITSELF block)
#   /cli/permission/enabled         _permission_probe, cli/src/permission.sh:
#                                   are permission cards on? (pages read
#                                   through _perm_lines, the App page among them)
CURL_ALLOWED="$WORK/curl-allowed.txt"
printf '%s\n' /cli/product-feedback/settings /cli/permission/enabled >"$CURL_ALLOWED"

NAMES="mail sendmail curl python3 open osascript ssh xdg-open say notify-send"
for name in $NAMES; do
    {
        echo '#!/bin/sh'
        case "$name" in
            python3)
                # A marker in any argument lets the call through.
                echo 'for a in "$@"; do'
                echo '    while IFS= read -r m; do'
                echo '        [ -n "$m" ] || continue'
                echo '        case "$a" in *"$m"*)'
                echo "            printf 'python3 %s\\n' \"\$m\" >>'$PASSED_LOG'"
                echo "            exec '$REAL_PY' \"\$@\" ;;"
                echo '        esac'
                echo "    done <'$PY_ALLOWED'"
                echo 'done'
                ;;
            curl)
                # Exactly one URL, and it is the row's daemon address plus
                # an allowed route; then the call goes through.
                echo 'url=""; n=0'
                echo 'for a in "$@"; do case "$a" in *://*) url="$a"; n=$((n + 1)) ;; esac; done'
                echo "base=\"\$(cat '$ALLOW_BASE')\""
                echo 'if [ "$n" = 1 ] && [ -n "$base" ]; then'
                echo '    while IFS= read -r r; do'
                echo '        [ -n "$r" ] || continue'
                echo '        if [ "$url" = "$base$r" ]; then'
                echo "            printf 'curl %s\\n' \"\$url\" >>'$PASSED_LOG'"
                echo "            exec '$REAL_CURL' \"\$@\""
                echo '        fi'
                echo "    done <'$CURL_ALLOWED'"
                echo 'fi'
                ;;
        esac
        echo "{ printf '%s' '$name'; for a in \"\$@\"; do printf ' [%s]' \"\$a\"; done; echo; } >>'$LOG'"
        echo 'exit 97'
    } >"$STUBS/$name"
    chmod +x "$STUBS/$name"
done
export PATH="$STUBS:$PATH"

now_ms() { "$REAL_PY" -c 'import time; print(int(time.time() * 1000))'; }

# use_port <port>: the CLI's daemon port, and the only address curl may ask.
use_port() {
    export K2_PORT="$1"
    printf 'http://127.0.0.1:%s\n' "$1" >"$ALLOW_BASE"
}
use_port 1

echo "Stubs on PATH come first:"
for name in $NAMES; do
    if [ "$(command -v "$name")" = "$STUBS/$name" ]; then
        ok "$name resolves to the stub"
    else
        bad "$name resolves to $(command -v "$name" || echo nothing), not the stub"
    fi
done
# The stubs fail loudly and log: try each kind once, then start clean.
rc=0
mail -s x nobody </dev/null >/dev/null 2>&1 || rc=$?
if [ "$rc" = 97 ] && grep -q '^mail \[-s\] \[x\] \[nobody\]$' "$LOG"; then
    ok "a stub exits 97 and logs its argv"
else
    bad "the mail stub exited $rc and logged: $(cat "$LOG")"
fi
rc=0
python3 -c 'print(1)' >/dev/null 2>&1 || rc=$?
if [ "$rc" = 97 ] && grep -q '^python3 \[-c\] \[print(1)\]$' "$LOG"; then
    ok "python3 with no allowed marker fails and logs"
else
    bad "the python3 stub exited $rc for an unlisted call"
fi
for url in "http://127.0.0.1:1/cli/msg" "http://127.0.0.2:1/cli/permission/enabled" \
    "https://example.com/cli/permission/enabled"; do
    : >"$LOG"
    rc=0
    curl -s "$url" >/dev/null 2>&1 || rc=$?
    if [ "$rc" = 97 ] && grep -qF "[$url]" "$LOG"; then
        ok "curl to $url fails and logs"
    else
        bad "the curl stub exited $rc for $url"
    fi
done
: >"$LOG"
rc=0
curl -s "http://127.0.0.1:1/cli/permission/enabled" "http://127.0.0.1:1/cli/msg" >/dev/null 2>&1 || rc=$?
if [ "$rc" = 97 ]; then
    ok "curl with an allowed URL and another fails"
else
    bad "the curl stub let two URLs through (exit $rc)"
fi
: >"$LOG"
rc=0
curl -s --connect-timeout 1 "http://127.0.0.1:1/cli/permission/enabled" >/dev/null 2>&1 || rc=$?
if [ "$rc" != 97 ] && [ ! -s "$LOG" ] && grep -q 'permission/enabled' "$PASSED_LOG"; then
    ok "curl to an allowed route at the daemon address goes through (exit $rc)"
else
    bad "the curl stub stopped an allowed call (exit $rc): $(cat "$LOG")"
fi
: >"$LOG"
: >"$PASSED_LOG"

# The daily tier, cards off and agents allowed to report: what `k2 --help`
# and `k2 help` print when no daemon answers.
DAILY="$WORK/daily.txt"
{ cli_page "$HELP_TEXT/_daily.txt"; cli_page "$HELP_TEXT/_daily-feedback.txt"; cli_page "$HELP_TEXT/_daily-end.txt"; } >"$DAILY"
[ -s "$DAILY" ] || { echo "FAIL: no daily help text under $HELP_TEXT" >&2; exit 1; }

# run <tag> <expected exit> <args...>: one command, checked on its own.
# Its stdout is kept as $WORK/out.<tag>.
run() {
    local tag="$1" want="$2" label rc=0 t0 t1 ms
    shift 2
    label="k2 $* [$tag]"
    : >"$LOG"
    t0="$(now_ms)"
    "$K2" "$@" </dev/null >"$WORK/out" 2>"$WORK/err" || rc=$?
    t1="$(now_ms)"
    ms=$((t1 - t0))
    cp "$WORK/out" "$WORK/out.$tag"
    if [ "$rc" = "$want" ]; then
        ok "$label: exit $rc"
    else
        bad "$label: exit $rc, want $want"
    fi
    if [ -s "$WORK/out" ]; then
        ok "$label: prints on stdout"
    else
        bad "$label: printed nothing on stdout"
    fi
    if [ -s "$WORK/err" ]; then
        bad "$label: stderr is not empty: $(head -c 600 "$WORK/err")"
    else
        ok "$label: stderr is empty"
    fi
    if [ -s "$LOG" ]; then
        bad "$label: ran a stubbed program: $(cut -c1-300 "$LOG")"
    else
        ok "$label: ran no stubbed program"
    fi
    if [ "$ms" -lt "$MAX_MS" ]; then
        ok "$label: ${ms} ms"
    else
        bad "$label: took ${ms} ms, the limit is ${MAX_MS} ms"
    fi
}

# same <label> <file> <file>: two outputs are byte for byte the same.
same() {
    if cmp -s "$2" "$3"; then
        ok "$1"
    else
        bad "$1"
        diff "$2" "$3" | head -8 >&2 || true
    fi
}

echo "Help and version, daemon port closed:"
run closed.help 0 --help
run closed.helpword 0 help
run closed.version 0 --version
run closed.activity 0 activity --help
run closed.mail 0 mail --help
run closed.mailwatch 0 mail watch --help
run closed.msg 0 msg --help
run closed.compute 0 compute --help
run closed.app 0 app --help
run closed.vault 0 vault --help
run closed.workspace 0 workspace --help
run closed.agent 0 agent --help
same "k2 --help is the whole daily page" "$DAILY" "$WORK/out.closed.help"
same "k2 help is the whole daily page" "$DAILY" "$WORK/out.closed.helpword"
if grep -q '^Usage: k2 app ' "$WORK/out.closed.app"; then
    ok "k2 app --help prints the App page on stdout"
else
    bad "k2 app --help: no 'Usage: k2 app' line on stdout"
fi

# The listener behind the never-answering daemon rows, as a file: `serve
# <mode> <port file>` runs one, `decide <outcome> <completed>` prints what
# the full mode makes of one connect outcome (checked below on its own).
#   hang: accepts and holds each connection, never answers.
#   full: never accepts, and connects until its listen queue is full: a
#         connect that times out (Linux, older macOS), or that is reset or
#         refused after at least one got through (newer macOS answers a
#         full queue that way). Anything else is a loud setup failure.
LISTENER="$WORK/listener.py"
cat >"$LISTENER" <<'PYEOF'
import os, socket, sys, time


def full_decision(outcome, completed):
    """What one connect outcome means while filling the queue: "more" (it
    got through, keep going), "full", or "not full: <why>"."""
    if outcome == "completed":
        return "more"
    if outcome == "timeout":
        return "full"
    if outcome in ("reset", "refused"):
        if completed > 0:
            return "full"
        return "not full: the first connect was %s, so nothing listens there" % outcome
    return "not full: a connect failed: %s" % outcome


def outcome_of(c, port):
    try:
        c.connect(("127.0.0.1", port))
        return "completed"
    except socket.timeout:
        return "timeout"
    except ConnectionResetError:
        return "reset"
    except ConnectionRefusedError:
        return "refused"
    except OSError as e:
        return str(e)


def serve(mode, pf):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    port = srv.getsockname()[1]
    held = []
    how = "accepts and never answers"
    if mode == "hang":
        srv.listen(16)
    else:
        # Some systems queue more than the backlog asks: keep going.
        import resource
        soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        want = 2048 if hard == resource.RLIM_INFINITY else min(hard, 2048)
        if soft != resource.RLIM_INFINITY and soft < want:
            resource.setrlimit(resource.RLIMIT_NOFILE, (want, hard))
        srv.listen(0)
        completed, verdict, outcome = 0, "more", ""
        while completed < 1500 and verdict == "more":
            c = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            c.settimeout(0.3)
            held.append(c)
            outcome = outcome_of(c, port)
            verdict = full_decision(outcome, completed)
            if verdict == "more":
                completed += 1
        if verdict == "more":
            verdict = "not full: %d connects all got through" % completed
        if verdict != "full":
            print("full listener: %s" % verdict, file=sys.stderr)
            sys.exit(1)
        how = "full after %d connects (the next: %s)" % (completed, outcome)
    with open(pf + ".how", "w") as f:
        f.write(how)
    with open(pf + ".tmp", "w") as f:
        f.write(str(port))
    os.rename(pf + ".tmp", pf)
    while True:
        if mode == "hang":
            c, _ = srv.accept()
            held.append(c)
        else:
            time.sleep(60)


if sys.argv[1] == "decide":
    print(full_decision(sys.argv[2], int(sys.argv[3])))
else:
    serve(sys.argv[2], sys.argv[3])
PYEOF

echo "The full listener's decision, per connect outcome:"
for row in "completed 0|more" "timeout 0|full" "timeout 5|full" "reset 3|full" \
    "refused 3|full" \
    "reset 0|not full: the first connect was reset, so nothing listens there" \
    "refused 0|not full: the first connect was refused, so nothing listens there" \
    "Network is unreachable 2|not full: a connect failed: Network is unreachable"; do
    args="${row%%|*}" want="${row#*|}"
    got="$("$REAL_PY" "$LISTENER" decide "${args% *}" "${args##* }")"
    if [ "$got" = "$want" ]; then
        ok "decide ${args% *} after ${args##* }: $got"
    else
        bad "decide ${args% *} after ${args##* }: got '$got', want '$want'"
    fi
done

# start_daemon <mode>: a listener that never answers; its port goes in
# DAEMON_PORT. A listener that can't be set up fails the test with its
# reason.
start_daemon() {
    local mode="$1" pf="$WORK/port.$1" i pid
    "$REAL_PY" "$LISTENER" serve "$mode" "$pf" 2>"$pf.err" &
    pid=$!
    DAEMON_PIDS="$DAEMON_PIDS $pid"
    for i in $(seq 1 400); do
        [ -s "$pf" ] && break
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.05
    done
    if [ ! -s "$pf" ]; then
        echo "FAIL: the $mode listener did not start: $(cat "$pf.err")" >&2
        exit 1
    fi
    echo "  the $mode listener: $(cat "$pf.how")"
    DAEMON_PORT="$(cat "$pf")"
}

for mode in hang full; do
    echo "Help, a daemon that never answers ($mode):"
    start_daemon "$mode"
    use_port "$DAEMON_PORT"
    run "$mode.help" 0 --help
    run "$mode.helpword" 0 help
    run "$mode.app" 0 app --help
    same "k2 --help [$mode] is the whole daily page" "$DAILY" "$WORK/out.$mode.help"
    same "k2 help [$mode] is the whole daily page" "$DAILY" "$WORK/out.$mode.helpword"
    same "k2 app --help [$mode] is the page it prints with the port closed" \
        "$WORK/out.closed.app" "$WORK/out.$mode.app"
done

echo "Calls let through (allowed): $(wc -l <"$PASSED_LOG" | tr -d ' ')"
sort "$PASSED_LOG" | uniq -c | sed 's/^/    /'
echo
echo "Passed: $pass  Failed: $fail"
[ "$fail" -eq 0 ]
