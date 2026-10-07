#!/usr/bin/env bash
# k2 sidecar v1 (prd-k2-sidecar-cli-v1 T2).
#
#   1. No daemon: help text; usage errors exit 2; both brief flags exit 2;
#      `k2 sessions spawn` is retired ("moved", exit 2); the tool catalog
#      lists `sidecar` as an `id` tool; schema and daily help list it.
#   2. A fake cell (K2_HOOK_SOCK + K2_HOOK_TOKEN set, owner token on disk)
#      against a stub daemon: the request carries the SCOPED token, never
#      the owner token; stdout is exactly the address; --json is the
#      route's JSON unchanged; `--brief -` reads stdin; refusals map to exit
#      codes (409 → 1, 403 → 3, 404 → 4, 413 → 2).
#   3. A real headless daemon (temp HOME, claude shim): new / list / stop /
#      access end to end; BRIEF.md is 0600; the brief text never reaches
#      the daemon log.
# Build first for part 3: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-sidecar-cli-XXXXXX)"
STUB_PID=""
DAEMON_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.3
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# Never inherit a session's identity from the caller.
unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
    K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL K2_PORT K2_HOOK_TOKEN || true

# ── 1. no daemon ─────────────────────────────────────────────────────
echo "== help + usage (no daemon) =="
HOME1="$WORK/home1"
mkdir -p "$HOME1"
nod() {
    env HOME="$HOME1" K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2_CLI" "$@"
}
capture_nod() {
    set +e
    out="$(nod "$@" 2>&1)"
    rc=$?
    set -e
}

capture_nod sidecar --help
assert_eq "sidecar --help exit 0" "$rc" "0"
assert_contains "help: new" "$out" "k2 sidecar new <name> --harness <preset>"
assert_contains "help: list" "$out" "k2 sidecar list [--workspace <ws>] [--json]"
assert_contains "help: stop" "$out" "k2 sidecar stop <name> [--workspace <ws>] [--force] [--json]"
assert_contains "help: access" "$out" "k2 sidecar access on|off|status [--workspace <ws>]"
assert_contains "help: Big 7" "$out" "claude, grok, codex, gemini, cursor-agent, pi, hermes"
assert_contains "help: secrets" "$out" "Do not put secrets in a brief."
assert_contains "help: study" "$out" "k2 study sidecars"
capture_nod sidecar
assert_eq "bare sidecar prints help" "$rc" "0"
assert_contains "bare sidecar help text" "$out" "open extra AI sessions"
# --help with no daemon at all (no port/token) still works.
set +e
out="$(env HOME="$HOME1" "$K2_CLI" sidecar --help 2>&1)"; rc=$?
set -e
assert_eq "sidecar --help needs no daemon" "$rc" "0"

capture_nod sidecar new
assert_eq "new without a name exit 2" "$rc" "2"
capture_nod sidecar new Gardens
assert_eq "new without --harness exit 2" "$rc" "2"
assert_contains "new usage text" "$out" "usage: k2 sidecar new <name> --harness <preset>"
printf 'x' > "$WORK/brief.md"
capture_nod sidecar new Gardens --harness claude --brief "$WORK/brief.md" --brief-text hi
assert_eq "both brief flags exit 2" "$rc" "2"
assert_contains "both brief flags say so" "$out" "not both"
capture_nod sidecar new Gardens --harness claude --brief "$WORK/nope.md"
assert_eq "missing brief file exit 2" "$rc" "2"
capture_nod sidecar new Gardens --harness claude --bogus
assert_eq "unknown flag exit 2" "$rc" "2"
capture_nod sidecar stop
assert_eq "stop without a name exit 2" "$rc" "2"
capture_nod sidecar access maybe
assert_eq "access bad action exit 2" "$rc" "2"
capture_nod sidecar frobnicate
assert_eq "unknown verb exit 2" "$rc" "2"

set +e
out="$(env HOME="$HOME1" "$K2_CLI" sessions spawn --agent foo 2>&1)"; rc=$?
set -e
assert_eq "sessions spawn retired exit 2" "$rc" "2"
assert_contains "sessions spawn says where it moved" "$out" "moved to: k2 sidecar new"

eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "catalog: sidecar tool id" "$(_cli_tool_id_for_verb sidecar)" "sidecar"
assert_eq "catalog: sidecar is an id tool (SC35)" "$(_cli_tool_default_mode sidecar)" "id"

schema="$(env HOME="$HOME1" "$K2_CLI" --schema)"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
names = {c["name"] for c in d["commands"]}
for n in ("sidecar new", "sidecar list", "sidecar stop", "sidecar access"):
    assert n in names, n
assert "sessions spawn" not in names, "sessions spawn is retired"
' "$schema" && ok "schema lists sidecar verbs, not sessions spawn" || bad "schema sidecar verbs"
help_daily="$(sed -n '/^cmd_help_v2_daily() {/,/^}/p' "$K2_CLI")"
assert_contains "daily help lists sidecar" "$help_daily" "sidecar new <name> --harness <preset>"
assert_absent "internal help drops sessions spawn" "$(sed -n '/^cmd_help_internal() {/,/^}/p' "$K2_CLI")" "sessions spawn"

# ── 2. fake cell → stub daemon ───────────────────────────────────────
echo "== fake cell: scoped token, stdout, --json, exit codes =="
OWNER="owner-disk-token-sidecar"
SCOPED="sess42.scoped-secret-sidecar"
HOME2="$WORK/home2"
mkdir -p "$HOME2/.k2"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
REFUSE = {
    "live": (409, "sidecar_live"),
    "gated": (403, "gated"),
    "gone": (404, "no_such_sidecar"),
    "huge": (413, "brief_too_large"),
}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {
            "method": method,
            "path": parsed.path,
            "token_query": (q.get("token") or [""])[0],
            "authorization": self.headers.get("Authorization") or "",
            "query": {k: v[0] for k, v in q.items()},
            "body": body.decode("utf-8", "replace"),
        }
        with open(os.path.join(work, "last_req.json"), "w") as f:
            json.dump(rec, f)
        status, out = 200, {}
        b = json.loads(body) if body else {}
        name = (b.get("name") or "").lower()
        if name in REFUSE:
            status, code = REFUSE[name]
            out = {"error": {"code": code, "hint": "stub says " + code}}
        elif parsed.path == "/cli/sidecar/new":
            out = {"ok": True, "address": "k2/" + name, "workspace": "k2", "handle": name,
                   "name": b.get("name"), "harness": "claude", "resumed": False,
                   "brief": {"path": ".k2/sidecars/%s/BRIEF.md" % name, "bytes": len(b.get("brief") or ""), "delivered": "launch_param"} if b.get("brief") is not None else None}
        elif parsed.path == "/cli/sidecar/stop":
            out = {"ok": True, "address": "k2/" + name, "stopped": True, "state": "asleep"}
        elif parsed.path == "/cli/sidecar/list":
            out = {"ok": True, "workspace": "k2", "cap": 8, "live": 0, "agentsCanManage": False, "sidecars": []}
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
STUB_PORT="$(cat "$WORK/stub.port")"

cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
last() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$WORK/last_req.json" "$1"; }

set +e
out="$(cell sidecar new Gardens --harness claude --brief-text 'do it' 2>"$WORK/err")"; rc=$?
set -e
assert_eq "new exit 0" "$rc" "0"
assert_eq "stdout is exactly the address" "$out" "k2/gardens"
assert_contains "stderr summary" "$(cat "$WORK/err")" "opened k2/gardens"
assert_eq "request carries the scoped token" "$(last token_query)" "$SCOPED"
assert_absent "never the owner token" "$(cat "$WORK/last_req.json")" "$OWNER"
assert_eq "POST /cli/sidecar/new" "$(last path)" "/cli/sidecar/new"
python3 -c '
import json, sys
r = json.load(open(sys.argv[1])); b = json.loads(r["body"])
assert b == {"workspace": sys.argv[2], "name": "Gardens", "harness": "claude", "brief": "do it"}, b
' "$WORK/last_req.json" "$WORK/proj" && ok "new body shape" || bad "new body shape"

printf '# from stdin\nline two\n' | cell sidecar new Stdin --harness claude --brief - >/dev/null 2>&1
python3 -c '
import json, sys
b = json.loads(json.load(open(sys.argv[1]))["body"])
assert b["brief"] == "# from stdin\nline two\n", b
' "$WORK/last_req.json" && ok "--brief - reads stdin" || bad "--brief - reads stdin"

json_out="$(cell sidecar new Raw --harness claude --json 2>/dev/null)"
python3 -c '
import json, sys
v = json.loads(sys.argv[1])
assert v["address"] == "k2/raw" and v["ok"] is True and v["brief"] is None, v
' "$json_out" && ok "--json prints the route JSON" || bad "--json prints the route JSON ($json_out)"

list_json="$(cell sidecar list --json)"
assert_eq "list GET path" "$(last path)" "/cli/sidecar/list"
assert_eq "list sends the workspace" "$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['query']['workspace'])" "$WORK/last_req.json")" "$WORK/proj"
assert_eq "list scoped token" "$(last token_query)" "$SCOPED"
python3 -c 'import json,sys; assert json.loads(sys.argv[1])["cap"] == 8' "$list_json" && ok "list --json shape" || bad "list --json shape"
assert_contains "list table empty state" "$(cell sidecar list)" "(no sidecars)"

stop_out="$(cell sidecar stop Gardens)"
assert_eq "stop prints stopped" "$stop_out" "stopped k2/gardens"
python3 -c '
import json, sys
b = json.loads(json.load(open(sys.argv[1]))["body"])
assert b == {"workspace": sys.argv[2], "name": "Gardens", "force": False}, b
' "$WORK/last_req.json" "$WORK/proj" && ok "stop body shape" || bad "stop body shape"

for pair in "live:1" "gated:3" "gone:4" "huge:2"; do
    nm="${pair%%:*}"; want="${pair##*:}"
    set +e
    err="$(cell sidecar new "$nm" --harness claude 2>&1 >/dev/null)"; rc=$?
    set -e
    assert_eq "refusal $nm exit $want" "$rc" "$want"
done
set +e
err="$(cell sidecar new live --harness claude 2>&1 >/dev/null)"
set -e
assert_contains "refusal prints code: hint" "$err" "sidecar_live: stub says sidecar_live"

# ── 3. real headless daemon ──────────────────────────────────────────
echo "== real daemon: new / list / stop / access =="
DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || { echo "FAIL: k2-daemon not built (cargo build -p k2-daemon)" >&2; exit 1; }

SB="$WORK/sb"
mkdir -p "$SB/.k2" "$SB/shim"
printf '#!/bin/sh\nprintf "%%s\\n" "$@" > "%s/claude.argv"\nexec /bin/cat\n' "$SB" > "$SB/shim/claude"
chmod +x "$SB/shim/claude"
HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    "$DAEMON_BIN" >"$SB/daemon.log" 2>&1 &
DAEMON_PID=$!
disown "$DAEMON_PID" 2>/dev/null || true
for _ in $(seq 1 200); do
    [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ] && break
    sleep 0.1
done
[ -s "$SB/.k2/daemon.port" ] || { echo "FAIL: daemon never wrote daemon.port" >&2; tail -30 "$SB/daemon.log" >&2; exit 1; }
DPORT="$(cat "$SB/.k2/daemon.port")"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
phase=""
for _ in $(seq 1 300); do
    phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || { echo "FAIL: daemon never reached ready" >&2; exit 1; }

WSP="$SB/sidews"
resp="$(curl -s -X POST "http://127.0.0.1:$DPORT/cli/workspace/create?token=$DTOKEN&path=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$WSP")" --data-raw '')"
assert_contains "workspace registered" "$resp" "sidews"
assert_absent "workspace create did not refuse" "$resp" '"error"'

real() {
    env HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" K2_HOOK_TOKEN="$DTOKEN" \
        K2_PROJECT_PATH="$WSP" "$K2_CLI" "$@"
}
MARKER="CLI-BRIEF-MARKER-$$-$RANDOM"
set +e
addr="$(printf '# brief\n%s\n' "$MARKER" | real sidecar new Gardens --harness claude --brief - 2>"$SB/new.err")"; rc=$?
set -e
assert_eq "real new exit 0" "$rc" "0"
assert_eq "real new prints the address" "$addr" "sidews/gardens"
[ "$rc" = "0" ] || cat "$SB/new.err" >&2
for _ in $(seq 1 100); do [ -s "$SB/claude.argv" ] && break; sleep 0.05; done
assert_contains "harness got the pointer" "$(cat "$SB/claude.argv" 2>/dev/null)" "Your brief is .k2/sidecars/gardens/BRIEF.md"
assert_absent "harness argv never holds the brief" "$(cat "$SB/claude.argv" 2>/dev/null)" "$MARKER"
mode="$(stat -f %Lp "$WSP/.k2/sidecars/gardens/BRIEF.md" 2>/dev/null || stat -c %a "$WSP/.k2/sidecars/gardens/BRIEF.md")"
assert_eq "BRIEF.md is 0600" "$mode" "600"
assert_contains "BRIEF.md content" "$(cat "$WSP/.k2/sidecars/gardens/BRIEF.md")" "$MARKER"

lst="$(real sidecar list)"
assert_contains "list table header" "$lst" "ADDRESS"
assert_contains "list row" "$lst" "sidews/gardens"
assert_contains "list row live" "$lst" "live"
real sidecar list --json | python3 -c '
import json, sys
v = json.load(sys.stdin)
assert v["live"] == 1 and v["sidecars"][0]["address"] == "sidews/gardens", v
assert v["sidecars"][0]["createdBy"] == "owner", v
' && ok "list --json live row" || bad "list --json live row"

set +e
real sidecar new Gardens --harness claude >/dev/null 2>"$SB/dup.err"; rc=$?
set -e
assert_eq "same name while live exit 1" "$rc" "1"
assert_contains "same name while live code" "$(cat "$SB/dup.err")" "sidecar_live"

assert_eq "stop" "$(real sidecar stop gardens)" "stopped sidews/gardens"
assert_eq "stop again" "$(real sidecar stop gardens)" "sidews/gardens was already asleep"
set +e
real sidecar stop nobody >/dev/null 2>"$SB/nob.err"; rc=$?
set -e
assert_eq "stop unknown exit 4" "$rc" "4"

assert_contains "access status off" "$(real sidecar access status)" "Allow hiring and managing agents: off"
assert_contains "access on" "$(real sidecar access on)" "Allow hiring and managing agents: on"
assert_contains "access status on" "$(real sidecar access status)" "Allow hiring and managing agents: on"

assert_absent "brief text never in the daemon log" "$(cat "$SB/daemon.log")" "$MARKER"
assert_contains "audit has sidecar-new" "$(cat "$SB/.k2/auth-audit.jsonl")" '"event":"sidecar-new"'
assert_absent "audit never holds the brief" "$(cat "$SB/.k2/auth-audit.jsonl")" "$MARKER"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
