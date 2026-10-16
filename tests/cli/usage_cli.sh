#!/usr/bin/env bash
# k2 usage — token usage and subscription limits (prd-usage-v2 §5).
#
#   1. No daemon: help text; usage errors exit 2; the tool catalog maps
#      `usage` to the locked `id` tool `usage`; schema lists the verbs.
#   2. A stub daemon: `tokens` sends the filters (workspace, harness,
#      login, model, since, until, group_by, tz) and renders totals, cost
#      and the breakdown; `--json` passes the body through; `turns` sends
#      limit/before and prints the older cursor; `limits` renders logins
#      (default mark, % used, state, freshness); `limits --refresh` is a
#      POST with a JSON body; the owner (outside K2) sends the disk owner
#      token; a K2 terminal (fake cell) sends the SCOPED passport, never
#      the owner token; a 403 (a Member, or an agent refreshing) → exit 3
#      with the daemon's text.
# Fake values only (example.test, made-up ids).

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

WORK="$(mktemp -d -t k2-usage-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

# Never inherit a session's identity from the caller.
for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

# ── 1. no daemon ─────────────────────────────────────────────────────
echo "== help + usage (no daemon) =="
HOME1="$WORK/home1"
mkdir -p "$HOME1"
nod() { env HOME="$HOME1" K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2_CLI" "$@"; }
capture_nod() {
    set +e
    out="$(nod "$@" 2>&1)"
    rc=$?
    set -e
}

capture_nod usage --help
assert_eq "usage --help exit 0" "$rc" "0"
assert_contains "help: tokens" "$out" "k2 usage tokens [--workspace PATH|--here]"
assert_contains "help: turns" "$out" "k2 usage turns [--limit N]"
assert_contains "help: limits" "$out" "k2 usage limits [--refresh]"
assert_contains "help: not your bill" "$out" "not your bill"
assert_contains "help: agent scope" "$out" "you see your own workspace only"
set +e
out="$(env HOME="$HOME1" "$K2_CLI" usage 2>&1)"; rc=$?
set -e
assert_eq "bare k2 usage prints help without a daemon" "$rc" "0"
capture_nod help usage
assert_contains "k2 help usage" "$out" "k2 usage limits"

capture_nod usage frobnicate
assert_eq "unknown verb exit 2" "$rc" "2"
assert_contains "unknown verb points at help" "$out" "k2 usage --help"
capture_nod usage tokens --harness gemini
assert_eq "bad harness exit 2" "$rc" "2"
capture_nod usage tokens --by color
assert_eq "bad --by exit 2" "$rc" "2"
capture_nod usage tokens --since
assert_eq "--since without a value exit 2" "$rc" "2"
capture_nod usage tokens --workspace /x --outside
assert_eq "workspace + outside exit 2" "$rc" "2"
capture_nod usage tokens --refresh
assert_eq "tokens --refresh exit 2" "$rc" "2"
capture_nod usage turns --limit 0
assert_eq "turns --limit 0 exit 2" "$rc" "2"
capture_nod usage limits --since 7d
assert_eq "limits --since exit 2" "$rc" "2"
capture_nod usage tokens --bogus
assert_eq "unknown flag exit 2" "$rc" "2"

eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "catalog: usage tool id" "$(_cli_tool_id_for_verb usage)" "usage"
assert_eq "catalog: usage is an id tool" "$(_cli_tool_default_mode usage)" "id"
if _cli_tool_is_locked usage; then ok "catalog: usage is locked"; else bad "catalog: usage is locked"; fi

schema="$(env HOME="$HOME1" "$K2_CLI" --schema)"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
names = {c["name"] for c in d["commands"]}
for n in ("usage tokens", "usage turns", "usage limits"):
    assert n in names, n
' "$schema" && ok "schema lists usage verbs" || bad "schema usage verbs"

# ── 2. stub daemon ───────────────────────────────────────────────────
echo "== stub daemon =="
OWNER="owner-disk-token-usage"
SCOPED="sess91.scoped-secret-usage"
HOME2="$WORK/home2"
mkdir -p "$HOME2/.k2"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" "$SCOPED" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, scoped = sys.argv[1], sys.argv[2]
TOTALS = {"input_tokens": 1200, "output_tokens": 340, "cache_read_tokens": 56000, "cache_write_tokens": 700,
          "reasoning_tokens": 40, "requests": 12, "turns": 12,
          "cost": {"usd": 1.234, "source": "mixed", "priced_requests": 11, "provider_requests": 1, "unpriced_requests": 1}}
REPORT = {"scope": "machine", "workspace": None, "tz": "America/Denver", "since_ms": None, "until_ms": None,
          "total": TOTALS, "harnesses": [dict(harness="claude", **TOTALS)], "models": [], "workspaces": [],
          "logins": [], "outside": TOTALS, "days": [dict(day="2026-10-15", **TOTALS)],
          "groups": [dict(key="claude/claude-opus-5-5", label="claude-opus-5-5", **TOTALS)],
          "prices": {"source": "bundled", "fetched_at": None, "models": 186}}
TURNS = {"rows": [{"rowid": 7, "cursor": "1792000000000:7", "workspace_path": "/w/app", "workspace": "/w", "outside": False,
                   "harness": "codex", "model": "gpt-5-codex", "account_id": "acc_1", "session_id": "s1",
                   "input_tokens": 10, "output_tokens": 2, "cache_read_tokens": 30, "cache_write_tokens": 0,
                   "reasoning_tokens": 1, "recorded_at": "x", "recorded_ms": 1792000000000,
                   "cost_usd": 0.0001, "cost_source": "model"}],
         "next_cursor": "1792000000000:7", "has_more": True}
LIMITS = {"generatedAt": "2026-10-16T12:00:00Z", "airgap": False, "tools": [
    {"tool": "claude", "display": "Claude", "supported": True, "logins": [
        {"accountId": "acc_1", "label": "work", "email": "person@example.test", "kind": "subscription",
         "isDefault": True, "isLive": True, "pinned": [], "state": "ok", "status": "", "error": None,
         "plan": "Max 20x", "checkedAt": "2026-10-16T11:58:00Z", "attemptedAt": None, "source": "live_statusline",
         "nextProbeAt": None, "windows": [{"id": "five_hour", "label": "Session", "kind": "session", "used": 0.32,
         "usedPercent": 32.0, "resetsAt": "2099-01-01T00:00:00Z", "windowSecs": 18000}]},
        {"accountId": "acc_2", "label": "spare", "email": None, "kind": "subscription",
         "isDefault": False, "isLive": False, "pinned": [{"scopeKind": "workspace", "scopeId": "p1"}],
         "state": "expired", "status": "Sign-in expired", "error": "the access token expired before the check",
         "plan": "", "checkedAt": "", "attemptedAt": None, "source": "", "nextProbeAt": None, "windows": []}]},
    {"tool": "gemini", "display": "Gemini", "supported": False, "logins": []}]}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        token = (q.get("token") or [""])[0]
        rec = {"method": method, "path": parsed.path, "token": token,
               "query": {k: v[0] for k, v in q.items()}, "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        status, out = 200, {}
        if parsed.path == "/cli/usage/tokens":
            if rec["query"].get("harness") == "grok":
                status, out = 403, {"error": "Usage is visible to the server Owner and Admins", "code": "admin_only"}
            else:
                out = REPORT
        elif parsed.path == "/cli/usage/turns":
            out = TURNS
        elif parsed.path == "/cli/usage/limits":
            out = LIMITS
        elif parsed.path == "/cli/usage/limits/refresh":
            if method != "POST":
                status, out = 405, {"error": "POST required"}
            elif token == scoped:
                status, out = 403, {"error": "an agent reads limits but can't start a check", "code": "owner_only"}
            else:
                out = LIMITS
        else:
            status, out = 404, {"error": "route not found"}
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

owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" TZ=America/Denver "$K2_CLI" "$@"; }
cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
last() { tail -n 1 "$WORK/reqs.jsonl" | python3 -c "import json,sys; print(json.load(sys.stdin)[sys.argv[1]])" "$1"; }
lastq() { tail -n 1 "$WORK/reqs.jsonl" | python3 -c "import json,sys; print(json.load(sys.stdin)['query'].get(sys.argv[1], '<none>'))" "$1"; }

out="$(owner usage tokens --since 7d --harness claude --login acc_1 --model claude-opus-5-5 --by model --workspace /w)"
assert_eq "tokens is a GET" "$(last method)" "GET"
assert_eq "tokens path" "$(last path)" "/cli/usage/tokens"
assert_eq "owner sends the disk owner token" "$(last token)" "$OWNER"
assert_eq "since sent" "$(lastq since)" "7d"
assert_eq "harness sent" "$(lastq harness)" "claude"
assert_eq "login sent as account" "$(lastq account)" "acc_1"
assert_eq "model sent" "$(lastq model)" "claude-opus-5-5"
assert_eq "group_by sent" "$(lastq group_by)" "model"
assert_eq "workspace sent" "$(lastq workspace)" "/w"
assert_eq "tz defaults to this computer's zone" "$(lastq tz)" "America/Denver"
assert_contains "tokens: total line" "$out" "Total"
assert_contains "tokens: requests" "$out" "12 req"
assert_contains "tokens: cache read in k" "$out" "56.0k"
assert_contains "tokens: cost" "$out" "Estimated API-equivalent cost: \$1.23"
assert_contains "tokens: not your bill" "$out" "not your bill"
assert_contains "tokens: unpriced tag" "$out" "+1 unpriced"
assert_contains "tokens: breakdown heading" "$out" "By model:"
assert_contains "tokens: a day" "$out" "2026-10-15"

out="$(owner usage tokens --tz UTC --json --here)"
python3 -c 'import json,sys; v=json.loads(sys.argv[1]); assert v["total"]["requests"]==12, v' "$out" \
    && ok "tokens --json passes the body through" || bad "tokens --json"
assert_eq "explicit tz wins" "$(lastq tz)" "UTC"
assert_eq "--here sends this folder" "$(lastq workspace)" "$(pwd)"

out="$(owner usage turns --limit 5 --before 1792000000001:9)"
assert_eq "turns path" "$(last path)" "/cli/usage/turns"
assert_eq "turns limit" "$(lastq limit)" "5"
assert_eq "turns before" "$(lastq before)" "1792000000001:9"
assert_contains "turns: a row" "$out" "gpt-5-codex"
assert_contains "turns: older cursor" "$out" "k2 usage turns --before 1792000000000:7"

out="$(owner usage limits)"
assert_eq "limits is a GET" "$(last method)" "GET"
assert_eq "limits path" "$(last path)" "/cli/usage/limits"
assert_contains "limits: tool heading" "$out" "Claude:"
assert_contains "limits: default mark + percent" "$out" "* work"
assert_contains "limits: window" "$out" "Session 32%"
assert_contains "limits: live" "$out" "live"
assert_contains "limits: expired state" "$out" "sign-in needs a refresh"
assert_contains "limits: pinned" "$out" "[pinned]"
assert_absent "limits: gemini without logins hidden" "$out" "Gemini"

out="$(owner usage limits --refresh --tool claude)"
assert_eq "refresh is a POST" "$(last method)" "POST"
assert_eq "refresh path" "$(last path)" "/cli/usage/limits/refresh"
body="$(last body)"
assert_eq "refresh body" "$(python3 -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1]), sort_keys=True))' "$body")" '{"tool": "claude"}'

echo "== K2 terminal: passport =="
out="$(cell usage limits)"
assert_eq "passport limits sends the scoped token" "$(last token)" "$SCOPED"
assert_absent "passport never sends the owner token" "$(cat "$WORK/reqs.jsonl" | tail -n 1)" "$OWNER"
set +e
out="$(cell usage limits --refresh 2>&1)"; rc=$?
set -e
assert_eq "passport refresh → exit 3" "$rc" "3"
assert_contains "passport refresh says why" "$out" "can't start a check"

echo "== refusal =="
set +e
out="$(owner usage tokens --harness grok 2>&1)"; rc=$?
set -e
assert_eq "403 → exit 3" "$rc" "3"
assert_contains "403 prints the daemon's text" "$out" "Owner and Admins"

echo "usage_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
