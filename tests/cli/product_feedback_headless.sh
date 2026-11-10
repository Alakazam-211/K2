#!/usr/bin/env bash
# Send feedback to K2 (prd-k2-product-feedback-v1, 0.46.11) end to end against
# a foreground sandbox daemon built from THIS worktree (temp HOME, no client),
# a loopback FAKE K2 Connect relay (`POST /api/identity/assert`) and a
# loopback FAKE intake (`POST /v1/submissions`). Nothing reaches
# connect.k2.dev or feedback.akzm.k2.dev.
#
#   1. the relay has no assert route yet (404): `k2 send-feedback` saves the
#      feedback, exits 0 and says the service isn't reachable yet; the row
#      stays queued with relay_not_ready;
#   2. the relay answers: the drain sends it on its own (box token, clientId
#      as the idempotency key); `list` shows it sent;
#   3. GET on a POST-only route is 405;
#   4. --diagnostics-preview prints the scrubbed details and sends nothing;
#      --diagnostics --yes sends exactly the previewed sections;
#   5. `settings --enabled off` refuses the next send (exit 3, disabled).
#
#   cargo build -p k2-daemon && bash tests/cli/product_feedback_headless.sh
#
# Every check fails loudly with the output.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }

# Never inherit a session's identity from the caller.
for v in $(env | grep -E '^(K2|K2SO)_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d /tmp/k2pf.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
mkdir -p "$SB/.k2" "$SB/tmp" "$SB/run"
DAEMON_PID=""
FAKE_PID=""
DLOG="$SB/daemon.log"
cleanup() {
    for p in "$DAEMON_PID" "$FAKE_PID"; do
        [ -n "$p" ] || continue
        kill "$p" 2>/dev/null || true
        for _ in $(seq 1 100); do kill -0 "$p" 2>/dev/null || break; sleep 0.1; done
        kill -9 "$p" 2>/dev/null || true
        wait "$p" 2>/dev/null || true
    done
    rm -rf "$WORK"
}
trap cleanup EXIT

pass=0
fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log tail ---" >&2
    tail -40 "$DLOG" >&2 2>/dev/null || true
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
jcheck() {
    python3 - "$1" "$2" <<'PY' || return 1
import json, sys
j = json.loads(sys.argv[1])
if not eval(sys.argv[2], {"j": j}):
    sys.exit(1)
PY
}
jok() { jcheck "$1" "$2" || fail "$3: $2 is false for: $1"; ok "$3"; }
has() { [[ "$1" == *"$2"* ]] || fail "$3: missing $(printf %q "$2") in:
$1"; }

echo "== fakes =="
# One python process, two ports. The relay answers 404 until $WORK/relay.ok
# exists; the intake answers 201 {id}. Every request is appended as JSON.
cat >"$WORK/fakes.py" <<'PY'
import json, os, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
work = sys.argv[1]
JWT = "eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJyb3Nzb24iLCJleHAiOjQxMDI0NDQ4MDB9.c2ln"
def handler(kind):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass
        def do_GET(self):
            self.answer(404, {"code": "not_found"})
        def do_POST(self):
            n = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(n).decode()
            rec = {"path": self.path, "headers": dict(self.headers), "body": body}
            with open(os.path.join(work, kind + ".jsonl"), "a") as f:
                f.write(json.dumps(rec) + "\n")
            if kind == "relay":
                if self.path != "/api/identity/assert" or not os.path.exists(os.path.join(work, "relay.ok")):
                    return self.answer(404, {"code": "not_found"})
                return self.answer(200, {"token": JWT, "expiresAt": 4102444800})
            if self.path != "/v1/submissions":
                return self.answer(404, {"code": "not_found"})
            key = self.headers.get("Idempotency-Key", "")
            return self.answer(201, {"id": key.replace("pf_", "remote-")})
        def answer(self, status, obj):
            b = json.dumps(obj).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)
    return H
servers = []
for kind in ("relay", "intake"):
    s = ThreadingHTTPServer(("127.0.0.1", 0), handler(kind))
    servers.append(s)
    with open(os.path.join(work, kind + ".port"), "w") as f:
        f.write(str(s.server_address[1]))
    threading.Thread(target=s.serve_forever, daemon=True).start()
threading.Event().wait()
PY
python3 "$WORK/fakes.py" "$WORK" &
FAKE_PID=$!
for _ in $(seq 1 100); do [ -s "$WORK/intake.port" ] && [ -s "$WORK/relay.port" ] && break; sleep 0.1; done
RELAY="http://127.0.0.1:$(cat "$WORK/relay.port")"
INTAKE="http://127.0.0.1:$(cat "$WORK/intake.port")"
ok "fake relay $RELAY, fake intake $INTAKE"

echo "== daemon =="
# Paired with K2 Connect, but never dialing: no auto start, a dead frps.
cat >"$SB/.k2/tunnel.json" <<'JSON'
{"server_addr": "127.0.0.1", "server_port": 1, "token": "tun-tok-headless", "subdomain": "rosson", "auto_start": false}
JSON
DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || fail "k2-daemon not built (cargo build -p k2-daemon)"
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
    HOME="$SB" K2SO_WATCHDOG_DISABLED=1 K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    K2_CONNECT_BASE="$RELAY" K2_PRODUCT_FEEDBACK_URL="$INTAKE" K2_PRODUCT_FEEDBACK_TICK_SECS=1 \
    K2_PUSH_GATEWAY_URL=http://127.0.0.1:1 K2_CERT_BROKER_URL=http://127.0.0.1:1 \
    "$DAEMON_BIN" >"$DLOG" 2>&1 &
DAEMON_PID=$!
phase=""
for _ in $(seq 1 400); do
    if [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ]; then
        DPORT="$(cat "$SB/.k2/daemon.port")"
        phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
        [ "$phase" = "ready" ] && break
    fi
    sleep 0.1
done
[ "$phase" = "ready" ] || fail "daemon never reached ready"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
BASE="http://127.0.0.1:$DPORT"
ok "daemon ready on $DPORT"

cli() {
    set +e
    OUT="$(env HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" "$K2_CLI" "$@" 2>"$WORK/cli.err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/cli.err")"
}
want_rc() { [ "$RC" = "$1" ] || fail "$2: exit $RC (want $1)
stdout: $OUT
stderr: $ERR"; }
show() { curl -s "$BASE/cli/product-feedback/show?id=$1&token=$DTOKEN"; }

echo "== 1. before the service exists =="
S="$(curl -s "$BASE/cli/product-feedback/settings?token=$DTOKEN")"
jok "$S" 'j["canSend"] is True and j["endpoint"] == "'"$INTAKE"'"' "settings: can send, the fake intake"
cli send-feedback "headless: the tab froze" --category bug
want_rc 0 "send while the relay has no route"
has "$OUT" "Saved on this server" "send says it is saved"
has "$OUT" "isn't reachable yet" "send says the service isn't reachable yet"
ID="$(printf '%s' "$OUT" | python3 -c 'import re,sys; print(re.search(r"pf_[0-9A-Za-z]{22}", sys.stdin.read()).group(0))')"
R="$(show "$ID")"
jok "$R" 'j["item"]["state"] == "queued" and j["item"]["error"] == "relay_not_ready"' "queued with relay_not_ready"
[ ! -s "$WORK/intake.jsonl" ] || fail "the intake got a submission without a token"
ok "nothing reached the intake"

echo "== 2. the relay answers: the drain sends it =="
touch "$WORK/relay.ok"
sent=""
for _ in $(seq 1 90); do
    R="$(show "$ID")"
    if jcheck "$R" 'j["item"]["state"] == "sent"'; then sent=1; break; fi
    sleep 1
done
[ -n "$sent" ] || fail "never sent after the relay came up: $R"
ok "sent by the drain on its own"
python3 - "$WORK/intake.jsonl" "$ID" <<'PY' || fail "submission headers/body"
import json, sys
rec = [json.loads(l) for l in open(sys.argv[1])][-1]
assert rec["path"] == "/v1/submissions", rec["path"]
h = {k.lower(): v for k, v in rec["headers"].items()}
assert h["idempotency-key"] == sys.argv[2], h
assert h["authorization"].startswith("K2-Box eyJ"), h
assert "tun-tok-headless" not in json.dumps(rec), "the tunnel token never goes to the intake"
b = json.loads(rec["body"])
assert sorted(b) == ["arch", "category", "client", "clientId", "daemonVersion", "installKind", "k2Version", "locale", "os", "osVersion", "senderKind", "text"], sorted(b)
assert (b["category"], b["client"], b["senderKind"], b["text"]) == ("bug", "cli", "person", "headless: the tab froze"), b
PY
ok "the body is exactly the wire contract"
python3 - "$WORK/relay.jsonl" <<'PY' || fail "relay request"
import json, sys
rec = [json.loads(l) for l in open(sys.argv[1]) if json.loads(l)["path"] == "/api/identity/assert"][-1]
h = {k.lower(): v for k, v in rec["headers"].items()}
assert h["authorization"] == "Bearer tun-tok-headless", h
assert h["x-k2-subdomain"] == "rosson", h
PY
ok "the relay saw the tunnel bearer and the label"
cli send-feedback list
want_rc 0 "list"
has "$OUT" "$ID" "list shows the row"
has "$OUT" "sent" "list shows it sent"

echo "== 3. GET on a POST-only route =="
code="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/cli/product-feedback/send?token=$DTOKEN")"
[ "$code" = 405 ] || fail "GET send answered $code"
ok "GET send is 405"

echo "== 4. diagnostics =="
mkdir -p "$SB/work/acme-payroll"
printf '%s [agent-hooks] path=Some("%s/work/acme-payroll/src") failed token=abc123\n' "$(date -u +%Y-%m-%dT%H:%M:%S.000Z)" "$SB" >> "$SB/.k2/daemon.stderr.log"
n_before="$(wc -l < "$WORK/intake.jsonl")"
cli send-feedback --diagnostics-preview
want_rc 0 "preview"
has "$ERR" "Nothing was sent." "preview sends nothing"
has "$OUT" "── Versions (versions) ──" "preview prints the sections"
[[ "$OUT" != *"token=abc123"* ]] || fail "the preview kept a secret: $OUT"
[[ "$OUT" != *"$SB"* ]] || fail "the preview kept the home path: $OUT"
[ "$(wc -l < "$WORK/intake.jsonl")" = "$n_before" ] || fail "the preview sent something"
ok "the preview is scrubbed and sends nothing"
cli send-feedback "headless: with details" --diagnostics --yes
want_rc 0 "send with diagnostics"
D_ID="$(printf '%s' "$OUT" | python3 -c 'import re,sys; print(re.search(r"pf_[0-9A-Za-z]{22}", sys.stdin.read()).group(0))')"
for _ in $(seq 1 30); do jcheck "$(show "$D_ID")" 'j["item"]["state"] == "sent"' && break; sleep 1; done
R="$(show "$D_ID")"
jok "$R" 'j["item"]["state"] == "sent" and [s["id"] for s in j["item"]["diagnostics"]][0] == "versions"' "sent with the details"
python3 - "$WORK/intake.jsonl" "$R" <<'PY' || fail "diagnostics sent != shown"
import json, sys
rec = [json.loads(l) for l in open(sys.argv[1])][-1]
b = json.loads(rec["body"])
shown = json.loads(sys.argv[2])["item"]["diagnostics"]
assert b["diagnostics"] == {"sections": shown}, "sent and shown differ"
PY
ok "the intake got exactly the sections the Sent tab shows"

echo "== 5. turned off =="
cli send-feedback settings --enabled off
want_rc 0 "settings off"
cli send-feedback "headless: while off"
want_rc 3 "send while off"
has "$ERR" '"disabled"' "the refusal names disabled"
cli send-feedback settings --enabled on
want_rc 0 "settings on"

echo ""
echo "product_feedback_headless: $pass checks passed"
