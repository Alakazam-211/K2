#!/usr/bin/env bash
# The address cap (0.46.18, Rosson: an agent that manages email hosting end
# to end has no imposed inbox count; the owner can still set a cap): the CLI
# half of `k2 hostmail config --workspace <ws> --address-cap <n>|none`.
#
# Hermetic: a throwaway HOME, every K2_*/K2SO_* var unset, and a python stub
# daemon on loopback that answers POST /cli/mail/config/set with the
# daemon's shapes (crates/k2-daemon/src/mail/routes_server.rs) and logs each
# request body. Checks:
#   * --address-cap none rides the wire as the string "none" (never int()),
#     and the answer's hint is printed: "Addresses: unlimited (this agent
#     manages email hosting)";
#   * a number rides as a number, and the hint names the owner's cap;
#   * anything else (None, -1, lots, no value) exits 2 and sends nothing;
#   * the help says none and that a manager has no cap unless one is set.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

fail() { echo "FAIL: $*" >&2; exit 1; }

SANDBOX="$(mktemp -d /tmp/k2cap-XXXXXX)"
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
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = sys.argv[1]

def reply(h, status, obj):
    body = json.dumps(obj).encode()
    h.send_response(status)
    h.send_header("Content-Type", "application/json")
    h.send_header("Content-Length", str(len(body)))
    h.send_header("Connection", "close")
    h.end_headers()
    h.wfile.write(body)

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def handle_any(self):
        u = urllib.parse.urlparse(self.path)
        agent = (self.headers.get("Authorization") or "") == "Bearer agent-passport"
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode() if n else ""
        body = json.loads(raw) if raw else {}
        with open(LOG, "a") as f:
            f.write("%s %s %s\n" % (self.command, u.path, raw))
        if u.path == "/cli/mail/config/set" and "addressCap" in body:
            cap = body["addressCap"]
            if agent and body.get("workspace") == "me":
                return reply(self, 403, {"ok": False, "error": {"code": "owner_only", "hint": "This agent manages email hosting, so it has no address cap unless the owner sets one, and that cap is the owner's to change. Ask your human."},
                                         "permission": {"action": "mail.address-cap", "params": {"workspace": "me", "cap": str(cap)}, "approvers": "owner or admin",
                                                        "command": "k2 permission request mail.address-cap workspace=me cap=%s --why \"<what you need it for>\"" % cap}})
            if cap == "none":
                words = "Addresses: unlimited (this agent manages email hosting)"
            elif isinstance(cap, int):
                words = "Addresses: up to %d (set by the owner for this workspace)" % cap
            else:
                return reply(self, 400, {"ok": False, "error": {"code": "usage", "hint": "'addressCap' must be a non-negative integer (0 = unlimited) or 'none'"}})
            return reply(self, 200, {"ok": True, "applied": {"workspace": {"project": "/w/alpha", "addressCap": cap, "addresses": words}},
                                     "hint": "mail config updated. %s." % words})
        return reply(self, 404, {"ok": False, "error": {"code": "not_found", "hint": "stub: " + u.path}})
    do_GET = handle_any
    do_POST = handle_any

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
: > "$LOG"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || fail "the stub daemon did not start"

export HOME="$SANDBOX/home"
mkdir -p "$HOME"
export K2_PORT="$(cat "$SANDBOX/port")"
export K2_HOOK_TOKEN=owner-token

last_body() { tail -n 1 "$LOG"; }

# ── none clears: a string on the wire, the answer's line printed ────────
out="$("$K2" hostmail config --workspace alpha --address-cap none)"
last_body | grep -q '^POST /cli/mail/config/set .*"addressCap": *"none"' || fail "none must ride as the string \"none\": $(last_body)"
last_body | grep -q '"workspace": *"alpha"' || fail "the workspace must ride along: $(last_body)"
<<<"$out" grep -qx 'mail config updated. Addresses: unlimited (this agent manages email hosting).' || fail "none: the answer's line: $out"

# ── a number: an int on the wire ────────────────────────────────────────
out="$("$K2" hostmail config --workspace alpha --address-cap 12)"
last_body | grep -q '"addressCap": *12[,}]' || fail "a number must ride as a number: $(last_body)"
<<<"$out" grep -qx 'mail config updated. Addresses: up to 12 (set by the owner for this workspace).' || fail "a cap: the answer's line: $out"

# ── anything else: exit 2, nothing sent ─────────────────────────────────
before="$(wc -l < "$LOG")"
for args in "--address-cap None" "--address-cap -1" "--address-cap lots" "--address-cap"; do
    set +e
    # shellcheck disable=SC2086
    out="$("$K2" hostmail config --workspace alpha $args 2>&1)"; rc=$?
    set -e
    [ "$rc" -eq 2 ] || fail "$args must exit 2 (got $rc): $out"
    <<<"$out" grep -q 'none' || fail "$args must say none is the other spelling: $out"
done
[ "$(wc -l < "$LOG")" -eq "$before" ] || fail "a usage error must send nothing: $(cat "$LOG")"

# ── help ────────────────────────────────────────────────────────────────
help="$("$K2" hostmail config --help)"
<<<"$help" grep -q -- '--address-cap <n>|none' || fail "config --help must show none: $help"
<<<"$help" grep -q 'manages email hosting has' || fail "config --help must say a manager has no cap: $help"
"$K2" hostmail create --help | grep -q 'An agent that manages email hosting has no cap unless the owner sets one' \
    || fail "create --help must say a manager has no cap"

echo "PASS: hostmail address cap"
