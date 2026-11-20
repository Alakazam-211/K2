#!/usr/bin/env bash
# Bash arithmetic never evaluates text that came from a flag or an
# argument. Bash evaluates an array subscript inside $(( )), and the
# subscript may hold a command substitution; a letter-only value names a
# shell variable, and bash evaluates that variable's value too. So every
# duration flag that reaches $(( )) takes digits (and its unit letter)
# only; anything else is the usage error, exit 2, before any request.
#
# For each guarded flag: a value with a command in a subscript, and a
# letter-only value that names a variable holding one, must exit 2, send
# nothing and never run the command (a marker file). A valid value still
# reaches the daemon as before.
#
# Flags covered:
#   compute node servers add --for          (7d, 12h, none)
#   compute node servers limits for= / --for
#   compute node servers add --max-time, limits max_job_secs=<n>h|m|s
#   sources simulate --for, sources trigger add --cooldown and
#     --on stale_for:<duration>
#   sources mute --for, sources history --from
#   desktop env --timeout, desktop setup --timeout
#
# A tiny stub daemon on loopback logs every request. No real daemon, no
# ~/.k2, no keychain: HOME is a throwaway dir.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2arith-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG, WORK = sys.argv[1], sys.argv[2]

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def refuse(self):
        self.reply(404, {"error": {"code": "not_found", "hint": "stub: nothing runs here"}})
    def record(self, entry):
        with open(LOG, "a") as f:
            f.write(json.dumps(entry, sort_keys=True) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        self.record({"method": "GET", "path": u.path, "query": {k: v[0] for k, v in parse_qs(u.query).items()}})
        if u.path == "/cli/desktop/status":
            state = "setting_up" if os.path.exists(os.path.join(WORK, "installing")) else "closed"
            return self.reply(200, {"desktop": "virtual", "state": state, "message": "stub"})
        return self.refuse()
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        path = urlparse(self.path).path
        self.record({"method": "POST", "path": path, "body": body})
        if path == "/cli/desktop/setup":
            return self.reply(200, {"ok": True, "state": "setting_up", "message": "Installing"})
        if path == "/cli/desktop/env":
            state = "waiting" if os.path.exists(os.path.join(WORK, "env_waiting")) else "ready"
            return self.reply(200, {"ok": True, "desktop": "virtual:ws1", "name": "acme", "state": state,
                                    "message": "stub", "env": {"DISPLAY": ":101"},
                                    "hold": {"by": "acme", "until": 1731301800}, "label": "Separate, not private"})
        return self.refuse()

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(WORK, "port.tmp"), "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(os.path.join(WORK, "port.tmp"), os.path.join(WORK, "port"))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$SANDBOX/port" ] && break; sleep 0.05; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

MARK="$SANDBOX/mark"
# A subscript bash would expand (and so run touch) if this text reached
# its arithmetic. BASH_VERSINFO is always set, so set -u never stops the
# evaluation before the subscript. P goes in a flag value; EV goes in a
# shell variable that a letter-only flag value names (h or m).
P="BASH_VERSINFO[\$(touch $MARK)]"
EV="BASH_VERSINFO[\$(touch $MARK)]"

# refused <label> <stderr text> [VAR=value…] -- <k2 args…>: exit 2, the
# stderr text, no request, and no marker file.
refused() {
    local label="$1" want="$2" before after rc=0
    shift 2
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    rm -f "$MARK"
    before="$(wc -l < "$LOG")"
    env ${envs[@]+"${envs[@]}"} "$K2" "$@" >"$SANDBOX/out" 2>"$SANDBOX/err" </dev/null || rc=$?
    after="$(wc -l < "$LOG")"
    if [ -e "$MARK" ]; then bad "$label: the command in the value ran"; else ok "$label: the command in the value never ran"; fi
    expect_eq "$label: exit" "$rc" "2"
    expect_eq "$label: requests sent" "$((after - before))" "0"
    if grep -qF -- "$want" "$SANDBOX/err"; then ok "$label: says '$want'"; else bad "$label: stderr lacks '$want': $(cat "$SANDBOX/err")"; fi
}

# sent <args…>: run k2 (the stub may refuse, so any exit), print the one
# request it sent as sorted JSON. Exactly one request and no marker, or a
# FAIL.
sent() {
    local before after
    rm -f "$MARK"
    before="$(wc -l < "$LOG")"
    "$K2" "$@" >/dev/null 2>&1 </dev/null || true
    after="$(wc -l < "$LOG")"
    [ ! -e "$MARK" ] || bad "k2 $*: the marker file appeared"
    if [ "$((after - before))" != "1" ]; then
        bad "k2 $* sent $((after - before)) requests, want 1"
        echo "{}"
        return 0
    fi
    tail -1 "$LOG"
}
field() { python3 -c 'import json,sys; v=json.loads(sys.argv[1]); print(json.dumps(eval(sys.argv[2])))' "$1" "$2"; }

echo "== compute node servers add --for =="
ADD=(compute node servers n1 add --url https://acme.example --code ABC123.FP)
refused "add --for <subscript>d" "usage: k2 compute node servers" -- "${ADD[@]}" --for "${P}d"
refused "add --for <subscript>h" "usage: bad duration" -- "${ADD[@]}" --for "${P}h"
refused "add --for hd (names a variable)" "usage: k2 compute node servers" "h=$EV" -- "${ADD[@]}" --for hd
r="$(sent "${ADD[@]}" --for 7d)"
expect_eq "add --for 7d" "$(field "$r" 'v["path"], v["body"].get("forSecs")')" '["/cli/compute/nodes/servers/add", 604800]'
r="$(sent "${ADD[@]}" --for 12h)"
expect_eq "add --for 12h" "$(field "$r" 'v["body"].get("forSecs")')" '43200'
r="$(sent "${ADD[@]}" --for none)"
expect_eq "add --for none" "$(field "$r" 'v["body"].get("forSecs")')" '0'

echo "== compute node servers limits for= / --for =="
LIM=(compute node servers n1 limits slot:2)
refused "limits for=<subscript>d" "usage: k2 compute node servers" -- "${LIM[@]}" "for=${P}d"
refused "limits --for <subscript>d" "usage: k2 compute node servers" -- "${LIM[@]}" --for "${P}d"
refused "limits for=hd (names a variable)" "usage: k2 compute node servers" "h=$EV" -- "${LIM[@]}" for=hd
r="$(sent "${LIM[@]}" for=2d)"
expect_eq "limits for=2d" "$(field "$r" 'v["path"], v["body"].get("forSecs")')" '["/cli/compute/nodes/servers/limits", 172800]'
r="$(sent "${LIM[@]}" --for 90m)"
expect_eq "limits --for 90m" "$(field "$r" 'v["body"].get("forSecs")')" '5400'

echo "== compute node servers add --max-time, limits max_job_secs=: a bad value stops =="
refused "add --max-time <subscript>h" "usage: bad duration" -- "${ADD[@]}" --max-time "${P}h"
refused "add --max-time 5x" "usage: bad duration" -- "${ADD[@]}" --max-time 5x
refused "limits max_job_secs=<subscript>h" "usage: bad duration" -- "${LIM[@]}" "max_job_secs=${P}h"
refused "limits max_job_secs=xh" "usage: bad duration" -- "${LIM[@]}" max_job_secs=xh
r="$(sent "${ADD[@]}" --max-time 2h)"
expect_eq "add --max-time 2h" "$(field "$r" 'v["body"]["set"].get("max_job_secs")')" '7200'
r="$(sent "${LIM[@]}" max_job_secs=1h30m)"
expect_eq "limits max_job_secs=1h30m" "$(field "$r" 'v["body"]["set"].get("max_job_secs")')" '5400'

echo "== sources simulate --for, trigger add --cooldown / --on stale_for: =="
refused "simulate --for <subscript>d" "usage: bad duration" -- sources simulate fleet crit --for "${P}d"
refused "simulate --for hd (names a variable)" "usage: bad duration" "h=$EV" -- sources simulate fleet crit --for hd
r="$(sent sources simulate fleet crit --for 1h)"
expect_eq "simulate --for 1h" "$(field "$r" 'v["path"], v["body"].get("forSecs")')" '["/cli/sources/simulate", 3600]'
refused "trigger add --cooldown <subscript>d" "usage: bad duration" -- sources trigger add --source fleet --on went_crit --cooldown "${P}d"
refused "trigger add --on stale_for:<subscript>h" "usage: bad duration" -- sources trigger add --source fleet --on "stale_for:${P}h"
refused "trigger add --cooldown md (names a variable)" "usage: bad duration" "m=$EV" -- sources trigger add --source fleet --on went_crit --cooldown md
r="$(sent sources trigger add --source fleet --on stale_for:30m --cooldown 2h)"
expect_eq "trigger add stale_for:30m --cooldown 2h" "$(field "$r" 'v["path"], v["body"].get("on"), v["body"].get("cooldownSecs")')" '["/cli/sources/triggers/add", {"kind": "stale_for", "secs": 1800}, 7200]'
r="$(sent sources trigger add --source fleet --on stale_for:45s --cooldown 300)"
expect_eq "trigger add stale_for:45s --cooldown 300" "$(field "$r" 'v["body"].get("on"), v["body"].get("cooldownSecs")')" '[{"kind": "stale_for", "secs": 45}, 300]'

echo "== sources mute --for, sources history --from =="
refused "mute --for <subscript>d" "Error: --for takes a duration" -- sources mute fleet --for "${P}d"
refused "mute --for hd (names a variable)" "Error: --for takes a duration" "h=$EV" -- sources mute fleet --for hd
r="$(sent sources mute fleet --for 30m)"
expect_eq "mute --for 30m" "$(field "$r" 'v["path"], v["body"].get("forSecs")')" '["/cli/sources/mute", 1800]'
refused "history --from hd (names a variable)" "Error: --from takes a duration" "h=$EV" -- sources history fleet health --from hd
refused "history --from ms (names a variable)" "Error: --from takes a duration" "m=$EV" -- sources history fleet health --from ms
t0="$(date +%s)"
r="$(sent sources history fleet health --from 6h)"
t1="$(date +%s)"
from="$(field "$r" 'int(v["query"]["from"])')"
if [ "$from" -ge "$((t0 - 21600))" ] && [ "$from" -le "$((t1 - 21600))" ]; then ok "history --from 6h: six hours back"; else bad "history --from 6h: from=$from (now $t0..$t1)"; fi

echo "== desktop env --timeout, desktop setup --timeout =="
refused "env --timeout <subscript>m" "usage: --timeout" -- desktop env --timeout "${P}m"
refused "env --timeout hm (names a variable)" "usage: --timeout" "h=$EV" -- desktop env --timeout hm
refused "setup --timeout hm (names a variable)" "usage: --timeout" "h=$EV" -- desktop setup --timeout hm
r="$(sent desktop env --timeout 2m)"
expect_eq "env --timeout 2m" "$(field "$r" 'v["path"]')" '"/cli/desktop/env"'
# The seconds still bound the wait: --timeout 0m gives up at once.
touch "$SANDBOX/env_waiting"
rc=0; "$K2" desktop env --timeout 0m >/dev/null 2>"$SANDBOX/err" </dev/null || rc=$?
rm -f "$SANDBOX/env_waiting"
expect_eq "env --timeout 0m on a waiting desktop: exit" "$rc" "1"
if grep -qF "still waiting after 0 s" "$SANDBOX/err"; then ok "env --timeout 0m: gives up at 0 s"; else bad "env --timeout 0m: $(cat "$SANDBOX/err")"; fi
rc=0; out="$("$K2" desktop setup --timeout 10m 2>/dev/null </dev/null)" || rc=$?
expect_eq "setup --timeout 10m: exit" "$rc" "0"
case "$out" in *"Installed."*) ok "setup --timeout 10m: waits and says installed" ;; *) bad "setup --timeout 10m: $out" ;; esac
touch "$SANDBOX/installing"
rc=0; "$K2" desktop setup --timeout 1s >/dev/null 2>"$SANDBOX/err" </dev/null || rc=$?
rm -f "$SANDBOX/installing"
expect_eq "setup --timeout 1s while installing: exit" "$rc" "1"
if grep -qF "still installing after 2 s" "$SANDBOX/err"; then ok "setup --timeout 1s: gives up after the first poll"; else bad "setup --timeout 1s: $(cat "$SANDBOX/err")"; fi

echo
echo "arith_user_input: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
