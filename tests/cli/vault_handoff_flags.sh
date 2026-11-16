#!/usr/bin/env bash
# k2 vault send, incoming, sent, ask, asks, unsend, accept-from and
# send-allow share one flag parser. Each verb takes only the flags it uses
# (K2, 2026-11-16): a flag another verb uses is refused on stderr, named,
# exit 2, and no request goes out. Never parsed and then dropped.
#
# A tiny stub daemon (python3 http.server) listens on loopback and on a stub
# vault.sock, records every request and answers it. No real daemon, no
# ~/.k2, no keychain: HOME is a throwaway dir.
#
# Covers, per verb: each flag it uses reaches the request (body or query),
# and flags it doesn't use are refused with nothing sent. Also:
#   send --workspace WS   a person's: the secret of that workspace (body
#                         workspace), the same as --scope ws:WS; the two
#                         disagreeing, or --workspace with --scope server, is
#                         refused
#   send --for            only with --stdin (a named send keeps the secret's
#                         own sites); --stdin refuses --scope and --workspace
#   send from a K2 terminal (an agent): --scope and --workspace are refused
#                         (the daemon sends the agent's own workspace's)
#   k2 msg <agent> "text" --secret NAME   still sends via=msg with the text
#                         as the note
#   incoming decline      takes no --as, --replace or --always

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
SANDBOX="$(mktemp -d /tmp/k2vh-XXXXXX)"
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

class H(BaseHTTPRequestHandler):
    channel = "tcp"
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def record(self, body):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        tok = q.pop("token", None)
        with open(LOG, "a") as f:
            f.write(json.dumps({"via": self.channel, "token": tok, "method": self.command, "path": u.path,
                                "query": q, "body": body}, sort_keys=True) + "\n")
    def do_GET(self):
        self.record(None)
        p = urlparse(self.path).path
        if p in ("/cli/vault/incoming", "/cli/vault/sent", "/cli/vault/asks"):
            return self.reply(200, {"items": [], "stub": p})
        if p in ("/cli/vault/accept-from", "/cli/vault/send-allow"):
            return self.reply(200, {"entries": [], "stub": p})
        return self.reply(404, {"error": {"code": "not_found", "hint": p}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        self.record(body)
        p = urlparse(self.path).path
        if p == "/cli/vault/send":
            return self.reply(200, {"status": "landed", "name": body.get("name"), "to": body.get("to"), "deliveryId": "hd_1", "stub": p})
        if p == "/cli/vault/incoming/decide":
            return self.reply(200, {"ok": True, "delivery": {"status": "landed", "name": "K", "peer": "bob", "deliveryId": body.get("id")}, "stub": p})
        if p == "/cli/vault/ask":
            return self.reply(200, {"to": body.get("to"), "name": body.get("name"), "askId": "ak_1", "delivered": True, "expiresInDays": 7, "stub": p})
        if p == "/cli/vault/unsend":
            return self.reply(200, {"ok": True, "deliveryId": body.get("id"), "honest": "stub", "stub": p})
        if p == "/cli/vault/accept-from":
            return self.reply(200, {"entries": [], "stub": p})
        return self.reply(404, {"error": {"code": "not_found", "hint": p}})

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
PORT="$(cat "$SANDBOX/port")"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

# sent <args…>: run k2 (a person in their own terminal: vault.sock is
# there), want exit 0 and exactly one request; print it as sorted JSON.
sent() {
    local before after rc
    before="$(wc -l < "$LOG")"
    set +e
    "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    rc=$?
    set -e
    after="$(wc -l < "$LOG")"
    if [ "$rc" != 0 ] || [ "$((after - before))" != "1" ]; then
        bad "k2 $* exit $rc, $((after - before)) requests, want 0 and 1 ($(head -c 300 "$SANDBOX/err"))"
        echo "{}"
        return 0
    fi
    tail -1 "$LOG"
}
# refused <flag> <args…>: want exit 2, the flag named on stderr, nothing sent.
refused() {
    local flag="$1"; shift
    local before rc
    before="$(wc -l < "$LOG")"
    set +e
    "$K2" "$@" </dev/null >/dev/null 2>"$SANDBOX/err"
    rc=$?
    set -e
    if [ "$rc" = 2 ] && grep -Fq -- "$flag" "$SANDBOX/err" && [ "$(wc -l < "$LOG")" = "$before" ]; then
        ok "k2 $* refused ($flag)"
    else
        bad "k2 $*: exit $rc, $(( $(wc -l < "$LOG") - before )) requests, stderr $(printf %q "$(head -c 300 "$SANDBOX/err")"); want exit 2 naming $flag and nothing sent"
    fi
}
# field <json> <python expression on v>
field() { python3 -c 'import json,sys; v=json.loads(sys.argv[1]); print(json.dumps(eval(sys.argv[2]), sort_keys=True))' "$1" "$2"; }

echo "== send: the flags it uses reach the body =="
r="$(sent vault send K --to bob --as K2 --note "for the deploy" --ttl 2h --reply ak_1 --kind generic --delivery file)"
expect_eq "send: name, to, as, note, ttl, reply, kind, delivery" \
    "$(field "$r" '[v["via"], v["path"], v["body"]]')" \
    "$(field '{}' '["unix", "/cli/vault/send", {"name": "K", "to": "bob", "as": "K2", "note": "for the deploy", "ttlSecs": 7200, "replyTo": "ak_1", "kind": "generic", "delivery": "file"}]')"
r="$(sent vault send K --to bob --scope ws:acme)"
expect_eq "send --scope ws:acme" "$(field "$r" '[v["body"].get("scope"), v["body"].get("workspace")]')" '["workspace", "acme"]'
r="$(sent vault send K --to bob --scope server)"
expect_eq "send --scope server" "$(field "$r" '[v["body"].get("scope"), v["body"].get("workspace")]')" '["server", null]'
r="$(sent vault send K --to bob --workspace acme)"
expect_eq "send --workspace acme: that workspace's secret" "$(field "$r" '[v["body"].get("scope"), v["body"].get("workspace")]')" '[null, "acme"]'
r="$(sent vault send K --to bob --workspace acme --scope ws:acme)"
expect_eq "send --workspace and --scope ws: agreeing" "$(field "$r" '[v["body"].get("scope"), v["body"].get("workspace")]')" '["workspace", "acme"]'
refused "--workspace" vault send K --to bob --workspace acme --scope ws:globex
refused "--workspace" vault send K --to bob --workspace acme --scope server
r="$(sent vault send K --to bob --json)"
expect_eq "send --json prints the daemon's answer" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["stub"])' "$SANDBOX/out")" "/cli/vault/send"
before="$(wc -l < "$LOG")"
set +e
printf 'v4lue' | "$K2" vault send K --to bob::far.example --stdin --kind api_key --for api.example.com,files.example.com >/dev/null 2>"$SANDBOX/err"
rc=$?
set -e
r="$(tail -1 "$LOG")"
expect_eq "send --stdin: exit 0, one request" "$rc $(( $(wc -l < "$LOG") - before ))" "0 1"
expect_eq "send --stdin --kind --for: value, kind, hosts over vault.sock" \
    "$(field "$r" '[v["via"], v["body"]["valueB64"], v["body"]["kind"], v["body"]["hosts"]]')" \
    '["unix", "djRsdWU=", "api_key", ["api.example.com", "files.example.com"]]'
refused "--for" vault send K --to bob --for api.example.com
refused "--scope" vault send K --to bob::far.example --stdin --scope ws:acme
refused "--workspace" vault send K --to bob::far.example --stdin --workspace acme
refused "--replace" vault send K --to bob --replace
refused "--always" vault send K --to bob --always

echo "== send through k2 msg --secret: via msg, the text is the note =="
r="$(sent msg bob "the staging key" --secret K)"
expect_eq "msg --secret" "$(field "$r" '[v["path"], v["body"]]')" \
    "$(field '{}' '["/cli/vault/send", {"name": "K", "to": "bob", "note": "the staging key", "via": "msg"}]')"
r="$(sent vault send K --to bob --via msg)"
expect_eq "send --via msg" "$(field "$r" 'v["body"].get("via")')" '"msg"'

echo "== send from a K2 terminal (an agent) =="
agent() { env K2_HOOK_TOKEN=agent-tok K2_PORT="$PORT" "$@"; }
before="$(wc -l < "$LOG")"
agent "$K2" vault send K --to bob --note hi >/dev/null 2>"$SANDBOX/err"
r="$(tail -1 "$LOG")"
expect_eq "agent send: one request over the passport" "$(( $(wc -l < "$LOG") - before )) $(field "$r" '[v["via"], v["token"], v["body"]]')" \
    "1 $(field '{}' '["tcp", "agent-tok", {"name": "K", "to": "bob", "note": "hi"}]')"
for f in "--scope ws:acme" "--workspace acme"; do
    before="$(wc -l < "$LOG")"
    set +e
    # shellcheck disable=SC2086
    agent "$K2" vault send K --to bob $f >/dev/null 2>"$SANDBOX/err"
    rc=$?
    set -e
    if [ "$rc" = 2 ] && grep -Fq -- "${f%% *}" "$SANDBOX/err" && [ "$(wc -l < "$LOG")" = "$before" ]; then
        ok "agent send $f refused"
    else
        bad "agent send $f: exit $rc, stderr $(printf %q "$(cat "$SANDBOX/err")")"
    fi
done

echo "== incoming, sent, asks: --workspace in the query =="
for verb in incoming sent asks; do
    r="$(sent vault "$verb" --workspace acme)"
    expect_eq "$verb --workspace" "$(field "$r" '[v["method"], v["path"], v["query"]]')" "$(field '{}' "[\"GET\", \"/cli/vault/$verb\", {\"workspace\": \"acme\"}]")"
    r="$(sent vault "$verb" --json)"
    expect_eq "$verb --json" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["stub"])' "$SANDBOX/out")" "/cli/vault/$verb"
    for f in --to --note --as --ttl --reply --scope --kind --for --delivery --via; do
        refused "$f" vault "$verb" "$f" x
    done
    for f in --stdin --replace --always; do
        refused "$f" vault "$verb" "$f"
    done
done

echo "== incoming accept|decline =="
r="$(sent vault incoming accept hd_1 --as K_NEW --replace --always)"
expect_eq "incoming accept --as --replace --always" "$(field "$r" '[v["via"], v["path"], v["body"]]')" \
    "$(field '{}' '["unix", "/cli/vault/incoming/decide", {"id": "hd_1", "action": "accept", "as": "K_NEW", "replace": True, "always": True}]')"
r="$(sent vault incoming decline hd_1)"
expect_eq "incoming decline" "$(field "$r" 'v["body"]')" "$(field '{}' '{"id": "hd_1", "action": "decline"}')"
refused "--workspace" vault incoming accept hd_1 --workspace acme
refused "--note" vault incoming accept hd_1 --note x
refused "--as" vault incoming decline hd_1 --as K_NEW
refused "--replace" vault incoming decline hd_1 --replace
refused "--always" vault incoming decline hd_1 --always

echo "== ask =="
r="$(sent vault ask bob K --note "for the deploy" --workspace acme)"
expect_eq "ask --note --workspace" "$(field "$r" '[v["path"], v["body"]]')" \
    "$(field '{}' '["/cli/vault/ask", {"to": "bob", "name": "K", "note": "for the deploy", "workspace": "acme"}]')"
r="$(sent vault ask bob K --json)"
expect_eq "ask --json" "$(python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print(v["stub"], v["askId"])' "$SANDBOX/out")" "/cli/vault/ask ak_1"
for f in --to --as --ttl --reply --scope --kind --for --delivery --via; do
    refused "$f" vault ask bob K "$f" x
done
for f in --stdin --replace --always; do
    refused "$f" vault ask bob K "$f"
done

echo "== unsend =="
r="$(sent vault unsend hd_1)"
expect_eq "unsend" "$(field "$r" '[v["path"], v["body"]]')" '["/cli/vault/unsend", {"id": "hd_1"}]'
r="$(sent vault unsend hd_1 --json)"
expect_eq "unsend --json" "$(python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print(v["stub"], v["deliveryId"])' "$SANDBOX/out")" "/cli/vault/unsend hd_1"
for f in --workspace --note --to --as --ttl --scope; do
    refused "$f" vault unsend hd_1 "$f" x
done
refused "--replace" vault unsend hd_1 --replace

echo "== accept-from, send-allow =="
r="$(sent vault accept-from list --workspace acme)"
expect_eq "accept-from list --workspace" "$(field "$r" '[v["method"], v["path"], v["query"]]')" '["GET", "/cli/vault/accept-from", {"workspace": "acme"}]'
r="$(sent vault accept-from add ws:globex --workspace acme)"
expect_eq "accept-from add --workspace" "$(field "$r" '[v["via"], v["path"], v["body"]]')" \
    "$(field '{}' '["unix", "/cli/vault/accept-from", {"workspace": "acme", "sender": "ws:globex", "action": "add"}]')"
refused "--to" vault accept-from list --to bob
refused "--note" vault accept-from add ws:globex --workspace acme --note x
r="$(sent vault send-allow list --workspace acme)"
expect_eq "send-allow list --workspace" "$(field "$r" '[v["method"], v["path"], v["query"]]')" '["GET", "/cli/vault/send-allow", {"workspace": "acme"}]'
refused "--to" vault send-allow list --workspace acme --to bob::far.example
refused "--note" vault send-allow add K --to bob::far.example --workspace acme --note x
refused "--stdin" vault send-allow --stdin

echo
echo "vault_handoff_flags: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
