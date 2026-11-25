#!/usr/bin/env bash
# Agent Isolation M1, every verb: in an identity session no command form
# sends the session's passport over loopback TCP.
#
# agent_isolation_passport_socket.sh checks a spread of verbs and each
# transport path. This one runs every form the router has
# (crates/k5-cli/tests/data/router_forms.tsv: each form that is not an
# alias, with its argv when it has one) in an identity session
# (K2_AGENT_CGROUP set, a passport, a live session socket, no owner token
# on disk) against a stub daemon that logs each request's transport.
# Each form runs alone, at most a few seconds, with stand-ins for programs
# that act on the machine first on PATH. The check: not one request that
# carries the passport reaches TCP, and every request it marked
# X-K2-Via-Tcp came over the socket.
#
# Bite: each form that sent a marked request runs again with
# K2_VIA_TCP=1 (the reroute's off-switch), and each must then send its
# passport over TCP.
#
# Skipped: update, app-update and release (they fetch and may replace
# the program under test).
#
# cli-parity-timeout: 5400
# Run with: bash tests/cli/agent_isolation_every_verb.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }
command -v timeout >/dev/null || { echo "FAIL: timeout is required" >&2; exit 1; }
FORMS_FILE="$SCRIPT_DIR/../../crates/k5-cli/tests/data/router_forms.tsv"
[ -f "$FORMS_FILE" ] || { echo "FAIL: $FORMS_FILE is missing" >&2; exit 1; }
PER_FORM="${K2_EVERY_VERB_SECS:-4}"

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-iso-every-XXXXXX)"
SOCK_DIR="$(mktemp -d /tmp/k2ie-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK" "$SOCK_DIR"
}
trap cleanup EXIT

PASSPORT="sess91.passportfe12"
SOCK="$SOCK_DIR/cell.sock"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import json, os, socketserver, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit
LOG_FILE = os.environ["LOG_FILE"]
LOCK = threading.Lock()

def handler(via):
    class H(BaseHTTPRequestHandler):
        def _answer(self, method):
            u = urlsplit(self.path)
            with LOCK, open(LOG_FILE, "a") as f:
                f.write(json.dumps({"via": via, "method": method, "path": u.path,
                                    "auth": self.headers.get("Authorization"),
                                    "query": u.query,
                                    "viaTcp": self.headers.get("X-K2-Via-Tcp")}) + "\n")
            body = b'{"view":"full","phase":"ready","features":[]}' if u.path == "/boot-status" else b'{"ok":true}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def do_GET(self): self._answer("GET")
        def _body(self, method):
            n = int(self.headers.get("Content-Length") or 0)
            if n:
                self.rfile.read(n)
            self._answer(method)
        def do_POST(self): self._body("POST")
        def do_PUT(self): self._body("PUT")
        def do_PATCH(self): self._body("PATCH")
        def do_DELETE(self): self._body("DELETE")
        def log_message(self, *_a): pass
    return H

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    def get_request(self):
        conn, _ = super().get_request()
        return conn, ("uds", 0)

class T(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True

u = U(os.environ["SOCK_PATH"], handler("uds"))
threading.Thread(target=u.serve_forever, daemon=True).start()
T(("127.0.0.1", int(os.environ["PORT"])), handler("tcp")).serve_forever()
PY

PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
: >"$LOG_FILE"
LOG_FILE="$LOG_FILE" PORT="$PORT" SOCK_PATH="$SOCK" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if [ -S "$SOCK" ] && curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/boot-status"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

FAKE_HOME="$WORK/home"
WS="$WORK/ws"
TOOLS="$WORK/stand-ins"
mkdir -p "$FAKE_HOME/.k2" "$WS/.k2" "$TOOLS" "$WORK/tmp"
printf '%s' "$PORT" >"$FAKE_HOME/.k2/heartbeat.port"
printf '%s' "$PORT" >"$FAKE_HOME/.k2/daemon.port"
for t in open xdg-open launchctl systemctl osascript security sudo pkill killall vi vim nano ssh scp docker podman; do
    printf '#!/bin/sh\nexit 1\n' >"$TOOLS/$t"
    chmod 755 "$TOOLS/$t"
done

# Each form that is not an alias: its argv, else the form itself.
"$PY" - "$FORMS_FILE" >"$WORK/forms" <<'PY'
import sys
rows = [l.rstrip("\n").split("\t") for l in open(sys.argv[1]) if not l.startswith("#")]
head, rows = rows[0], rows[1:]
assert head[:3] == ["form", "argv", "alias_of"], head
skip = {"update", "app-update", "release"}
for r in rows:
    form, argv, alias = r[0], r[1], r[2]
    if alias != "-":
        continue
    words = (form if argv == "-" else argv)
    if words.split()[0] in skip:
        continue
    print(words)
PY
total_forms="$(wc -l <"$WORK/forms" | tr -d ' ')"
[ "$total_forms" -gt 1000 ] || bad "only $total_forms forms read from $FORMS_FILE"

run_form() { # words, then extra env words
    local words="$1"
    shift
    : >"$LOG_FILE"
    # shellcheck disable=SC2086
    (cd "$WS" && timeout -k 1 "$PER_FORM" env -i PATH="$TOOLS:/usr/bin:/bin" HOME="$FAKE_HOME" \
        K2_HOME="$FAKE_HOME/.k2" TMPDIR="$WORK/tmp" K2_PROJECT_PATH="$WS" K2_HOOK_SOCK="$SOCK" \
        K2_HOOK_TOKEN="$PASSPORT" K2_AGENT_CGROUP=a-ws1 "$@" \
        "$K2_CLI" $words >/dev/null 2>&1 </dev/null) || true
    # Let a request in flight land in the log.
    sleep 0.05
}
# tcp-passport uds-marked uds-passport tcp-marked, for the log as it is.
tally() {
    "$PY" -c 'import json, sys
t = m = u = tm = 0
for l in open(sys.argv[1]):
    try:
        r = json.loads(l)
    except ValueError:
        continue
    p = r.get("auth") == "Bearer " + sys.argv[2] or ("token=" + sys.argv[2]) in (r.get("query") or "")
    if r["via"] == "tcp" and p:
        t += 1
    if r["via"] == "uds" and r.get("viaTcp") == "1":
        m += 1
    if r["via"] == "uds" and p:
        u += 1
    if r["via"] == "tcp" and r.get("viaTcp") is not None:
        tm += 1
print(t, m, u, tm)' "$LOG_FILE" "$PASSPORT"
}

echo "== $total_forms forms in an identity session"
over_tcp=()
marked_forms=()
passport_forms=0
while IFS= read -r words; do
    run_form "$words"
    read -r t m u tm <<<"$(tally)"
    if [ "$t" -gt 0 ] || [ "$tm" -gt 0 ]; then
        over_tcp+=("$words (passport over TCP: $t, marked over TCP: $tm)")
    fi
    if [ "$u" -gt 0 ]; then passport_forms=$((passport_forms + 1)); fi
    if [ "$m" -gt 0 ]; then marked_forms+=("$words"); fi
done <"$WORK/forms"
echo "  $passport_forms form(s) sent the passport over the socket; ${#marked_forms[@]} of them marked X-K2-Via-Tcp"
if [ "${#over_tcp[@]}" = 0 ]; then
    ok "no form sent its passport over TCP ($total_forms forms)"
else
    bad "${#over_tcp[@]} form(s) sent the passport over TCP:"
    printf '    %s\n' "${over_tcp[@]}" >&2
fi
[ "$passport_forms" -gt 50 ] || bad "only $passport_forms forms sent the passport at all (the stub or the session is wrong)"

echo "== bite: K2_VIA_TCP=1, each form that marked a request"
no_bite=()
for words in "${marked_forms[@]}"; do
    run_form "$words" K2_VIA_TCP=1
    read -r t _m _u _tm <<<"$(tally)"
    [ "$t" -gt 0 ] || no_bite+=("$words")
done
if [ "${#marked_forms[@]}" -gt 0 ] && [ "${#no_bite[@]}" = 0 ]; then
    ok "BITE: all ${#marked_forms[@]} form(s) send the passport over TCP with the reroute off"
else
    bad "BITE: ${#no_bite[@]} of ${#marked_forms[@]} form(s) sent nothing over TCP with the reroute off:"
    printf '    %s\n' "${no_bite[@]}" >&2
fi

echo
echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
