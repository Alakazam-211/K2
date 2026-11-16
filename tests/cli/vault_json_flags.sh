#!/usr/bin/env bash
# k2 vault --json: every vault verb whose code prints the daemon's answer
# takes --json (K2, 2026-11-16), and `k2 --schema` lists it. This runs a
# sample across the different code paths and parses each output as JSON:
#   status, set, hosts, grant   cmd_vault (migrate.sh): GET, POST with a value
#                               on stdin, POST policy, the grant path
#   sign --scope                the policy body names the scope
#   file, sync                  vault.sh: the wait call, the sync call
#   placement add               job.sh (_vault_placement)
#   incoming                    the handoff verbs (vault.sh)
#   systemd-cred --scope --dry-run   job.sh, over vault.sock (Linux only: a
#                               uname shim says Linux; nothing is placed)
# Then `k2 --schema` lists --json on each vault verb that has it, and the
# once-hidden flags made public with a test here or in
# tests/cli/vault_handoff_flags.sh.
#
# A tiny stub daemon (python3 http.server) on loopback and a stub vault.sock
# record every request and answer it. No real daemon, no ~/.k2, no keychain:
# HOME is a throwaway dir.

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

# Short path: the stub vault.sock must fit a Unix socket path.
SANDBOX="$(mktemp -d /tmp/k2vj-XXXXXX)"
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
import json, os, socketserver, sys, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG, PORTFILE, SOCK = sys.argv[1], sys.argv[2], sys.argv[3]

# Each route's answer carries "stub": <path>, so a test knows the printed
# JSON is the daemon's answer as it came.
ANSWERS = {
    "/cli/vault/status": {"open": True, "keyHome": "file", "counts": {"user": 1}},
    "/cli/vault/set": {"created": True, "ref": "k2v://server/K", "secret": {"name": "K", "latestVersion": 1}},
    "/cli/vault/policy": {"secret": {"name": "K", "hosts": ["api.example.com"]}},
    "/cli/vault/grant": {"grant": {"id": "gr_1", "consumerLabel": "ws:acme"}},
    "/cli/vault/file": {"path": "/tmp/k2-use/K", "ttlSecs": 60},
    "/cli/vault/sync": {"dir": "/tmp/k2-secrets", "delivered": [], "present": ["K"], "hint": "ok"},
    "/cli/vault/placement/add": {"placement": {"id": "pl_1", "name": "K", "where": "box:/etc/k", "version": 1}},
    "/cli/vault/incoming": {"items": []},
    "/cli/vault/systemd-cred": {"ok": True, "confirmText": "place K v1 on k2-watch.service", "version": 1, "plan": {"unit": "k2-watch.service"}},
}

class H(BaseHTTPRequestHandler):
    channel = "tcp"
    def log_message(self, *a):
        pass
    def answer(self, body):
        p = urlparse(self.path).path
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        q.pop("token", None)
        with open(LOG, "a") as f:
            f.write(json.dumps({"via": self.channel, "method": self.command, "path": p, "query": q, "body": body}, sort_keys=True) + "\n")
        if p in ANSWERS:
            code, obj = 200, dict(ANSWERS[p], stub=p)
        else:
            code, obj = 404, {"error": {"code": "not_found", "hint": p}}
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        self.answer(None)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.answer(json.loads(self.rfile.read(n) or b"{}"))

class UH(H):
    channel = "unix"
    def address_string(self):
        return "unix"

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

if os.path.exists(SOCK):
    os.unlink(SOCK)
us = U(SOCK, UH)
threading.Thread(target=us.serve_forever, daemon=True).start()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(PORTFILE, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
export HOME="$SANDBOX/h"
mkdir -p "$HOME/.k2"
SOCK="$HOME/.k2/vault.sock"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" "$SOCK" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && [ -S "$SOCK" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
[ -S "$SOCK" ] || { echo "FAIL: the stub vault.sock did not start" >&2; exit 1; }
printf '%s' "$(cat "$SANDBOX/port")" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

# A uname that says Linux, for systemd-cred only (it is Linux only).
mkdir -p "$SANDBOX/linux-bin"
cat > "$SANDBOX/linux-bin/uname" <<'SH'
#!/bin/sh
if [ "${1:-}" = "-s" ]; then echo Linux; else exec /usr/bin/uname "$@"; fi
SH
chmod +x "$SANDBOX/linux-bin/uname"

# json_of <path> <args…>: run k2 (stdin from $SANDBOX/in), want exit 0, a
# request to <path>, and stdout that parses as JSON with "stub": <path>.
# Sets R to the request, as sorted JSON (no subshell: the counts stay).
R=""
json_of() {
    local path="$1"; shift
    local before rc got
    before="$(wc -l < "$LOG")"
    set +e
    "$@" <"$SANDBOX/in" >"$SANDBOX/out" 2>"$SANDBOX/err"
    rc=$?
    set -e
    if [ "$rc" != 0 ]; then
        bad "${*:2} exit $rc ($(head -c 300 "$SANDBOX/err"))"
        R="{}"
        return 0
    fi
    if got="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["stub"])' "$SANDBOX/out" 2>&1)" && [ "$got" = "$path" ]; then
        ok "${*:2}: JSON, the daemon's answer"
    else
        bad "${*:2}: stdout is not the daemon's answer as JSON: $(head -c 300 "$SANDBOX/out") ($got)"
    fi
    if [ "$(tail -n +"$((before + 1))" "$LOG" | grep -c "\"path\": \"$path\"")" -lt 1 ]; then
        bad "${*:2}: no request to $path"
    fi
    R="$(grep "\"path\": \"$path\"" "$LOG" | tail -1)"
}
field() { python3 -c 'import json,sys; v=json.loads(sys.argv[1]); print(json.dumps(eval(sys.argv[2]), sort_keys=True))' "$1" "$2"; }

: > "$SANDBOX/in"
echo "== a sample of vault verbs with --json =="
json_of /cli/vault/status "$K2" vault status --json
printf 'v4lue' > "$SANDBOX/in"
json_of /cli/vault/set "$K2" vault set K --for api.example.com --json
r="$R"
expect_eq "set --json still sends the value and the sites" "$(field "$r" '[v["body"]["name"], v["body"]["valueB64"], v["body"]["hosts"]]')" '["K", "djRsdWU=", ["api.example.com"]]'
: > "$SANDBOX/in"
json_of /cli/vault/policy "$K2" vault hosts K --add api.example.com --json
r="$R"
expect_eq "hosts --json: the policy body" "$(field "$r" '[v["body"]["name"], v["body"]["addHosts"]]')" '["K", ["api.example.com"]]'
json_of /cli/vault/grant "$K2" vault grant K --to ws:acme --as fetch --json
r="$R"
expect_eq "grant --json: the grant body" "$(field "$r" '[v["body"]["to"], v["body"]["as"]]')" '["ws:acme", "fetch"]'
json_of /cli/vault/file "$K2" vault file K --json
r="$R"
expect_eq "file --json: the file body" "$(field "$r" 'v["body"]["secret"]')" '"K"'
json_of /cli/vault/sync "$K2" vault sync --json
json_of /cli/vault/placement/add "$K2" vault placement add K --where box:/etc/k --json
r="$R"
expect_eq "placement add --json: the placement body" "$(field "$r" '[v["body"]["name"], v["body"]["where"]]')" '["K", "box:/etc/k"]'
json_of /cli/vault/incoming "$K2" vault incoming --json
r="$R"
expect_eq "incoming --json: over vault.sock" "$(field "$r" 'v["via"]')" '"unix"'

echo "== sign --scope names the secret's scope =="
json_of /cli/vault/policy "$K2" vault sign K hmac-sha256 --sign-header X-Sig --scope ws:acme --json
r="$R"
expect_eq "sign --scope ws:acme" "$(field "$r" '[v["body"]["name"], v["body"]["scope"], v["body"]["workspace"], v["body"]["sign"]["scheme"]]')" '["K", "workspace", "acme", "hmac-sha256"]'
json_of /cli/vault/policy "$K2" vault sign K none --scope server --json
r="$R"
expect_eq "sign none --scope server" "$(field "$r" '[v["body"]["scope"], v["body"]["sign"]]')" '["server", "none"]'

echo "== systemd-cred --scope --dry-run --json (Linux; nothing placed) =="
before="$(wc -l < "$LOG")"
json_of /cli/vault/systemd-cred env PATH="$SANDBOX/linux-bin:$PATH" "$K2" vault systemd-cred K --unit k2-watch.service --scope server --dry-run --json
r="$R"
expect_eq "systemd-cred --scope server: the dry run over vault.sock" "$(field "$r" '[v["via"], v["body"]["name"], v["body"]["scope"], v["body"]["unit"], v["body"]["dryRun"]]')" '["unix", "K", "server", "k2-watch.service", true]'
expect_eq "systemd-cred --dry-run: one request, no confirm" "$(( $(wc -l < "$LOG") - before ))" "1"
json_of /cli/vault/systemd-cred env PATH="$SANDBOX/linux-bin:$PATH" "$K2" vault systemd-cred K --unit k2-watch.service --scope ws:acme --dry-run --json
r="$R"
expect_eq "systemd-cred --scope ws:acme" "$(field "$r" '[v["body"]["scope"], v["body"]["workspace"]]')" '["workspace", "acme"]'

echo "== k2 --schema lists them =="
"$K2" --schema > "$SANDBOX/schema.json"
check_flags() {
    local cmd="$1"; shift
    local missing
    missing="$(python3 -c '
import json, sys
c = {x["name"]: x for x in json.load(open(sys.argv[1]))["commands"]}
have = {f["name"] for f in c[sys.argv[2]]["flags"]}
print(" ".join(f for f in sys.argv[3:] if f not in have))' "$SANDBOX/schema.json" "$cmd" "$@")"
    if [ -z "$missing" ]; then ok "schema: vault ${cmd#vault } has $*"; else bad "schema: $cmd lacks $missing"; fi
}
for cmd in status open list show set rotate hosts delivery sign public-key agent-limit grant revoke disable enable \
    destroy get audit requests presets env-file file proxy-url sync import service doctor send incoming \
    "incoming accept" "incoming decline" sent accept-from "accept-from add" "accept-from remove" send-allow \
    "send-allow add" "send-allow remove" ask asks unsend "placement add" "placement done" "placement rm" \
    placements systemd-cred recipient "carry import"; do
    check_flags "vault $cmd" --json
done
check_flags "vault sign" --scope
check_flags "vault systemd-cred" --scope
check_flags "vault send" --scope --workspace
check_flags "vault ask" --workspace
check_flags "vault asks" --workspace
hidden="$(python3 -c '
import json, sys
c = {x["name"]: x for x in json.load(open(sys.argv[1]))["commands"]}
out = []
for cmd, flags in (("vault send", ["--via"]), ("vault fetch", ["--dst", "--fmt", "--method", "--header", "--data", "--output"]), ("vault proxy-url", ["--secret"])):
    have = {f["name"] for f in c[cmd]["flags"]}
    out += ["%s %s" % (cmd, f) for f in flags if f in have]
print(", ".join(out))' "$SANDBOX/schema.json")"
expect_eq "schema: internal and alias flags stay hidden" "$hidden" ""

echo
echo "vault_json_flags: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
