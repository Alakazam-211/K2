#!/usr/bin/env bash
# k2 llm accounts — the LLM login wallet CLI.
#
#   1. No daemon: help text; usage errors exit 2; the tool catalog maps
#      `llm` to the locked `id` tool `llm-accounts`; schema and daily help
#      list it.
#   2. A stub daemon: list (human + --json passthrough), status, usage;
#      switch / next / rename / remove are POSTs with JSON bodies; the owner
#      (outside K2) sends the disk owner token; a K2 terminal (fake cell)
#      sends the SCOPED passport, never the owner token; 403 owner_only →
#      exit 3 with the hint; 404 → 4; 409 → 1.
#   3. The add flow: the stub reports the sign-in page and a device code,
#      then asks for a pasted code; the code is read from stdin, reaches the
#      stub's login/input, never appears in the CLI's output; signed_in →
#      exit 0 with the "make it active" offer.
#   4. Pins + API keys: pin / unpin bodies (scope, scopeId, tool, id), the
#      pins listing, list shows "pinned to" and "billed per token";
#      add-key reads the key from stdin only — it reaches the stub in the
#      JSON body and never appears in the CLI's output or any child's argv;
#      a key given as an argument is refused before anything is sent;
#      a K2 terminal gets 403 owner_only → exit 3; 409 pinned_active → 1.
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

WORK="$(mktemp -d -t k2-llm-accounts-cli-XXXXXX)"
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

capture_nod llm accounts --help
assert_eq "llm accounts --help exit 0" "$rc" "0"
assert_contains "help: list" "$out" "k2 llm accounts list [--tool <tool>] [--json]"
assert_contains "help: add" "$out" "k2 llm accounts add <tool> <label> [--device]"
assert_contains "help: switch" "$out" "k2 llm accounts switch <tool> <label>"
assert_contains "help: next" "$out" "k2 llm accounts next <tool>"
assert_contains "help: every session" "$out" "Switching a login affects every"
assert_contains "help: read-only" "$out" "Agents and K2 terminals are read-only"
assert_contains "help: study" "$out" "k2 study llm-accounts"
set +e
out="$(env HOME="$HOME1" "$K2_CLI" llm accounts --help 2>&1)"; rc=$?
set -e
assert_eq "help needs no daemon" "$rc" "0"
set +e
out="$(env HOME="$HOME1" "$K2_CLI" llm 2>&1)"; rc=$?
set -e
assert_eq "bare k2 llm prints help, no daemon" "$rc" "0"
assert_contains "bare k2 llm help text" "$out" "LLM login wallet"
capture_nod help llm
assert_contains "k2 help llm" "$out" "k2 llm accounts list"

capture_nod llm accounts frobnicate
assert_eq "unknown verb exit 2" "$rc" "2"
capture_nod llm frobnicate
assert_eq "unknown llm noun exit 2" "$rc" "2"
capture_nod llm accounts add claude
assert_eq "add without a label exit 2" "$rc" "2"
capture_nod llm accounts add gemini work
assert_eq "add on a tool without a wallet exit 2" "$rc" "2"
assert_contains "says which tools" "$out" "claude, codex, grok"
capture_nod llm accounts add nosuch work
assert_eq "unknown tool exit 2" "$rc" "2"
capture_nod llm accounts switch claude
assert_eq "switch without a label exit 2" "$rc" "2"
capture_nod llm accounts list --bogus
assert_eq "unknown flag exit 2" "$rc" "2"
capture_nod llm accounts rename claude a
assert_eq "rename without new label exit 2" "$rc" "2"

eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "catalog: llm tool id" "$(_cli_tool_id_for_verb llm)" "llm-accounts"
assert_eq "catalog: llm-accounts is an id tool" "$(_cli_tool_default_mode llm-accounts)" "id"
if _cli_tool_is_locked llm-accounts; then ok "catalog: llm-accounts is locked"; else bad "catalog: llm-accounts is locked"; fi

schema="$(env HOME="$HOME1" "$K2_CLI" --schema)"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
names = {c["name"] for c in d["commands"]}
for n in ("llm accounts list", "llm accounts status", "llm accounts usage", "llm accounts add", "llm accounts switch", "llm accounts next"):
    assert n in names, n
' "$schema" && ok "schema lists llm accounts verbs" || bad "schema llm accounts verbs"
help_daily="$(sed -n '/^cmd_help_v2_daily() {/,/^}/p' "$K2_CLI")"
assert_contains "daily help lists llm accounts" "$help_daily" "llm accounts list"

# ── 2. stub daemon ───────────────────────────────────────────────────
echo "== stub daemon =="
OWNER="owner-disk-token-llmacc"
SCOPED="sess77.scoped-secret-llmacc"
PASTED="PASTE-ME-4242-CODE"
HOME2="$WORK/home2"
mkdir -p "$HOME2/.k2"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" "$SCOPED" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, scoped = sys.argv[1], sys.argv[2]
POST_ONLY = {"add", "login", "login/input", "login/cancel", "switch", "next", "rename", "remove", "refresh",
             "pin", "unpin", "add-key"}
state = {"polls": 0, "input": None}

def acct(i, label, active, st="signed_in", tool="claude"):
    return {"id": i, "tool": tool, "label": label, "active": active, "state": st, "detail": None,
            "email": "person@example.test", "org": None, "plan": "max", "expiresAt": None,
            "refreshedAt": None, "lastUsedAt": None, "createdAt": 1, "createdBy": "owner",
            "usage": {"harness": tool, "plan": "max", "windows": [{"label": "Session", "used": 0.42, "resetsAt": "2026-10-07T12:00:00Z"},
                                                               {"label": "Weekly", "used": 0.1, "resetsAt": "2026-10-10T12:00:00Z"}],
                      "checkedAt": "2026-10-07T10:00:00Z", "status": ""} if active else None,
            "usageCheckedAt": None}

LIST = {"tools": [
    {"tool": "claude", "display": "Claude", "supported": True, "activeId": "acc_work", "loginMethod": "temp_home",
     "accounts": [acct("acc_work", "work", True), acct("acc_personal", "personal", False)]},
    {"tool": "codex", "display": "Codex", "supported": True, "activeId": None, "loginMethod": "temp_home", "accounts": []},
    {"tool": "grok", "display": "Grok", "supported": True, "activeId": None, "loginMethod": "temp_home", "accounts": []},
    {"tool": "gemini", "display": "Gemini", "supported": True, "subscription": False, "apiKeys": True, "activeId": None,
     "loginMethod": None, "accounts": []},
    {"tool": "cursor", "display": "Cursor Agent", "supported": False, "subscription": False, "apiKeys": False,
     "activeId": None, "loginMethod": None, "accounts": []},
], "logins": [], "airgap": False, "switchNote": "Switching a login affects every unpinned session on this server."}
KEY_ACCT = dict(acct("acc_key", "metered", False), kind="api_key", billedPerToken=True, usage=None)
PIN_WS = {"scopeKind": "workspace", "scopeId": "proj-1", "tool": "claude", "accountId": "acc_personal", "label": "builder"}
LIST["tools"][0]["accounts"][1]["pinnedTo"] = [PIN_WS]
LIST["tools"][0]["accounts"][1]["inUse"] = True
LIST["tools"][0]["accounts"].append(KEY_ACCT)
LIST["tools"][0]["pins"] = [PIN_WS]

def login(st, **kw):
    base = {"loginId": "login_1", "accountId": "acc_new", "tool": "claude", "label": "fresh", "mode": "other_device",
            "method": "temp_home", "state": st, "url": None, "code": None, "error": None, "screen": [],
            "startedAt": 1, "done": False, "banner": None, "offerMakeActive": False}
    base.update(kw)
    return {"login": base}

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
        verb = parsed.path[len("/cli/llm/accounts/"):] if parsed.path.startswith("/cli/llm/accounts/") else None
        b = json.loads(body) if body else {}
        status, out = 200, {}
        if verb is None:
            status, out = 404, {"error": "route not found"}
        elif verb in POST_ONLY and method != "POST":
            status, out = 405, {"error": "POST required"}
        elif verb in POST_ONLY and token == scoped:
            status, out = 403, {"error": {"code": "owner_only", "hint": "Change logins on Settings → LLMs, or run k2 from a terminal outside K2."}}
        elif verb == "list":
            out = LIST
        elif verb == "status":
            key = rec["query"].get("id")
            if key in ("work", "acc_work"):
                out = {"account": acct("acc_work", "work", True)}
            else:
                status, out = 404, {"error": {"code": "not_found", "hint": "no such login: " + str(key)}}
        elif verb == "usage":
            out = {"rows": [{"accountId": "acc_work", "tool": "claude", "label": "work", "active": True,
                             "usage": acct("acc_work", "work", True)["usage"]}]}
        elif verb == "switch":
            out = {"switch": {"tool": "claude", "from": "acc_work", "to": b.get("id"), "savedOutgoing": True,
                              "refreshed": False, "alreadyActive": False}, "account": acct(b.get("id"), "personal", True)}
        elif verb == "next":
            if b.get("tool") == "grok":
                status, out = 404, {"error": {"code": "no_next", "hint": "no other signed-in grok login"}}
            else:
                out = {"switch": {"tool": "claude", "from": "acc_work", "to": "acc_personal", "savedOutgoing": True,
                                  "refreshed": False, "alreadyActive": False}, "account": acct("acc_personal", "personal", True)}
        elif verb == "rename":
            if b.get("label") == "dup":
                status, out = 409, {"error": {"code": "duplicate_label", "hint": "a login named \"dup\" already exists"}}
            else:
                out = {"account": acct(b.get("id"), b.get("label"), False)}
        elif verb == "remove":
            out = {"removed": {"id": b.get("id"), "movedTo": "/tmp/x"}}
        elif verb == "refresh":
            out = {"accounts": [acct("acc_work", "work", True)]}
        elif verb in ("add", "login"):
            state["polls"] = 0
            state["input"] = None
            out = {"account": acct("acc_new", "fresh", False, "signing_in"), "login": login("signing_in")["login"]}
        elif verb == "login/status":
            state["polls"] += 1
            if state["input"] is not None:
                out = login("signed_in", done=True, offerMakeActive=True)
            elif state["polls"] == 1:
                out = login("waiting_for_browser", url="https://signin.example.test/authorize?x=1", code="WXYZ-1234")
            else:
                out = login("waiting_for_code", url="https://signin.example.test/authorize?x=1", code="WXYZ-1234",
                            screen=["Paste code here if prompted >"])
        elif verb == "login/input":
            state["input"] = b.get("text")
            out = {"ok": True}
        elif verb == "login/cancel":
            out = login("cancelled", done=True)
        elif verb == "pins":
            out = {"pins": [PIN_WS]}
        elif verb == "pin":
            if b.get("id") == "acc_work":
                status, out = 409, {"error": {"code": "pinned_active", "hint": "work is the pool's active claude login; switch the pool first"}}
            else:
                out = {"pin": {"scopeKind": b.get("scope"), "scopeId": b.get("scopeId"), "tool": b.get("tool"),
                               "accountId": b.get("id"), "label": b.get("scopeId")},
                       "note": "This agent's Claude history will live with this login."}
        elif verb == "unpin":
            out = {"unpinned": b.get("scopeId") != "nothing-here"}
        elif verb == "add-key":
            out = {"account": dict(KEY_ACCT, tool=b.get("tool"), label=b.get("label"))}
        else:
            status, out = 404, {"error": {"code": "not_found", "hint": "no route"}}
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

# The owner outside K2: disk owner token, no cell.
owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_LLMA_POLL_SECS=0.1 "$K2_CLI" "$@"; }
# A K2 terminal (agent or shell tab): the session passport.
cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
last() { tail -n 1 "$WORK/reqs.jsonl" | python3 -c "import json,sys; print(json.load(sys.stdin)[sys.argv[1]])" "$1"; }
last_body() { tail -n 1 "$WORK/reqs.jsonl" | python3 -c "import json,sys; print(json.dumps(json.loads(json.load(sys.stdin)['body']), sort_keys=True))"; }

out="$(owner llm accounts list)"
assert_contains "list: tool heading" "$out" "Claude:"
assert_contains "list: active mark" "$out" "* work"
assert_contains "list: usage summary" "$out" "Session 42% · Weekly 10%"
assert_contains "list: idle login" "$out" "personal"
assert_contains "list: empty tool" "$out" "Codex: no logins"
assert_contains "list: api-key-only tool" "$out" "Gemini: no API keys (add one: k2 llm accounts add-key gemini <label>)"
assert_contains "list: unsupported tool" "$out" "Cursor Agent: no login wallet yet"
assert_contains "list: pinned to" "$out" "pinned to: workspace builder (in use)"
assert_contains "list: api key marker" "$out" "api key · billed per token"
assert_contains "list: every session note" "$out" "Switching a login affects every unpinned session on this server."
assert_eq "list is a GET" "$(last method)" "GET"
assert_eq "list path" "$(last path)" "/cli/llm/accounts/list"
assert_eq "owner sends the disk owner token" "$(last token)" "$OWNER"

json_out="$(owner llm accounts list --json)"
python3 -c 'import json,sys; v=json.loads(sys.argv[1]); assert v["tools"][0]["activeId"]=="acc_work", v' "$json_out" \
    && ok "list --json passes the body through" || bad "list --json passthrough"
out="$(owner llm accounts list --tool codex)"
assert_absent "list --tool filters" "$out" "Claude:"

out="$(owner llm accounts status claude work)"
assert_contains "status: label + active" "$out" "claude work (active)"
assert_contains "status: state" "$out" "signed_in"
assert_eq "status sends id" "$(tail -n 1 "$WORK/reqs.jsonl" | python3 -c 'import json,sys; print(json.load(sys.stdin)["query"]["id"])')" "work"
set +e
owner llm accounts status claude missing >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "status unknown → exit 4" "$rc" "4"
assert_contains "status 404 hint" "$(cat "$WORK/err")" "not_found"

out="$(owner llm accounts usage)"
assert_contains "usage row" "$out" "Session 42%"

out="$(owner llm accounts switch claude personal)"
assert_contains "switch says now using" "$out" "claude: now using personal"
assert_contains "switch says every session" "$out" "every unpinned session on this server"
assert_eq "switch is a POST" "$(last method)" "POST"
assert_eq "switch path" "$(last path)" "/cli/llm/accounts/switch"
assert_eq "switch body is the resolved id" "$(last_body)" '{"id": "acc_personal"}'

out="$(owner llm accounts next claude)"
assert_contains "next switches" "$out" "now using personal"
assert_eq "next body" "$(last_body)" '{"tool": "claude"}'
set +e
owner llm accounts next grok >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "next with nothing to switch to → exit 4" "$rc" "4"
assert_contains "no_next hint" "$(cat "$WORK/err")" "no_next"

owner llm accounts rename claude personal "home use" >/dev/null
assert_eq "rename body" "$(last_body)" '{"id": "acc_personal", "label": "home use"}'
set +e
owner llm accounts rename claude personal dup >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "rename duplicate → exit 1 (409)" "$rc" "1"
assert_contains "duplicate hint" "$(cat "$WORK/err")" "duplicate_label"

owner llm accounts remove claude personal >/dev/null
assert_eq "remove path" "$(last path)" "/cli/llm/accounts/remove"
assert_eq "remove body" "$(last_body)" '{"id": "acc_personal"}'
set +e
owner llm accounts remove claude nobody >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "remove unknown label → exit 4" "$rc" "4"

owner llm accounts refresh --usage >/dev/null
assert_eq "refresh body" "$(last_body)" '{"usage": true}'

# Every POST the CLI made carried a JSON body and no route was GET'd that
# is POST-only.
python3 - "$WORK/reqs.jsonl" <<'PY' && ok "no POST-only route was ever a GET" || bad "a POST-only route was sent as GET"
import json, sys
post_only = {"add", "login", "login/input", "login/cancel", "switch", "next", "rename", "remove", "refresh",
             "pin", "unpin", "add-key"}
for line in open(sys.argv[1]):
    r = json.loads(line)
    verb = r["path"][len("/cli/llm/accounts/"):]
    if verb in post_only:
        assert r["method"] == "POST", r
        json.loads(r["body"])
PY

echo "== K2 terminal: passport, read-only =="
out="$(cell llm accounts list)"
assert_contains "passport list works" "$out" "* work"
assert_eq "passport list sends the scoped token" "$(last token)" "$SCOPED"
assert_absent "never the owner token" "$(tail -n 1 "$WORK/reqs.jsonl")" "$OWNER"
set +e
cell llm accounts switch claude personal >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "passport switch → exit 3" "$rc" "3"
assert_contains "owner_only hint" "$(cat "$WORK/err")" "owner_only: Change logins on Settings → LLMs, or run k2 from a terminal outside K2."
assert_eq "passport mutation sent the scoped token" "$(last token)" "$SCOPED"
assert_absent "passport mutation never the owner token" "$(tail -n 1 "$WORK/reqs.jsonl")" "$OWNER"

# ── 3. add flow ──────────────────────────────────────────────────────
echo "== add: sign-in page, device code, pasted code =="
set +e
out="$(printf '%s\n' "$PASTED" | owner llm accounts add claude fresh --device 2>"$WORK/err")"; rc=$?
set -e
err_text="$(cat "$WORK/err")"
assert_eq "add exit 0 on signed_in" "$rc" "0"
assert_contains "prints the sign-in page" "$out" "Open this page to sign in: https://signin.example.test/authorize?x=1"
assert_contains "prints the device code" "$out" "Enter this code on that page: WXYZ-1234"
assert_eq "sign-in page printed once" "$(printf '%s\n' "$out" | grep -c 'Open this page')" "1"
assert_contains "prompts for the code" "$err_text" "Paste the code:"
assert_contains "signed in line" "$out" "Signed in: claude fresh."
assert_contains "make-active offer" "$out" "k2 llm accounts switch claude 'fresh'"
assert_absent "pasted code never in stdout" "$out" "$PASTED"
assert_absent "pasted code never in stderr" "$err_text" "$PASTED"
python3 - "$WORK/reqs.jsonl" "$PASTED" <<'PY' && ok "add body + login/input carries the pasted code" || bad "add/login input bodies"
import json, sys
reqs = [json.loads(l) for l in open(sys.argv[1])]
adds = [r for r in reqs if r["path"] == "/cli/llm/accounts/add"]
assert adds and json.loads(adds[-1]["body"]) == {"tool": "claude", "label": "fresh", "mode": "other_device"}, adds[-1:]
inputs = [r for r in reqs if r["path"] == "/cli/llm/accounts/login/input"]
assert len(inputs) == 1, inputs
assert json.loads(inputs[0]["body"]) == {"loginId": "login_1", "text": sys.argv[2]}, inputs
assert inputs[0]["method"] == "POST"
# The code never travels in a query string.
assert all(sys.argv[2] not in r["path"] and sys.argv[2] not in json.dumps(r["query"]) for r in reqs)
PY

echo "== add: no code on stdin cancels =="
set +e
out="$(owner llm accounts add claude fresh </dev/null 2>"$WORK/err")"; rc=$?
set -e
assert_eq "empty code → exit 1" "$rc" "1"
assert_contains "says cancelled" "$(cat "$WORK/err")" "cancelled"
assert_eq "cancel was posted" "$(last path)" "/cli/llm/accounts/login/cancel"
assert_eq "add without --device asks for this computer" \
    "$(grep '/cli/llm/accounts/add' "$WORK/reqs.jsonl" | tail -n 1 | python3 -c 'import json,sys; print(json.loads(json.load(sys.stdin)["body"])["mode"])')" "this_computer"

# ── 4. pins + API keys ───────────────────────────────────────────────
echo "== pins =="
capture_nod llm accounts pin claude personal
assert_eq "pin without a scope exit 2" "$rc" "2"
capture_nod llm accounts pin --workspace a --session b claude personal
assert_eq "pin with both scopes exit 2" "$rc" "2"
capture_nod llm accounts pin --workspace a claude
assert_eq "pin without a label exit 2" "$rc" "2"
capture_nod llm accounts unpin --session x
assert_eq "unpin without a tool exit 2" "$rc" "2"
capture_nod llm accounts pin --workspace a cursor x
assert_eq "pin on a tool without a wallet exit 2" "$rc" "2"

out="$(owner llm accounts pin --workspace builder claude personal)"
assert_eq "pin is a POST" "$(last method)" "POST"
assert_eq "pin path" "$(last path)" "/cli/llm/accounts/pin"
assert_eq "pin body (workspace)" "$(last_body)" '{"id": "acc_personal", "scope": "workspace", "scopeId": "builder", "tool": "claude"}'
assert_contains "pin prints the result" "$out" "pinned workspace builder claude to personal"
assert_contains "pin prints the note" "$out" "Claude history will live with this login"
owner llm accounts pin --session tab-42 claude personal >/dev/null
assert_eq "pin body (session)" "$(last_body)" '{"id": "acc_personal", "scope": "session", "scopeId": "tab-42", "tool": "claude"}'
set +e
owner llm accounts pin --workspace builder claude work >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "pin the pool's live login → exit 1 (409)" "$rc" "1"
assert_contains "pinned_active hint" "$(cat "$WORK/err")" "pinned_active: work is the pool's active claude login"

out="$(owner llm accounts unpin --workspace builder claude)"
assert_eq "unpin path" "$(last path)" "/cli/llm/accounts/unpin"
assert_eq "unpin body" "$(last_body)" '{"scope": "workspace", "scopeId": "builder", "tool": "claude"}'
assert_contains "unpin says pool" "$out" "uses the pool's active login again"
out="$(owner llm accounts unpin --session nothing-here claude)"
assert_contains "unpin with no pin" "$out" "no claude pin on session nothing-here"

out="$(owner llm accounts pins)"
assert_eq "pins is a GET" "$(last method)" "GET"
assert_contains "pins row" "$out" "workspace"
assert_contains "pins row label → login label" "$out" "builder"
assert_contains "pins row login" "$out" "-> personal"
json_out="$(owner llm accounts pins --json)"
python3 -c 'import json,sys; v=json.loads(sys.argv[1]); assert v["pins"][0]["accountId"]=="acc_personal", v' "$json_out" \
    && ok "pins --json passes the body through" || bad "pins --json passthrough"
out="$(cell llm accounts pins)"
assert_contains "pins works from a K2 terminal" "$out" "-> personal"
assert_eq "passport pins sends the scoped token" "$(grep '/cli/llm/accounts/pins' "$WORK/reqs.jsonl" | tail -n 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')" "$SCOPED"
set +e
cell llm accounts pin --workspace builder claude personal >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "passport pin → exit 3" "$rc" "3"
assert_contains "passport pin owner_only" "$(cat "$WORK/err")" "owner_only:"

echo "== add-key =="
API_KEY="sk-fake-K2TEST-0123456789abcdef"
# Log every curl / python3 argv the CLI starts.
WRAP="$WORK/wrap"
mkdir -p "$WRAP"
for prog in curl python3; do
    real="$(command -v "$prog")"
    printf '#!/bin/sh\nprintf "%%s\\n" "%s $*" >> "%s/argv.log"\nexec "%s" "$@"\n' "$prog" "$WORK" "$real" > "$WRAP/$prog"
    chmod 755 "$WRAP/$prog"
done
: > "$WORK/argv.log"
nreq_before="$(wc -l < "$WORK/reqs.jsonl")"
set +e
out="$(printf '%s\n' "$API_KEY" | env PATH="$WRAP:$PATH" HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" llm accounts add-key gemini metered 2>"$WORK/err")"; rc=$?
set -e
err_text="$(cat "$WORK/err")"
assert_eq "add-key exit 0" "$rc" "0"
assert_contains "add-key says billed per token" "$out" "billed per token"
assert_absent "key never in stdout" "$out" "$API_KEY"
assert_absent "key never in stderr" "$err_text" "$API_KEY"
[ -s "$WORK/argv.log" ] && ok "argv wrappers saw the children" || bad "argv wrappers saw nothing"
assert_absent "key never on any child's argv" "$(cat "$WORK/argv.log")" "$API_KEY"
assert_eq "add-key is a POST" "$(last method)" "POST"
assert_eq "add-key path" "$(last path)" "/cli/llm/accounts/add-key"
assert_eq "add-key body carries the key" "$(last_body)" "{\"key\": \"$API_KEY\", \"label\": \"metered\", \"tool\": \"gemini\"}"
python3 - "$WORK/reqs.jsonl" "$API_KEY" <<'PY' && ok "key only ever in the add-key JSON body" || bad "key leaked outside the add-key body"
import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    if sys.argv[2] in line:
        assert r["path"] == "/cli/llm/accounts/add-key" and r["method"] == "POST", r
        assert sys.argv[2] not in r["path"] and sys.argv[2] not in json.dumps(r["query"]) and sys.argv[2] not in r["token"], r
PY

set +e
printf '%s\n' "$API_KEY" | owner llm accounts add-key claude k2 "$API_KEY" >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "key as an argument → exit 2" "$rc" "2"
assert_contains "says stdin" "$(cat "$WORK/err")" "read from stdin, never an argument"
set +e
owner llm accounts add-key claude k2 </dev/null >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "no key on stdin → exit 2" "$rc" "2"
nreq_after="$(grep -c 'add-key' "$WORK/reqs.jsonl")"
assert_eq "refused add-keys sent nothing" "$nreq_after" "1"
capture_nod llm accounts add-key cursor x
assert_eq "add-key on a tool without keys exit 2" "$rc" "2"
set +e
printf '%s\n' "$API_KEY" | cell llm accounts add-key claude k3 >/dev/null 2>"$WORK/err"; rc=$?
set -e
assert_eq "passport add-key → exit 3" "$rc" "3"
assert_contains "passport add-key owner_only" "$(cat "$WORK/err")" "owner_only:"
assert_absent "owner_only error never echoes the key" "$(cat "$WORK/err")" "$API_KEY"
: "$nreq_before"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
