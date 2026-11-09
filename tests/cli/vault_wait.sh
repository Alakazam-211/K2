#!/usr/bin/env bash
# `k2 vault file --wait` / `env-file --wait` and `k2 thread secret --once`
# (prd-vault-for-agents-v1 §3.4, §3.5), against a stub daemon:
#   * the path prints on stdout only, after the daemon's "waiting" answers,
#     and the CLI follows the card it was told about;
#   * past its own deadline the CLI exits 3 and names the open card;
#   * no card (404) is exit 4, a dismissed card is exit 3;
#   * --wait is an env-file flag too, and doesn't go with --done;
#   * --once goes on the card request.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d -t k2-vault-wait-XXXXXX)"
STUB_PID=""
trap '[ -n "$STUB_PID" ] && { kill "$STUB_PID" 2>/dev/null; wait "$STUB_PID" 2>/dev/null || true; }; rm -rf "$WORK"' EXIT

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
cat >"$WORK/stub.py" <<'PY'
import http.server, json, sys
LOG = sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def reply(self, status, out):
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        self.reply(200, {})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode()
        with open(LOG, "a") as f:
            f.write(self.path.split("?")[0] + " " + raw + "\n")
        b = json.loads(raw or "{}")
        if self.path.startswith("/cli/thread/secret"):
            return self.reply(200, {"ok": True, "id": "c9", "status": "pending", "name": b.get("name"), "sends": "x"})
        name = b.get("secret") or (b.get("secrets") or [""])[0]
        if name == "NOCARD":
            return self.reply(404, {"error": {"code": "no_card", "hint": "no card asks for NOCARD; ask a person with k2 thread secret <addr> --name NOCARD"}})
        if name == "DECLINED":
            return self.reply(403, {"error": {"code": "card_declined", "hint": "a person declined; nothing was saved"}})
        if name == "NEVER":
            return self.reply(200, {"ok": True, "waiting": True, "card": "c1", "waitSecs": 1})
        if b.get("waitCard") == "c1":
            return self.reply(200, {"ok": True, "path": "/run/k2-secrets/use-files/abc-" + name, "name": name, "ttlSecs": 60})
        return self.reply(200, {"ok": True, "waiting": True, "card": "c1", "waitSecs": 600})
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
python3 "$WORK/stub.py" "$PORT" "$WORK/posts.log" &
STUB_PID=$!
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break; sleep 0.05; done
k2s() { env HOME="$WORK" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN=stub-token K2_PROJECT_PATH="$WORK" "$K2_CLI" "$@"; }

: >"$WORK/posts.log"
out="$(k2s vault file SAVED --wait 10m 2>"$WORK/err")" || fail "file --wait: $(cat "$WORK/err")"
[ "$out" = "/run/k2-secrets/use-files/abc-SAVED" ] || fail "stdout is only the path: $out"
grep -Fq "waiting for a person to save the card (c1)" "$WORK/err" || fail "no waiting line on stderr: $(cat "$WORK/err")"
first="$(head -1 "$WORK/posts.log")"
printf '%s' "$first" | grep -Fq '"wait": "10m"' || fail "the first call sends --wait as typed: $first"
second="$(sed -n 2p "$WORK/posts.log")"
printf '%s' "$second" | grep -Fq '"waitCard": "c1"' || fail "the second call follows the card: $second"
ok "file --wait prints only the path and follows the card it was told about"

: >"$WORK/posts.log"
out="$(k2s vault env-file SAVED --wait 10m 2>"$WORK/err")" || fail "env-file --wait: $(cat "$WORK/err")"
printf '%s' "$out" | grep -Fq "abc-SAVED" || fail "env-file --wait: $out"
printf '%s' "$(head -1 "$WORK/posts.log")" | grep -Fq '"wait": "10m"' || fail "env-file sends --wait: $(cat "$WORK/posts.log")"
ok "env-file --wait works the same way"

set +e
err="$(k2s vault file NEVER --wait 1s 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" = 3 ] || fail "past the deadline: exit $rc ($err)"
printf '%s' "$err" | grep -Fq "no answer yet; the card is still open (card c1)" || fail "past the deadline: $err"
ok "past its deadline the CLI exits 3 and names the open card"

set +e
err="$(k2s vault file NOCARD --wait 10m 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" = 4 ] || fail "no card: exit $rc ($err)"
printf '%s' "$err" | grep -Fq "no card asks for NOCARD" || fail "no card: $err"
set +e
err="$(k2s vault file DECLINED --wait 10m 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" = 3 ] || fail "dismissed: exit $rc ($err)"
printf '%s' "$err" | grep -Fq "card_declined" || fail "dismissed: $err"
ok "no card is exit 4; a dismissed card is exit 3"

set +e
err="$(k2s vault file --done /tmp/x --wait 1m 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] || fail "--done --wait: exit $rc ($err)"
ok "--wait doesn't go with --done"

: >"$WORK/posts.log"
k2s thread secret main --name CODE --scope session --delivery file --once >/dev/null 2>"$WORK/err" || fail "thread secret --once: $(cat "$WORK/err")"
grep -Fq '"once": true' "$WORK/posts.log" || fail "--once isn't on the request: $(cat "$WORK/posts.log")"
ok "thread secret --once goes on the card request"

help="$(k2s vault file --help)"
printf '%s' "$help" | grep -Fq -- "--wait 10m" || fail "file --help doesn't say --wait: $help"
help="$(k2s thread secret --help)"
printf '%s' "$help" | grep -Fq -- "--once" || fail "thread secret --help doesn't say --once"
ok "help names --wait and --once"

echo "PASS: vault wait ($pass checks)"
