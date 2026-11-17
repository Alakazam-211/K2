#!/usr/bin/env bash
# Idempotent sends (0.46.15), the CLI half: `k2 msg`, `k2 msg --inbox-wake`,
# `k2 thread "text"` and `k2 thread ask` send one key per invocation; a
# send that got no answer is asked again with the SAME key; when it still
# gets none, the command says it may have been delivered and how to ask once
# more (`--idempotency-key <key>`). And the permission footer ("Ask with:" /
# "It can't be requested.") closes only a refusal of the send itself, never
# a successful send, a not-delivered answer or a timeout (0.46.15 field
# reports: a lookup of the sender's own name, refused to a passport over
# TCP, left that line under unrelated outcomes).
#
# A stub daemon keeps the daemon's contract: the first request with a key
# delivers (counted), a repeat answers the first answer with
# `"replayed": true` (a Thread post's `clientMsgId`: `"duplicate": true`),
# and a repeat while the first still runs waits for it. Its first answer
# per key can be slowed past the CLI's request limit (_K2_POST_MAX_TIME).
# The cell socket is served too: the tries stay on it (a POST that may
# have run never goes again over TCP).
# /boot-status lists `send-once` (the daemon's promise that it drops a
# repeated key) unless the FEATURES file says `none`: then the CLI sends
# once, never again over TCP, and says to check before resending.
#
# Hermetic: throwaway HOME, every K2_*/K2SO_* var unset, a python stub on
# loopback and a Unix socket. No real daemon, ~/.k2 or keychain.
#
#   bash tests/cli/msg_send_once.sh      (fails loudly on any miss)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
[ -x "$K2" ] || { echo "FAIL: $K2 not found/executable" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

# Short: a Unix socket path must stay under ~104 bytes.
SANDBOX="$(mktemp -d /tmp/k2so1-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
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
import json, os, socketserver, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit, parse_qs

LOG, PORTF, MODEF, SOCK, FEATURESF = sys.argv[1:6]
lock = threading.Lock()
answers = {}      # (path, key) -> {"done": Event, "answer": (status, body)}
deliveries = {}   # path -> count
log_lock = threading.Lock()

def mode():
    return open(MODEF).read().strip()

def key_of(raw):
    raw = raw.strip()
    if raw.startswith("{"):
        try:
            d = json.loads(raw)
        except Exception:
            return None
        return d.get("idempotency_key") or d.get("clientMsgId")
    q = parse_qs(raw)
    for k in ("idempotency_key", "clientMsgId"):
        if q.get(k):
            return q[k][0]
    return None

def deliver(path):
    """The first try of a key: count it, slow it per the mode, answer."""
    m = mode()
    with lock:
        n = deliveries.get(path, 0) + 1
        deliveries[path] = n
        with open(LOG + ".deliveries", "w") as f:
            json.dump(deliveries, f)
    if m == "slow_first":
        time.sleep(2.5)
    elif m == "hang":
        time.sleep(4)
    if path == "/cli/workspace/msg":
        if m == "folder":
            return 200, {"attempts": 1, "deliveryId": "m_folder%d" % n, "success": False,
                         "reason": "folder_trust_pending", "hint": "Waiting for the owner to trust the recipient's folder."}
        return 200, {"success": True, "deliveryId": "m_%d" % n, "target_session_id": "s1", "attempts": 1}
    if path == "/cli/thread/ask":
        if m == "notfound":
            return 404, {"ok": False, "error": {"code": "not_found", "hint": "unknown overlay addr 'history'"}}
        return 200, {"ok": True, "id": "card_%d" % n, "seq": 10 + n}
    if path == "/cli/thread/post":
        return 200, {"ok": True, "id": "t_%d" % n, "seq": 20 + n, "from": "me"}
    if path == "/cli/inbox/deliver":
        return 200, {"ok": True, "id": "inbox_%d" % n, "title": "note", "filename": "note.md"}
    return 404, {"error": "route not found"}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    via = "tcp"
    def log_message(self, *a):
        pass
    def address_string(self):
        return self.via
    def reply(self, status, obj):
        body = json.dumps(obj, separators=(",", ":")).encode()
        try:
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass  # the CLI gave up on this try (its request limit)
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        u = urlsplit(self.path)
        path = u.path
        if path == "/boot-status":
            feats = [] if open(FEATURESF).read().strip() == "none" else ["post-idempotency-v1", "send-once"]
            return self.reply(200, {"view": "full", "phase": "ready", "features": feats})
        if path == "/__reset":
            # A new section: delivery counts (and ids) start again at 1.
            with lock:
                deliveries.clear()
                with open(LOG + ".deliveries", "w") as f:
                    json.dump(deliveries, f)
            return self.reply(200, {"ok": True})
        key = key_of(body)
        with log_lock, open(LOG, "a") as f:
            f.write("%s %s %s key=%s\n" % (self.via, self.command, path, key or "-"))
        if path in ("/cli/workspace/msg", "/cli/thread/ask", "/cli/thread/post", "/cli/inbox/deliver"):
            if key is None:
                status, obj = deliver(path)
                return self.reply(status, obj)
            with lock:
                slot = answers.get((path, key))
                first = slot is None
                if first:
                    slot = {"done": threading.Event(), "answer": None}
                    answers[(path, key)] = slot
            if first:
                slot["answer"] = deliver(path)
                slot["done"].set()
                status, obj = slot["answer"]
                return self.reply(status, obj)
            slot["done"].wait(10)
            status, obj = slot["answer"]
            obj = dict(obj)
            obj["duplicate" if path == "/cli/thread/post" else "replayed"] = True
            return self.reply(status, obj)
        if self.via == "uds":
            return self.reply(404, {"error": "route not served on cell socket"})
        # Every other route (the sender-name lookups) refuses the passport
        # with the permission object, as the daemon does while cards are on.
        self.reply(403, {"error": {"code": "owner_only", "hint": "only the server's owner may do this"},
                         "permission": {"requestable": False, "where": "a person does this"}})
    do_GET = handle_any
    do_POST = handle_any

class HU(H):
    via = "uds"

class US(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    def get_request(self):
        r, _ = super().get_request()
        return r, ("uds", 0)

us = US(SOCK, HU)
threading.Thread(target=us.serve_forever, daemon=True).start()
srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
srv.daemon_threads = True
with open(PORTF, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
MODE="$SANDBOX/mode"
SOCK="$SANDBOX/cell.sock"
FEATURES="$SANDBOX/features"
: > "$LOG"
echo fast > "$MODE"
echo send-once > "$FEATURES"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" "$MODE" "$SOCK" "$FEATURES" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && [ -S "$SOCK" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] && [ -S "$SOCK" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2" "$SANDBOX/ws/.k2"
printf '# note\n\nhello\n' > "$SANDBOX/ws/note.md"
cd "$SANDBOX/ws"

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $*" >&2; fail=$((fail + 1)); }

# run [sock|tcp] <args…>: an agent in a K2 terminal (passport; the cell
# socket with `sock`). Sets OUT, ERR, RC, LAST (the last line of both).
run() {
    local via="$1"; shift
    : > "$LOG"
    local -a env_extra=()
    [ "$via" = sock ] && env_extra=(K2_HOOK_SOCK="$SOCK")
    set +e
    env K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN=passport-once K2_PROJECT_PATH="$SANDBOX/ws" \
        _K2_POST_MAX_TIME="${MAXT:-30}" _K2_IDEM_TRIES="${TRIES:-3}" ${env_extra[@]+"${env_extra[@]}"} \
        "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    RC=$?
    set -e
    OUT="$(cat "$SANDBOX/out")"
    ERR="$(cat "$SANDBOX/err")"
    LAST="$(cat "$SANDBOX/out" "$SANDBOX/err" | sed '/^$/d' | tail -1)"
}
deliveries() {
    python3 -c 'import json,sys
try:
    d = json.load(open(sys.argv[1]))
except FileNotFoundError:
    d = {}
print(d.get(sys.argv[2], 0))' "$LOG.deliveries" "$1"
}
# A new section: the stub's delivery counts start again at 1.
reset_stub() { curl -s -o /dev/null "http://127.0.0.1:${PORT}/__reset"; }
# The keys the sends to <path> carried, one per line.
keys_for() { grep " $1 key=" "$LOG" | sed 's/.* key=//'; }
no_footer() {
    local label="$1"
    if printf '%s\n%s\n' "$OUT" "$ERR" | grep -q "Ask with:\|It can.t be requested"; then
        bad "$label: a permission footer under a send that wasn't refused: $ERR"
    else
        ok
    fi
}

echo "== 1. no answer, then the same key again: one delivery, its deliveryId =="
reset_stub
echo slow_first > "$MODE"
MAXT=1 run sock msg beta "ping once"
[ "$RC" = 0 ] && ok || bad "msg after a slow first try: exit $RC ($ERR)"
printf '%s' "$OUT" | grep -q '"deliveryId":"m_1"' && ok || bad "msg: the first delivery's id: $OUT"
[ "$(deliveries /cli/workspace/msg)" = 1 ] && ok || bad "msg: delivered $(deliveries /cli/workspace/msg) times, want 1"
n_tries="$(keys_for /cli/workspace/msg | wc -l | tr -d ' ')"
[ "$n_tries" -ge 2 ] && ok || bad "msg: want a retry, saw $n_tries tries: $(cat "$LOG")"
n_keys="$(keys_for /cli/workspace/msg | sort -u | wc -l | tr -d ' ')"
k1="$(keys_for /cli/workspace/msg | head -1)"
[ "$n_keys" = 1 ] && [ "$k1" != "-" ] && ok || bad "msg: every try carries one key, saw: $(keys_for /cli/workspace/msg | tr '\n' ' ')"
[ "$(grep -c "^uds POST /cli/workspace/msg" "$LOG" || true)" -ge 2 ] && ! grep -q "^tcp POST /cli/workspace/msg" "$LOG" && ok \
    || bad "msg: every try stays on the cell socket (a POST that may have run is not resent over TCP): $(cat "$LOG")"
printf '%s' "$ERR" | grep -q "already had this send" && ok || bad "msg: the replay note: $ERR"
no_footer "msg after a retry"

echo "== 2. still no answer: 'may have been delivered' + the key; rerun with it: no duplicate =="
reset_stub
echo hang > "$MODE"
MAXT=1 TRIES=2 run tcp msg beta "ping hang"
[ "$RC" = 1 ] && ok || bad "msg with no answer: exit $RC, want 1 ($ERR)"
key="$(keys_for /cli/workspace/msg | head -1)"
printf '%s' "$ERR" | grep -q "may have been delivered" && ok || bad "msg: no 'may have been delivered' line: $ERR"
printf '%s' "$ERR" | grep -qF -- "--idempotency-key $key" && ok || bad "msg: the line names the key it sent ($key): $ERR"
[ "$(keys_for /cli/workspace/msg | sort -u | wc -l | tr -d ' ')" = 1 ] && ok || bad "msg: both tries carry one key: $(cat "$LOG")"
no_footer "msg with no answer"
sleep 3   # the stub's first try finishes
echo fast > "$MODE"
run tcp msg beta "ping hang" --idempotency-key "$key"
[ "$RC" = 0 ] && ok || bad "rerun with the key: exit $RC ($ERR)"
printf '%s' "$OUT" | grep -q '"deliveryId":"m_1"' && ok || bad "rerun: the first delivery's id: $OUT"
printf '%s' "$OUT" | grep -q '"replayed":true' && ok || bad "rerun: the answer is the replay: $OUT"
[ "$(keys_for /cli/workspace/msg)" = "$key" ] && ok || bad "rerun: sent the given key: $(cat "$LOG")"
[ "$(deliveries /cli/workspace/msg)" = 1 ] && ok || bad "rerun delivered again: $(deliveries /cli/workspace/msg) deliveries"

echo "== 3. a bad --idempotency-key is refused before anything is sent =="
reset_stub
run tcp msg beta "ping" --idempotency-key "no spaces allowed"
[ "$RC" = 2 ] && ok || bad "bad key: exit $RC, want 2 ($ERR)"
grep -q "/cli/workspace/msg" "$LOG" && bad "bad key: a send went out: $(cat "$LOG")" || ok

echo "== 4. two invocations are two sends (new keys) =="
reset_stub
echo fast > "$MODE"
run tcp msg beta "one"
k_a="$(keys_for /cli/workspace/msg)"
run tcp msg beta "two"
k_b="$(keys_for /cli/workspace/msg)"
[ -n "$k_a" ] && [ "$k_a" != "-" ] && [ "$k_a" != "$k_b" ] && ok || bad "each invocation has its own key: '$k_a' '$k_b'"

echo "== 5. the footer: a successful msg ends with its deliveryId =="
reset_stub
run sock msg beta "hello"
[ "$RC" = 0 ] && ok || bad "msg: exit $RC ($ERR)"
printf '%s' "$LAST" | grep -q '"deliveryId":"m_' && ok || bad "msg: the last line is not the answer: $LAST"
no_footer "successful msg"

echo "== 6. the footer: a not-delivered msg (waits on folder trust) has none =="
reset_stub
echo folder > "$MODE"
run sock msg beta "hello"
[ "$RC" = 2 ] && ok || bad "folder-trust msg: exit $RC, want 2 ($ERR)"
printf '%s' "$OUT" | grep -q '"deliveryId":"m_folder1"' && ok || bad "folder-trust msg: $OUT"
no_footer "folder-trust msg"

echo "== 7. thread ask: one card per key; success ends with the card id =="
reset_stub
echo slow_first > "$MODE"
MAXT=1 run sock thread ask k2 "Ship it?" --options "Ship,Wait"
[ "$RC" = 0 ] && ok || bad "ask after a slow first try: exit $RC ($ERR)"
[ "$(deliveries /cli/thread/ask)" = 1 ] && ok || bad "ask: posted $(deliveries /cli/thread/ask) cards, want 1"
[ "$(keys_for /cli/thread/ask | sort -u | wc -l | tr -d ' ')" = 1 ] && [ "$(keys_for /cli/thread/ask | head -1)" != "-" ] && ok \
    || bad "ask: every try carries one key: $(cat "$LOG")"
printf '%s' "$OUT" | grep -q '^card_1  card #11$' && ok || bad "ask: the first card: $OUT"
no_footer "ask after a retry"
echo fast > "$MODE"
run sock thread ask k2 "Ship it?" --options "Ship,Wait"
[ "$RC" = 0 ] && ok || bad "ask: exit $RC ($ERR)"
printf '%s' "$LAST" | grep -q '^card_[0-9]*  card #[0-9]*$' && ok || bad "ask: the last line is the card: $LAST"
no_footer "successful ask"

echo "== 8. thread ask to an unknown address: its error, no footer =="
reset_stub
echo notfound > "$MODE"
run sock thread ask history "Ship it?" --options "Ship,Wait"
[ "$RC" = 2 ] && ok || bad "ask not_found: exit $RC, want 2 ($ERR)"
printf '%s' "$ERR" | grep -q "not_found" && ok || bad "ask not_found: the error: $ERR"
no_footer "ask not_found"

echo "== 9. thread post: clientMsgId, the same on a retry =="
reset_stub
echo slow_first > "$MODE"
MAXT=1 run sock thread k2 "the report is ready"
[ "$RC" = 0 ] && ok || bad "thread post after a slow first try: exit $RC ($ERR)"
[ "$(deliveries /cli/thread/post)" = 1 ] && ok || bad "thread post: posted $(deliveries /cli/thread/post) times"
[ "$(keys_for /cli/thread/post | sort -u | wc -l | tr -d ' ')" = 1 ] && [ "$(keys_for /cli/thread/post | head -1)" != "-" ] && ok \
    || bad "thread post: every try carries one clientMsgId: $(cat "$LOG")"
printf '%s' "$OUT" | grep -q '^t_1  seq=21' && ok || bad "thread post: the first post: $OUT"
no_footer "thread post"

echo "== 10. msg --inbox-wake: one package per key =="
reset_stub
echo slow_first > "$MODE"
MAXT=1 run sock msg beta --inbox-wake ./note.md
[ "$RC" = 0 ] && ok || bad "inbox-wake after a slow first try: exit $RC ($ERR)"
[ "$(deliveries /cli/inbox/deliver)" = 1 ] && ok || bad "inbox-wake: $(deliveries /cli/inbox/deliver) packages, want 1"
[ "$(keys_for /cli/inbox/deliver | sort -u | wc -l | tr -d ' ')" = 1 ] && [ "$(keys_for /cli/inbox/deliver | head -1)" != "-" ] && ok \
    || bad "inbox-wake: every try carries one key: $(cat "$LOG")"
printf '%s' "$OUT" | grep -q '"id":"inbox_1"' && ok || bad "inbox-wake: the first package: $OUT"
no_footer "inbox-wake"

echo "== 11. a daemon without send-once: one send, no retry, no TCP resend, check first =="
reset_stub
echo none > "$FEATURES"
echo slow_first > "$MODE"
MAXT=1 run sock msg beta "ping old daemon"
[ "$RC" = 1 ] && ok || bad "old daemon, no answer: exit $RC, want 1 ($ERR)"
n_sends="$(grep -c " /cli/workspace/msg key=" "$LOG" || true)"
[ "$n_sends" = 1 ] && ok || bad "old daemon: want exactly one send (no retry, no TCP resend), saw $n_sends: $(cat "$LOG")"
grep -q "^uds POST /cli/workspace/msg" "$LOG" && ok || bad "old daemon: the one send went over the cell socket: $(cat "$LOG")"
printf '%s' "$ERR" | grep -q "may have been delivered; check before resending" && ok || bad "old daemon: the check-first line: $ERR"
printf '%s' "$ERR" | grep -q -- "--idempotency-key" && bad "old daemon: offers a key it can't honour: $ERR" || ok
no_footer "old daemon, no answer"
sleep 2
echo fast > "$MODE"
MAXT=1 run tcp thread ask k2 "Ship it?" --options "Ship,Wait"
[ "$RC" = 0 ] && ok || bad "old daemon, answered ask: exit $RC ($ERR)"
[ "$(grep -c " /cli/thread/ask key=" "$LOG" || true)" = 1 ] && ok || bad "old daemon ask: one send: $(cat "$LOG")"
echo send-once > "$FEATURES"

echo
echo "msg_send_once: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
