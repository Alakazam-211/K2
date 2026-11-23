#!/usr/bin/env bash
# A confirm answer with no token ends the command (exit 3) with one line,
# and the step it was for is never sent. Before, a null token was printed
# as the word "None" and went out as the token, and an empty one went out
# as null.
#
# What the REAL CLI sends, against a stub that answers vault.sock and the
# daemon's TCP port and records each request:
#   1. k2 vault rekey (_vault_confirm_token): a token, then a missing, null,
#      empty, non-text and non-JSON token.
#   2. The phone fallback whose card answer can't be read (a 200 that
#      isn't JSON): before, the command went on with no token.
#   3. k2 vault rollback of a value root units load
#      (_vault_confirm_token_plan): a token, then a null one.
#   4. k2 api-key create --hire (the hire key's confirm): a token, then a
#      null one.
#
# No daemon build; a throwaway HOME; never touches ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
# A person's own terminal: no K2 terminal passport (the vault refuses one).
unset K2_HOOK_TOKEN K5_HOOK_TOKEN K2SO_HOOK_TOKEN SSH_CONNECTION SSH_TTY K2_VAULT_CONFIRM_METHOD K5_VAULT_CONFIRM_METHOD
mkdir -p "$HOME/.k2"
printf 'test-owner-token' > "$HOME/.k2/heartbeat.token"
SOCK="$HOME/.k2/vault.sock"

WORK="$(mktemp -d -t k2-confirm-token-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

# The stub. $WORK/finish holds the raw body confirm/finish answers with;
# $WORK/mode is "unanswered" for the phone fallback.
python3 - "$WORK" "$SOCK" <<'PYEOF' &
import json, os, socketserver, sys, threading
from http.server import BaseHTTPRequestHandler

work, sock = sys.argv[1], sys.argv[2]
n = {"i": 0}
lock = threading.Lock()

def read(name, default):
    try:
        return open(os.path.join(work, name)).read()
    except OSError:
        return default

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def address_string(self):
        return "stub"

    def raw(self, data, code=200):
        data = data.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def reply(self, obj, code=200):
        self.raw(json.dumps(obj), code)

    def record(self, body):
        with lock:
            n["i"] += 1
            with open(os.path.join(work, "req-%03d.json" % n["i"]), "w") as f:
                json.dump({"path": self.path.split("?")[0], "body": body}, f)

    def do_GET(self):
        path = self.path.split("?")[0]
        self.record({})
        if path == "/cli/vault/show":
            return self.reply({"secret": {"id": "sec_1", "name": "DB_PASS"},
                               "placements": [{"target": "systemd_cred", "version": 2, "unit": "acme.service"}]})
        return self.reply({"ok": False, "error": {"code": "not_found", "hint": path}}, 404)

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}"
        body = json.loads(raw)
        path = self.path.split("?")[0]
        self.record(body)
        mode = read("mode", "").strip()
        if path == "/cli/vault/confirm/start":
            if body.get("phone"):
                # A 200 that can't be read: no card was raised.
                return self.raw("not json")
            if "rollbackPlan" in body:
                return self.reply({"methods": ["vault-password"], "confirmText": "Roll DB_PASS back to v1"})
            return self.reply({"methods": ["local-auth"], "phoneOffered": mode == "unanswered"})
        if path == "/cli/vault/confirm/finish":
            if mode == "unanswered" and body.get("method") != "phone":
                return self.reply({"ok": False, "error": {"code": "confirm_unanswered", "hint": "no one answered"}}, 409)
            return self.raw(read("finish", '{"token": "tok-ok", "expiresIn": 60}'))
        if path in ("/cli/vault/rekey", "/cli/vault/rollback"):
            return self.reply({"ok": True})
        if path == "/cli/api-keys/create":
            return self.reply({"id": "ak_1", "key": "k2ak_test", "capabilities": {"hire": True}})
        return self.reply({"ok": False, "error": {"code": "not_found", "hint": path}}, 404)

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

class T(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    allow_reuse_address = True

if os.path.exists(sock):
    os.unlink(sock)
t = T(("127.0.0.1", 0), H)
threading.Thread(target=t.serve_forever, daemon=True).start()
u = U(sock, H)
with open(os.path.join(work, "port.tmp"), "w") as f:
    f.write(str(t.server_address[1]))
os.rename(os.path.join(work, "port.tmp"), os.path.join(work, "port"))
u.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do [ -S "$SOCK" ] && [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -S "$SOCK" ] && [ -s "$WORK/port" ] || { echo "FAIL: the stub never came up" >&2; exit 1; }
PORT="$(cat "$WORK/port")"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
export K2_PORT="$PORT"

PASS=0; FAIL=0
ok() { PASS=$((PASS + 1)); echo "  ok: $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL: $1" >&2; }

NO_TOKEN="no_confirm_token: the confirm answer had no token, so nothing was sent"

# The requests of one run, one line each: the route after /cli/ and the
# token the body carried ("-" for none).
seq_of() {
    python3 - "$WORK" <<'PYEOF'
import glob, json, os, sys
for f in sorted(glob.glob(os.path.join(sys.argv[1], "req-*.json"))):
    r = json.load(open(f))
    tok = r["body"].get("confirmToken", "-")
    print("%s %s" % (r["path"].split("/cli/", 1)[-1], "null" if tok is None else tok))
PYEOF
}

# run <finish body> <mode> <args...>
run() {
    local fin="$1" m="$2"; shift 2
    rm -f "$WORK"/req-*.json
    printf '%s' "$fin" > "$WORK/finish"
    printf '%s' "$m" > "$WORK/mode"
    set +e
    "$K2_CLI" "$@" < /dev/null > "$WORK/out" 2> "$WORK/err"
    RC=$?
    set -e
    SEQ="$(seq_of | tr '\n' ';')"
}

# refused <what> <the sequence up to the confirm> [traceback-ok]: exit 3,
# the line, and nothing after the confirm.
refused() {
    [ "$RC" = 3 ] && ok "$1: exit 3" || bad "$1: rc=$RC, want 3: $(cat "$WORK/err")"
    [ "$(tail -n 1 "$WORK/err")" = "$NO_TOKEN" ] && ok "$1: says there was no token" || bad "$1: err: $(cat "$WORK/err")"
    [ "$SEQ" = "$2" ] && ok "$1: nothing sent after the confirm" || bad "$1: sequence: $SEQ"
    if [ -n "${3:-}" ]; then return 0; fi
    if grep -q "Traceback" "$WORK/err"; then bad "$1: a traceback: $(cat "$WORK/err")"; else ok "$1: no traceback"; fi
}

echo "1. k2 vault rekey"
run '{"token": "tok-ok", "expiresIn": 60}' "" vault rekey
[ "$RC" = 0 ] && ok "a token: rekey ran" || bad "a token: rc=$RC: $(cat "$WORK/err")"
[ "$SEQ" = "vault/confirm/start -;vault/confirm/finish -;vault/rekey tok-ok;" ] && ok "a token: it went in the body" || bad "a token: sequence: $SEQ"
for fin in '{"expiresIn": 60}' '{"token": null, "expiresIn": 60}' '{"token": "", "expiresIn": 60}' \
    '{"token": 7, "expiresIn": 60}' 'not json'; do
    run "$fin" "" vault rekey
    refused "finish $fin" "vault/confirm/start -;vault/confirm/finish -;"
done

echo "2. the phone fallback, its card answer can't be read"
run '{"token": "tok-ok"}' unanswered vault rekey
# The card reader's own traceback comes first; the last line is ours.
refused "no card" "vault/confirm/start -;vault/confirm/finish -;vault/confirm/start -;" traceback-ok
grep -q "Nobody answered on this computer." "$WORK/err" && ok "no card: says nobody answered" || bad "no card: err: $(cat "$WORK/err")"

echo "3. k2 vault rollback, root units load the value (the plan confirm)"
run '{"token": "tok-ok", "expiresIn": 60}' "" vault rollback DB_PASS --to 1
[ "$RC" = 0 ] && ok "a token: rollback ran" || bad "a token: rc=$RC: $(cat "$WORK/err")"
[ "$SEQ" = "vault/show -;vault/confirm/start -;vault/confirm/finish -;vault/rollback tok-ok;" ] && ok "a token: it went in the body" || bad "a token: sequence: $SEQ"
run '{"token": null, "expiresIn": 60}' "" vault rollback DB_PASS --to 1
refused "plan, null token" "vault/show -;vault/confirm/start -;vault/confirm/finish -;"

echo "4. k2 api-key create --hire"
run '{"token": "tok-ok", "expiresIn": 60}' "" api-key create --label acme-hire --hire
[ "$RC" = 0 ] && ok "a token: the key was made" || bad "a token: rc=$RC: $(cat "$WORK/err")"
[ "$SEQ" = "vault/confirm/start -;vault/confirm/finish -;api-keys/create tok-ok;" ] && ok "a token: it went in the body" || bad "a token: sequence: $SEQ"
run '{"token": null, "expiresIn": 60}' "" api-key create --label acme-hire --hire
refused "hire, null token" "vault/confirm/start -;vault/confirm/finish -;"

echo "confirm_token_missing: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
