#!/usr/bin/env bash
# k2 compute git-access setup and the setup card (0.46.10): CLI shapes
# against a tiny stub daemon on loopback (python3 http.server). No real
# daemon, no vault, no keychain, no ~/.k2: HOME is a throwaway dir. Fake
# keys and tokens only.
#
# Covers: `setup --github-app --key-file` and `--github-token-stdin` post
# the value in the request BODY (never argv or the query) with the owner
# token, and render the credential without echoing it; a bare `setup` is a
# usage error (exit 2, no request) that names the card an agent asks with;
# `k2 compute git-access` with no credential names the person's command and
# the agent's card; the agent's grant refused `git_access_needs_setup`
# exits 3 and ends with the daemon's "Ask with: k2 permission request
# compute.git-access-setup …" line; the help and `k2 study compute` (GIT
# ACCESS) name the card.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2ga-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

SETUP_ASK='k2 permission request compute.git-access-setup --why "<what you need it for>"'

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]
ASK = 'k2 permission request compute.git-access-setup --why "<what you need it for>"'

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
    def log(self, method, body):
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write(json.dumps({"method": method, "path": u.path, "query": u.query, "body": body,
                                "token": (parse_qs(u.query).get("token") or [""])[0]}) + "\n")
        return u
    def do_GET(self):
        u = self.log("GET", None)
        if u.path == "/cli/compute/git-access":
            return self.reply(200, {"workspace": "builder", "credential": None, "grants": [], "note": "x"})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = self.log("POST", body)
        if u.path == "/cli/compute/git-access/setup":
            if "app_id" in body:
                return self.reply(200, {"credential": {"kind": "app", "appId": body["app_id"], "app": "acme-compute"}, "note": "1-hour, read-only tokens"})
            return self.reply(200, {"credential": {"kind": "token"}, "note": "nodes get this token itself"})
        if u.path == "/cli/compute/git-access/grant":
            return self.reply(403, {"error": {"code": "git_access_needs_setup",
                                              "hint": "this server has no GitHub credential yet, so no grant can work",
                                              "workspace": "builder", "repos": "acme/app"},
                                    "permission": {"action": "compute.git-access-setup", "params": {}, "approvers": "owner or admin", "command": ASK}})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
PORT="$(cat "$SANDBOX/port")"

# Owner-style environment: port + token from a throwaway ~/.k2, no session
# (a terminal outside K2).
export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"

last() { tail -1 "$LOG" | python3 -c "import json,sys; v=json.load(sys.stdin); x=v[sys.argv[1]]; print(x if isinstance(x, str) else json.dumps(x, sort_keys=True))" "$1"; }

PEM_BODY="MIIEvQIBADANBgkqhkiG9w0BAQEFAASCK2TESTfakePEMbodyLINE0123456789"
printf -- '-----BEGIN PRIVATE KEY-----\n%s\n-----END PRIVATE KEY-----\n' "$PEM_BODY" > "$SANDBOX/app.pem"
PAT="github_pat_11K2TESTfakeCLItoken0123456789"

echo "== setup from a terminal outside K2 (CLI parity) =="
out="$("$K2" compute git-access setup --github-app 4242 --key-file "$SANDBOX/app.pem" </dev/null)"
expect_has "App setup renders" "$out" "GitHub credential set: App 4242 (acme-compute)"
expect_not "the key is never printed" "$out" "$PEM_BODY"
expect_eq "App setup path" "$(last path)" "/cli/compute/git-access/setup"
expect_has "the key rides the body" "$(last body)" "$PEM_BODY"
expect_has "the App id rides the body" "$(last body)" '"app_id": "4242"'
expect_not "never the query" "$(last query)" "PRIVATE"
expect_eq "sent with the owner token" "$(last token)" "owner-test-token"

out="$(printf '%s\n' "$PAT" | "$K2" compute git-access setup --github-token-stdin)"
expect_has "token setup renders" "$out" "GitHub credential set: fine-grained token"
expect_not "the token is never printed" "$out" "$PAT"
expect_eq "token body" "$(last body)" "{\"token\": \"$PAT\"}"
expect_not "token never in the query" "$(last query)" "github_pat"

echo "== a bare setup names the card =="
n_before="$(wc -l < "$LOG")"
set +e
err="$("$K2" compute git-access setup 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "bare setup exits 2" "$rc" "2"
expect_eq "no request for a usage error" "$(wc -l < "$LOG")" "$n_before"
expect_has "usage names the agent's card" "$err" 'k2 permission request compute.git-access-setup --why'

echo "== no credential: the list names both ways =="
out="$("$K2" compute git-access </dev/null)"
expect_has "list: the person's command" "$out" "credential: none (a person runs: k2 compute git-access setup --github-app <id> --key-file <app.pem>;"
expect_has "list: the agent's card" "$out" "an agent asks: $SETUP_ASK"

echo "== the agent's grant before setup: exit 3, ends with the ask =="
set +e
err="$("$K2" compute git-access grant --github acme/app 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "refused grant exits 3" "$rc" "3"
expect_has "refusal code" "$err" "git_access_needs_setup"
expect_eq "it ends with the ask" "$(printf '%s\n' "$err" | tail -1)" "Ask with: $SETUP_ASK"

echo "== help and study name the card =="
help="$("$K2" compute git-access --help)"
expect_has "help: the card" "$help" 'k2 permission request compute.git-access-setup --why "…"'
expect_has "help: K2's own form" "$help" "K2's own form"
expect_has "help: Contents read-only" "$help" "Contents read-only"
expect_has "help: needs_setup" "$help" "git_access_needs_setup"
study="$("$K2" study compute)"
expect_has "study: needs_setup" "$study" "git_access_needs_setup"
expect_has "study: the card" "$study" 'k2 permission request compute.git-access-setup --why "…"'
expect_has "study: never in chat" "$study" "Never ask for a key or token in chat or on a secret card."

echo
echo "compute git-access CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
