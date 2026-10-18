#!/usr/bin/env bash
# `k2 agent identity` (0.46.1) and its leftover spelling
# `k2 workspace set-agent-identity`.
#
# Drives the REAL cli/k2 against a stub daemon (loopback TCP plus a cell
# socket) that records each request and asserts what the CLI sends:
#   1. tool policy: both spellings are the locked `agent-identity` id tool.
#   2. owner terminal: POST /cli/agent/identity {workspace, color} with the
#      owner token; rgb(r,g,b) and r,g,b become hex; --name rides along.
#   3. show (no flags): GET with workspace=<agent>; prints name/color/picture.
#   4. --image: PNG sent as a data URL; SVG / non-image / both image flags
#      are usage errors (exit 2) that never reach the daemon.
#   5. agent session (cell socket + passport): the passport rides the cell
#      socket as Bearer, never the owner token on disk; no agent named sends
#      no workspace (the daemon uses the passport's own).
#   6. daemon refusals: 403 → exit 3 with the hint; 404 → exit 4; 429 →
#      exit 1 "try again".
#   7. the alias sends the same request and prints a one-line note.
#   8. --json prints the daemon JSON; --schema, k2 agent --help and
#      k2 study identity teach the verb.
#
# No daemon build; never touches ~/.k2 (HOME is a temp dir).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORK="$(mktemp -d -t k2-agent-identity-XXXXXX)"
# Short path: unix socket paths are capped near 104 bytes.
SOCK="/tmp/k2ai-$$-${RANDOM}.sock"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
    rm -f "$SOCK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

python3 - "$WORK" "$SOCK" <<'PYEOF' &
import json, os, socketserver, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

work, sock = sys.argv[1], sys.argv[2]
n = {"i": 0}
lock = threading.Lock()

def mode():
    p = os.path.join(work, "mode")
    return open(p).read().strip() if os.path.exists(p) else "ok"

VIEW = {"workspace": "docs", "handle": "docs", "displayName": "Docs", "color": "#22c55e",
        "image": None, "canEditIdentity": True, "canEditHandle": True}

class H(BaseHTTPRequestHandler):
    def reply(self, obj, code=200):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def record(self, method, body):
        u = urlparse(self.path)
        with lock:
            n["i"] += 1
            i = n["i"]
        rec = {
            "transport": "uds" if isinstance(self.server, socketserver.UnixStreamServer) else "tcp",
            "method": method,
            "path": u.path,
            "query": {k: v[0] for k, v in parse_qs(u.query).items()},
            "auth": self.headers.get("Authorization", ""),
            "body": json.loads(body) if body else None,
        }
        with open(os.path.join(work, "req-%03d.json" % i), "w") as f:
            json.dump(rec, f)
        return rec

    def answer(self, rec):
        m = mode()
        if rec["path"] != "/cli/agent/identity":
            return self.reply({"error": "stub: unknown " + rec["path"]}, 404)
        if m == "forbidden":
            return self.reply({"error": "agent_cannot_rename",
                               "message": "agents can't rename themselves; ask your owner"}, 403)
        if m == "missing":
            return self.reply({"error": "workspace_not_found", "message": "no workspace \"zz\""}, 404)
        if m == "limited":
            return self.reply({"error": "rate_limited", "retryAfterSecs": 42}, 429)
        v = dict(VIEW)
        if rec["method"] == "POST":
            b = rec["body"] or {}
            if "color" in b:
                v["color"] = b["color"]
            if "image" in b:
                v["image"] = b["image"]
            v["ok"] = True
            v["changed"] = [k for k in ("displayName", "color", "image") if k in b]
        return self.reply(v)

    def do_GET(self):
        self.answer(self.record("GET", b""))

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.answer(self.record("POST", body))

    def log_message(self, *a):
        pass

class UServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    def get_request(self):
        req, _ = super().get_request()
        return req, ("uds", 0)

tcp = HTTPServer(("127.0.0.1", 0), H)
uds = UServer(sock, H)
threading.Thread(target=uds.serve_forever, daemon=True).start()
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(tcp.server_address[1]))
tcp.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID"
for _ in $(seq 1 50); do [ -f "$WORK/stub.port" ] && [ -S "$SOCK" ] && break; sleep 0.1; done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub never wrote its port" >&2; exit 1; }
[ -S "$SOCK" ] || { echo "FAIL: stub never bound its cell socket" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/stub.port")"

OWNER="owner-tok-identity-1111"
PASSPORT="sess42.passport-secret-2222"
mkdir -p "$HOME/.k2"
printf '%s' "$OWNER" > "$HOME/.k2/heartbeat.token"

PASS=0
FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
last_req() { ls -1 "$WORK"/req-*.json 2>/dev/null | sort | tail -1; }
req_count() { ls -1 "$WORK"/req-*.json 2>/dev/null | wc -l | tr -d ' '; }
field() { python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(eval(sys.argv[2], {"r": r}))' "$(last_req)" "$1"; }
# Owner terminal: no cell socket, the owner token.
owner_k2() { env -u K2_HOOK_SOCK K2_HOOK_TOKEN="$OWNER" "$K2_CLI" "$@"; }
# Agent session: cell socket + passport (owner token still on disk).
agent_k2() { env K2_HOOK_SOCK="$SOCK" K2_HOOK_TOKEN="$PASSPORT" "$K2_CLI" "$@"; }

# ── 1. Tool policy ─────────────────────────────────────────────────────
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
[ "$(_cli_tool_id_for_verb agent identity)" = "agent-identity" ] && ok "1: agent identity tool id" || bad "1: agent identity tool id"
[ "$(_cli_tool_id_for_verb workspace set-agent-identity)" = "agent-identity" ] && ok "1: alias tool id" || bad "1: alias tool id"
[ "$(_cli_tool_id_for_verb workspace list)" = "" ] && ok "1: other workspace verbs stay ungoverned" || bad "1: workspace list mapped"
[ "$(_cli_tool_id_for_verb agent hire)" = "" ] && ok "1: agent hire unchanged" || bad "1: agent hire mapped"
[ "$(_cli_tool_default_mode agent-identity)" = "id" ] && ok "1: id tool" || bad "1: not an id tool"
_cli_tool_is_locked agent-identity && ok "1: locked" || bad "1: not locked"

# ── 2. Owner terminal: POST with the owner token ──────────────────────
out="$(owner_k2 agent identity docs --color 'rgb(34, 197, 94)' --name 'Doc Bot' 2>&1)" || bad "2: exit non-zero: $out"
got="$(field 'r["transport"], r["method"], r["path"], r["query"].get("token"), r["body"]')"
want="('tcp', 'POST', '/cli/agent/identity', '$OWNER', {'workspace': 'docs', 'displayName': 'Doc Bot', 'color': '#22c55e'})"
[ "$got" = "$want" ] && ok "2: owner POST body + token" || bad "2: got [$got] want [$want]"
grep -qF 'Agent docs: displayName, color updated' <<<"$out" && ok "2: change summary" || bad "2: output: $out"
owner_k2 agent identity docs --color '1,2,3' >/dev/null 2>&1 || bad "2: r,g,b exit non-zero"
[ "$(field 'r["body"]["color"]')" = "#010203" ] && ok "2: r,g,b → hex" || bad "2: r,g,b: $(field 'r["body"]')"
owner_k2 agent identity docs --color '#ABC' >/dev/null 2>&1 || bad "2: hex exit non-zero"
[ "$(field 'r["body"]["color"]')" = "#ABC" ] && ok "2: hex passes through (the daemon checks it)" || bad "2: hex: $(field 'r["body"]')"
# A path is sent absolute.
mkdir -p "$WORK/ws/scout"
(cd "$WORK/ws" && owner_k2 agent identity ./scout --clear-image >/dev/null 2>&1) || bad "2: path exit non-zero"
[ "$(field 'r["body"]')" = "{'workspace': '$(cd "$WORK/ws/scout" && pwd)', 'image': None}" ] && ok "2: path absolute + clear-image null" \
    || bad "2: path body: $(field 'r["body"]')"

# ── 3. Show ────────────────────────────────────────────────────────────
out="$(owner_k2 agent identity docs 2>&1)" || bad "3: exit non-zero: $out"
got="$(field 'r["method"], r["query"].get("workspace"), r["body"]')"
[ "$got" = "('GET', 'docs', None)" ] && ok "3: show is a GET" || bad "3: got [$got]"
grep -qF '  name:    Docs' <<<"$out" && grep -qF '  color:   #22c55e' <<<"$out" && grep -qF '  picture: none' <<<"$out" \
    && ok "3: show prints the look" || bad "3: output: $out"

# ── 4. Pictures ────────────────────────────────────────────────────────
python3 -c 'import base64,sys; open(sys.argv[1],"wb").write(base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="))' "$WORK/me.png"
owner_k2 agent identity docs --image "$WORK/me.png" >/dev/null 2>&1 || bad "4: png exit non-zero"
[ "$(field 'r["body"]["image"][:22]')" = "data:image/png;base64," ] && ok "4: png data URL" || bad "4: image: $(field 'r["body"]')"
before="$(req_count)"
printf '<svg xmlns="http://www.w3.org/2000/svg"/>' > "$WORK/me.svg"
set +e
out="$(owner_k2 agent identity docs --image "$WORK/me.svg" 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] && grep -qF 'PNG, JPEG or WebP' <<<"$out" && ok "4: svg refused locally (exit 2)" || bad "4: svg rc=$rc out=$out"
set +e
out="$(owner_k2 agent identity docs --image "$WORK/me.png" --clear-image 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] && ok "4: --image with --clear-image is exit 2" || bad "4: both rc=$rc out=$out"
set +e
out="$(owner_k2 agent identity docs --image "$WORK/nope.png" 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] && ok "4: missing file is exit 2" || bad "4: missing rc=$rc out=$out"
[ "$(req_count)" = "$before" ] && ok "4: usage errors never reach the daemon" || bad "4: requests sent"

# ── 5. Agent session: passport over the cell socket ───────────────────
out="$(agent_k2 agent identity --color '#123456' 2>&1)" || bad "5: exit non-zero: $out"
got="$(field 'r["transport"], r["method"], r["auth"], "token" in r["query"], r["body"]')"
want="('uds', 'POST', 'Bearer $PASSPORT', False, {'color': '#123456'})"
[ "$got" = "$want" ] && ok "5: passport on the cell socket, no workspace" || bad "5: got [$got] want [$want]"
grep -qF "$OWNER" "$(last_req)" && bad "5: owner token sent from an agent session" || ok "5: owner token never sent"
agent_k2 agent identity >/dev/null 2>&1 || bad "5: show exit non-zero"
got="$(field 'r["transport"], r["method"], r["query"]')"
[ "$got" = "('uds', 'GET', {})" ] && ok "5: agent show names no workspace" || bad "5: show got [$got]"

# ── 6. Daemon refusals ─────────────────────────────────────────────────
echo forbidden > "$WORK/mode"
set +e
out="$(agent_k2 agent identity --name 'Me' 2>&1)"; rc=$?
set -e
[ "$rc" = 3 ] && grep -qF "ask your owner" <<<"$out" && ok "6: 403 → exit 3 with the hint" || bad "6: 403 rc=$rc out=$out"
echo missing > "$WORK/mode"
set +e
out="$(owner_k2 agent identity zz 2>&1)"; rc=$?
set -e
[ "$rc" = 4 ] && ok "6: 404 → exit 4" || bad "6: 404 rc=$rc out=$out"
echo limited > "$WORK/mode"
set +e
out="$(agent_k2 agent identity --color '#000000' 2>&1)"; rc=$?
set -e
[ "$rc" = 1 ] && grep -qF "try again in 42s" <<<"$out" && ok "6: 429 → exit 1, try again" || bad "6: 429 rc=$rc out=$out"
echo ok > "$WORK/mode"

# ── 7. The alias ───────────────────────────────────────────────────────
out="$(owner_k2 workspace set-agent-identity docs --color '#abcdef' 2>"$WORK/alias.err")" || bad "7: alias exit non-zero"
got="$(field 'r["method"], r["path"], r["body"]')"
[ "$got" = "('POST', '/cli/agent/identity', {'workspace': 'docs', 'color': '#abcdef'})" ] && ok "7: alias sends the same request" || bad "7: got [$got]"
grep -qF "is now 'k2 agent identity'" "$WORK/alias.err" && ok "7: alias note on stderr" || bad "7: stderr: $(cat "$WORK/alias.err")"
grep -qF "Agent docs: color updated" <<<"$out" && ok "7: alias stdout unchanged in shape" || bad "7: stdout: $out"
agent_k2 workspace set-agent-identity --color '#fedcba' >/dev/null 2>&1 || bad "7: alias in an agent session"
[ "$(field 'r["transport"], r["auth"]')" = "('uds', 'Bearer $PASSPORT')" ] && ok "7: alias sends the passport in an agent session" \
    || bad "7: alias agent transport: $(field 'r["transport"], r["auth"]')"

# ── 8. --json and teaching surfaces ───────────────────────────────────
out="$(owner_k2 agent identity docs --json 2>&1)" || bad "8: --json exit non-zero"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["handle"]=="docs" and "canEditIdentity" in d' "$out" \
    && ok "8: --json prints the daemon JSON" || bad "8: --json: $out"
"$K2_CLI" --schema | python3 -c 'import json,sys; d=json.load(sys.stdin); c=[x for x in d["commands"] if x["name"]=="agent identity"]; assert c and "--clear-image" in c[0]["usage"]' \
    && ok "8: --schema lists agent identity" || bad "8: schema"
owner_k2 agent --help | grep -q '^  identity \[<name>\]' && ok "8: k2 agent --help lists identity" || bad "8: agent help"
owner_k2 agent identity --help | grep -qF "agents can't rename themselves" && ok "8: identity --help" || bad "8: identity help"
"$K2_CLI" study identity | grep -qF "k2 agent identity --color" && ok "8: k2 study identity teaches it" || bad "8: study"

echo
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
