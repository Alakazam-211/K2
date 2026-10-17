#!/usr/bin/env bash
# k2 hostchat (Sew S29 K14) — the CLI over /cli/sew/*, against a stub daemon.
#
# Covers:
#   1. help without a daemon; the tool catalog (hostchat = id + locked, so a
#      K2 terminal sends its passport, never the owner token).
#   2. Request shapes: enable --identity standalone --voice --voice-url sends
#      {identity, voice:{publicUrl,...}} and waits on /cli/sew/status;
#      media install/url/probe/renew/uninstall; config --public-url (setup
#      links follow it; --setup-url is refused before any request);
#      users owner/invite/reset/revoke/reactivate/role/list.
#   3. A setup link prints to stdout only (never stderr); --json prints raw.
#   4. Exit codes: 400 → 2, 403/409 → 3, usage errors → 2 before any request.
#   5. Person-only steps refuse inside a K2 terminal before any request.
#   6. A release without a Sew pin: enable/preflight/upgrade/media install
#      exit 3 with sew_not_released; status says so plainly, exit 0.
# Fake values only (example.test, made-up ids). Never touches the real ~/.k2.

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

WORK="$(mktemp -d -t k2-hostchat-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

# ── 1. no daemon ─────────────────────────────────────────────────────
echo "== help + catalog (no daemon) =="
HOME1="$WORK/home1"
mkdir -p "$HOME1"
set +e
out="$(env HOME="$HOME1" K2_PORT=1 "$K2_CLI" hostchat --help 2>&1)"; rc=$?
set -e
assert_eq "hostchat --help exit 0" "$rc" "0"
assert_contains "help: identity flag" "$out" "--identity k2|standalone"
assert_contains "help: identity is fixed" "$out" "changing it later means a fresh install"
assert_contains "help: voice url" "$out" "k2 hostchat media url <wss://HOST>"
assert_contains "help: users invite" "$out" "k2 hostchat users invite <name|email>"
assert_contains "help: manage sew" "$out" "chat_manage on"
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "catalog: hostchat tool id" "$(_cli_tool_id_for_verb hostchat)" "hostchat"
assert_eq "catalog: hostchat is an id tool" "$(_cli_tool_default_mode hostchat)" "id"
if _cli_tool_is_locked hostchat; then ok "catalog: hostchat is locked"; else bad "catalog: hostchat is locked"; fi
schema="$(env HOME="$HOME1" "$K2_CLI" --schema)"
python3 -c '
import json, sys
names = {c["name"] for c in json.loads(sys.argv[1])["commands"]}
for n in ("hostchat", "hostchat enable", "hostchat media", "hostchat users", "hostchat config"):
    assert n in names, n
' "$schema" && ok "schema lists the hostchat verbs" || bad "schema lists the hostchat verbs"

# ── 2. stub daemon ───────────────────────────────────────────────────
echo "== stub daemon =="
OWNER="owner-disk-token-hostchat"
SCOPED="sess42.scoped-secret-hostchat"
LINK="https://hearth.box.example.test/setup#token=sewpw_ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ"
HOME2="$WORK/home2"
mkdir -p "$HOME2/.k2"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" "$SCOPED" "$LINK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, scoped, link = sys.argv[1], sys.argv[2], sys.argv[3]
state = {"polls": 0}

def user(u, role="member"):
    return {"ok": True, "memberId": "48831765147549703", "username": u, "installRole": role, "deactivated": False,
            "created": True, "changed": True, "setupLink": {"url": link, "expiresAt": "2026-10-17T12:00:00Z"}}

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
        b = json.loads(body) if body else {}
        p = parsed.path
        status, out = 200, {"ok": True}
        unreleased = os.path.exists(os.path.join(work, "unreleased"))
        msg = "Sew isn't available in this K2 release yet. It will be turned on in a later K2 update."
        if unreleased and p == "/cli/sew/status":
            out = {"ok": True, "supported": True, "state": "not-installed", "released": False, "notice": msg}
        elif unreleased and p in ("/cli/sew/server/enable", "/cli/sew/server/upgrade", "/cli/sew/preflight",
                                  "/cli/sew/media/install", "/cli/sew/media/preflight"):
            status, out = 409, {"ok": False, "error": {"code": "sew_not_released", "hint": msg}}
        elif p == "/cli/sew/status":
            state["polls"] += 1
            done = state["polls"] >= 2
            out = {"ok": True, "supported": True, "state": "running" if done else "installing", "identityProvider": "standalone",
                   "installedVersion": "0.1.0", "installId": "48831765147549701", "alarms": [], "warnings": [],
                   "enableProgress": {"current": None if done else "install"},
                   "voice": {"state": "running" if done else "installing", "publicUrl": "wss://hearth.box.example.test",
                             "turnHost": "turn-box.k2.dev", "nodeIp": "203.0.113.7", "cert": {"daysLeft": 89},
                             "turnTls": {"served": "ok"}, "progress": {"result": "done" if done else "running", "step": None if done else "turn-cert"}}}
        elif p == "/cli/sew/server/enable":
            if b.get("identity") not in (None, "standalone", "k2"):
                status, out = 400, {"ok": False, "error": {"code": "bad_request", "hint": "identity is k2 or standalone"}}
            else:
                out = {"ok": True, "state": "installing", "voice": "voice" in b}
        elif p == "/cli/sew/media/install":
            out = {"ok": True, "voice": "installing"}
        elif p == "/cli/sew/media/url":
            if not b.get("publicUrl", "").startswith("wss://"):
                status, out = 400, {"ok": False, "error": {"code": "bad_request", "hint": "voice URL must be wss://<host>"}}
            elif "evil" in b["publicUrl"]:
                status, out = 400, {"ok": False, "error": {"code": "url_not_on_this_box", "hint": "not on this box"}}
            else:
                out = {"ok": True, "publicUrl": b["publicUrl"], "applied": True, "changed": True}
        elif p == "/cli/sew/server/config":
            out = {"ok": True, "set": {k: v for k, v in b.items()}, "restarted": True}
        elif p.startswith("/cli/sew/users/"):
            verb = p.rsplit("/", 1)[1]
            if token == scoped and b.get("user") == "locked":
                status, out = 403, {"ok": False, "error": {"code": "chat_manage_off", "hint": "\"Manage Sew\" is off"}}
            elif verb == "revoke":
                out = {"ok": True, "username": b["user"], "sessionsRevoked": 2}
            elif verb == "invite" and b.get("user") == "taken":
                status, out = 409, {"ok": False, "error": {"code": "username_taken", "hint": "that username is taken"}}
            else:
                out = user(b.get("user"), b.get("role") or ("owner" if verb == "owner" else "member"))
        elif p == "/cli/sew/users":
            out = {"ok": True, "users": [{"username": "rosson", "installRole": "owner", "kind": "person", "displayName": "Rosson"},
                                         {"username": "bob", "installRole": "member", "kind": "person", "deactivated": True}]}
        elif p.startswith("/cli/sew/media/") or p.startswith("/cli/sew/server/"):
            out = {"ok": True, "echo": b}
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

owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@"; }
cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
# Requests to /cli/sew/* (a K2 terminal's CLI also asks /cli/whoami).
nreq() { if [ -f "$WORK/reqs.jsonl" ]; then grep -c '"path": "/cli/sew/' "$WORK/reqs.jsonl" || true; else echo 0; fi; }
req_where() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
rows = [r for r in rows if r["path"] == sys.argv[2]]
r = rows[-1]
print(r[sys.argv[3]] if sys.argv[3] != "body" else json.dumps(json.loads(r["body"] or "{}"), sort_keys=True))
' "$WORK/reqs.jsonl" "$1" "$2"
}
run() {
    set +e
    out="$("$@" 2>"$WORK/err")"; rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
mkdir -p "$WORK/proj"
cd "$WORK"
touch sew.tar.gz sew-media.tar.gz

echo "== enable standalone + voice =="
run owner hostchat enable --tarball sew.tar.gz --identity standalone --voice-url wss://hearth.box.example.test --media-tarball sew-media.tar.gz
assert_eq "enable exit 0" "$rc" "0"
assert_eq "enable is a POST with the owner token" "$(req_where /cli/sew/server/enable method)/$(req_where /cli/sew/server/enable token)" "POST/$OWNER"
body="$(req_where /cli/sew/server/enable body)"
assert_contains "enable: identity standalone" "$body" '"identity": "standalone"'
assert_contains "enable: voice url" "$body" '"publicUrl": "wss://hearth.box.example.test"'
assert_contains "enable: media tarball absolute" "$body" "\"mediaTarballPath\": \"$WORK/sew-media.tar.gz\""
assert_contains "enable: tarball absolute" "$body" "\"tarballPath\": \"$WORK/sew.tar.gz\""
assert_contains "enable: identity is fixed (stderr)" "$err" "fixed for this install"
assert_contains "enable waited for status" "$out" "running"
assert_contains "enable: voice summary" "$out" "wss://hearth.box.example.test"
assert_contains "enable: turn line" "$out" "turn-box.k2.dev:5349"

run owner hostchat enable --identity ldap
assert_eq "bad identity → exit 2" "$rc" "2"

echo "== voice =="
run owner hostchat media install --url wss://hearth.box.example.test --node-ip 203.0.113.7 --tarball sew-media.tar.gz --no-wait
assert_eq "media install exit 0" "$rc" "0"
assert_eq "media install body" "$(req_where /cli/sew/media/install body)" \
    "{\"mediaTarballPath\": \"$WORK/sew-media.tar.gz\", \"nodeIp\": \"203.0.113.7\", \"publicUrl\": \"wss://hearth.box.example.test\"}"
run owner hostchat media url wss://hearth.box.example.test
assert_eq "media url exit 0" "$rc" "0"
assert_eq "media url body" "$(req_where /cli/sew/media/url body)" '{"publicUrl": "wss://hearth.box.example.test"}'
run owner hostchat media url wss://evil.example
assert_eq "foreign voice url → exit 2" "$rc" "2"
assert_contains "foreign voice url says why" "$err" "url_not_on_this_box"
before="$(nreq)"
run owner hostchat media probe --udp ok --tcp ok
assert_eq "probe without --turn-tls → exit 2" "$rc" "2"
assert_eq "probe usage sends nothing" "$(nreq)" "$before"
run owner hostchat media probe --udp ok --tcp fail --turn-tls ok
assert_eq "probe exit 0" "$rc" "0"
assert_eq "probe body" "$(req_where /cli/sew/media/probe body)" '{"tcp": "fail", "turnTls": "ok", "udp": "ok"}'
run owner hostchat media renew
assert_eq "renew is a POST" "$(req_where /cli/sew/media/renew method)" "POST"
run owner hostchat media uninstall
assert_eq "media uninstall is a POST" "$(req_where /cli/sew/media/uninstall method)" "POST"

echo "== config =="
run owner hostchat config --public-url https://hearth.box.example.test
assert_eq "config exit 0" "$rc" "0"
assert_eq "config body" "$(req_where /cli/sew/server/config body)" '{"publicUrl": "https://hearth.box.example.test"}'
before="$(nreq)"
run owner hostchat config --setup-url https://hearth.box.example.test/setup
assert_eq "--setup-url → exit 2" "$rc" "2"
assert_contains "--setup-url says links follow the public URL" "$err" "<public-url>/setup"
assert_eq "--setup-url sends nothing" "$(nreq)" "$before"
run owner hostchat config
assert_eq "config without a key → exit 2" "$rc" "2"

echo "== users =="
run owner hostchat users owner rosson --display-name "Rosson Long"
assert_eq "users owner exit 0" "$rc" "0"
assert_eq "owner body" "$(req_where /cli/sew/users/owner body)" '{"displayName": "Rosson Long", "user": "rosson"}'
assert_contains "the setup link prints to stdout" "$out" "$LINK"
assert_absent "the setup link never reaches stderr" "$err" "sewpw_"
run owner hostchat users invite ana@acme.example.test --admin
assert_eq "invite body" "$(req_where /cli/sew/users/invite body)" '{"role": "admin", "user": "ana@acme.example.test"}'
assert_contains "invite: link on stdout" "$out" "$LINK"
run owner hostchat users invite bob --role guest --email bob@acme.example.test --json
assert_eq "invite --json exit 0" "$rc" "0"
assert_eq "invite --json is the raw body" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["setupLink"]["url"])')" "$LINK"
assert_eq "invite with role + email" "$(req_where /cli/sew/users/invite body)" '{"email": "bob@acme.example.test", "role": "guest", "user": "bob"}'
run owner hostchat users invite taken
assert_eq "username_taken → exit 3" "$rc" "3"
assert_contains "username_taken says so" "$err" "username_taken"
run owner hostchat users reset bob
assert_contains "reset prints a new link" "$out" "$LINK"
run owner hostchat users revoke bob
assert_contains "revoke reports ended sessions" "$out" "sessions ended:  2"
run owner hostchat users reactivate bob
assert_eq "reactivate body" "$(req_where /cli/sew/users/reactivate body)" '{"user": "bob"}'
run owner hostchat users role bob admin
assert_eq "role body" "$(req_where /cli/sew/users/role body)" '{"role": "admin", "user": "bob"}'
before="$(nreq)"
run owner hostchat users role bob
assert_eq "role without a role → exit 2" "$rc" "2"
run owner hostchat users invite
assert_eq "invite without a name → exit 2" "$rc" "2"
assert_eq "usage errors send nothing" "$(nreq)" "$before"
run owner hostchat users list
assert_contains "list shows users" "$out" "rosson"
assert_contains "list marks deactivated" "$out" "(deactivated)"

echo "== K2 terminal (agent) =="
run cell hostchat users invite locked
assert_eq "agent: refusal → exit 3" "$rc" "3"
assert_eq "agent sends its passport, never the owner token" "$(req_where /cli/sew/users/invite token)" "$SCOPED"
assert_contains "agent: Manage Sew hint" "$err" "chat_manage_off"
run cell hostchat media url wss://hearth.box.example.test
assert_eq "agent media url uses the passport" "$(req_where /cli/sew/media/url token)" "$SCOPED"
before="$(nreq)"
run cell hostchat recovery-key
assert_eq "agent: recovery key refused (exit 3)" "$rc" "3"
assert_contains "agent: person only" "$err" "person_only"
run cell hostchat purge --confirm-hostname box
assert_eq "agent: purge refused (exit 3)" "$rc" "3"
run cell hostchat rotate-master --drop-prev
assert_eq "agent: drop-prev refused (exit 3)" "$rc" "3"
assert_eq "person-only refusals send nothing" "$(nreq)" "$before"

echo "== release without a Sew pin =="
touch "$WORK/unreleased"
NR="Sew isn't available in this K2 release yet. It will be turned on in a later K2 update."
run owner hostchat status
assert_eq "unreleased: status exit 0" "$rc" "0"
assert_eq "unreleased: status says so plainly" "$out" "Sew: $NR"
for verb in "enable --tarball sew.tar.gz" "preflight" "upgrade --tarball sew.tar.gz" "media install --tarball sew-media.tar.gz --no-wait" "media preflight"; do
    # shellcheck disable=SC2086
    run owner hostchat $verb
    assert_eq "unreleased: $verb → exit 3" "$rc" "3"
    assert_contains "unreleased: $verb says sew_not_released" "$err" '"error": "sew_not_released"'
    assert_contains "unreleased: $verb gives the message" "$err" "$NR"
done
rm -f "$WORK/unreleased"

echo
echo "hostchat CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
