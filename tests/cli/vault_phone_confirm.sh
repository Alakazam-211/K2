#!/usr/bin/env bash
# When the vault confirm asks the owner's phone (0.46.17): what the REAL
# cli/k2 sends over vault.sock, against a stub vault socket that records each
# request. `k2 vault rekey` is the step (any owner confirm takes the same
# path).
#   1. At the computer (no SSH): this computer's prompt alone; no phone card.
#   2. Nobody answers it (409 confirm_unanswered): the phone card, once, and
#      the terminal says which Thread it went to and why that one.
#   3. Over SSH: the phone card goes up beside the prompt (first answer wins).
#   4. Over SSH with K2_VAULT_CONFIRM_METHOD=local: never the phone.
#   5. No check here at all (phoneOnly): the phone alone.
#   6. K2_VAULT_CONFIRM_METHOD=phone at the computer: the phone alone.
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
printf '1' > "$HOME/.k2/heartbeat.port"
printf 'test-owner-token' > "$HOME/.k2/heartbeat.token"
SOCK="$HOME/.k2/vault.sock"

WORK="$(mktemp -d -t k2-vault-phone-XXXXXX)"
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

python3 - "$WORK" "$SOCK" <<'PYEOF' &
import json, os, socketserver, sys
from http.server import BaseHTTPRequestHandler

work, sock = sys.argv[1], sys.argv[2]
n = {"i": 0}

def mode():
    try:
        return open(os.path.join(work, "mode")).read().strip()
    except OSError:
        return "answered"

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def address_string(self):
        return "stub"

    def reply(self, obj, code=200):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        path = self.path.split("?")[0]
        n["i"] += 1
        with open(os.path.join(work, "req-%03d.json" % n["i"]), "w") as f:
            json.dump({"path": path, "body": body}, f)
        m = mode()
        if path == "/cli/vault/confirm/start":
            if body.get("phone"):
                return self.reply({"phone": {"request": "pr_abc123", "card": "c1", "addr": "ws1", "thread": "the workspace you ran it in", "expiresIn": 300, "text": "Rekey the vault"}})
            if m == "phoneonly":
                return self.reply({"methods": [], "phoneOffered": True, "phoneOnly": True})
            return self.reply({"methods": ["local-auth"], "phoneOffered": True})
        if path == "/cli/vault/confirm/finish":
            if body.get("method") == "phone":
                return self.reply({"token": "tok-phone", "expiresIn": 60, "confirmedOn": "phone"})
            if m == "unanswered" and not body.get("phoneRequest"):
                return self.reply({"ok": False, "error": {"code": "confirm_unanswered", "hint": "no one answered this Mac's prompt within two minutes"}}, 409)
            return self.reply({"token": "tok-local", "expiresIn": 60})
        if path == "/cli/vault/rekey":
            return self.reply({"ok": True, "confirmToken": body.get("confirmToken")})
        return self.reply({"ok": False, "error": {"code": "not_found", "hint": path}}, 404)

class S(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

if os.path.exists(sock):
    os.unlink(sock)
S(sock, H).serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do [ -S "$SOCK" ] && break; sleep 0.1; done
[ -S "$SOCK" ] || { echo "FAIL: stub vault socket never came up" >&2; exit 1; }

PASS=0; FAIL=0
ok() { PASS=$((PASS + 1)); echo "  ok: $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL: $1" >&2; }

# The requests of one run, one line each: path, method, phone, phoneRequest.
seq_of() {
    python3 - "$WORK" <<'PYEOF'
import glob, json, os, sys
for f in sorted(glob.glob(os.path.join(sys.argv[1], "req-*.json"))):
    r = json.load(open(f))
    b = r["body"]
    print("%s %s%s%s" % (r["path"].rsplit("/cli/vault/", 1)[-1], b.get("method") or "-",
          " phone" if b.get("phone") else "", (" req=" + b["phoneRequest"]) if b.get("phoneRequest") else ""))
PYEOF
}

run() {
    # $1 mode, then env assignments for this run.
    local m="$1"; shift
    rm -f "$WORK"/req-*.json
    printf '%s' "$m" > "$WORK/mode"
    set +e
    env "$@" "$K2_CLI" vault rekey < /dev/null > "$WORK/out" 2> "$WORK/err"
    RC=$?
    set -e
    SEQ="$(seq_of | tr '\n' ';')"
}

echo "1. at the computer: its own prompt alone"
run answered
[ "$RC" = 0 ] && ok "rekey ran" || { bad "rc=$RC: $(cat "$WORK/err")"; }
[ "$SEQ" = "confirm/start -;confirm/finish local-auth;rekey -;" ] && ok "no phone card" || bad "sequence: $SEQ"

echo "2. nobody answers the computer: the phone card, once"
run unanswered
[ "$RC" = 0 ] && ok "rekey ran" || bad "rc=$RC: $(cat "$WORK/err")"
[ "$SEQ" = "confirm/start -;confirm/finish local-auth;confirm/start - phone;confirm/finish phone req=pr_abc123;rekey -;" ] && ok "fell back to the phone" || bad "sequence: $SEQ"
grep -q "Nobody answered on this computer." "$WORK/err" && ok "says nobody answered" || bad "err: $(cat "$WORK/err")"
grep -q "card pr_abc123 on ws1's Thread — the workspace you ran it in" "$WORK/err" && ok "names the Thread and why" || bad "err: $(cat "$WORK/err")"
grep -q "Confirmed on your phone." "$WORK/err" && ok "says the phone confirmed it" || bad "err: $(cat "$WORK/err")"

echo "3. over SSH: the phone beside the prompt"
run answered SSH_CONNECTION="192.0.2.7 50000 192.0.2.8 22"
[ "$RC" = 0 ] && ok "rekey ran" || bad "rc=$RC: $(cat "$WORK/err")"
[ "$SEQ" = "confirm/start -;confirm/start - phone;confirm/finish local-auth req=pr_abc123;rekey -;" ] && ok "raced" || bad "sequence: $SEQ"
grep -q "or on your phone: card pr_abc123" "$WORK/err" && ok "says both" || bad "err: $(cat "$WORK/err")"

echo "4. over SSH, K2_VAULT_CONFIRM_METHOD=local: never the phone"
run unanswered SSH_TTY=/dev/ttys009 K2_VAULT_CONFIRM_METHOD=local
[ "$RC" != 0 ] && ok "fails loudly" || bad "rc=0"
[ "$SEQ" = "confirm/start -;confirm/finish local-auth;" ] && ok "no phone card" || bad "sequence: $SEQ"
grep -q "confirm_unanswered" "$WORK/err" && ok "says why" || bad "err: $(cat "$WORK/err")"

echo "5. no check here at all: the phone alone"
run phoneonly
[ "$RC" = 0 ] && ok "rekey ran" || bad "rc=$RC: $(cat "$WORK/err")"
[ "$SEQ" = "confirm/start -;confirm/start - phone;confirm/finish phone req=pr_abc123;rekey -;" ] && ok "phone only" || bad "sequence: $SEQ"

echo "6. K2_VAULT_CONFIRM_METHOD=phone at the computer"
run answered K2_VAULT_CONFIRM_METHOD=phone
[ "$RC" = 0 ] && ok "rekey ran" || bad "rc=$RC: $(cat "$WORK/err")"
[ "$SEQ" = "confirm/start -;confirm/start - phone;confirm/finish phone req=pr_abc123;rekey -;" ] && ok "phone only" || bad "sequence: $SEQ"

echo "vault_phone_confirm: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
