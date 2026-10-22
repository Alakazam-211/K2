#!/usr/bin/env bash
# Garden access (prd-garden-access-v1 §3 G0a, §8): "off = 0.46.3" — the CLI
# half of the switch-off goldens: tests/golden/garden_access_off/cli_zen.txt.
#
# Captured from integ/0.46.3-r9, BEFORE any Garden access code existed. With
# the switch off the daemon answers as 0.46.3 did, so every byte below must
# stay the same: `k2 zen` help, the Garden and widget verbs a person and an
# agent run (the request line and headers each sends, what it prints, its
# exit code), including the verbs Garden access adds or changes (`garden
# access`, `garden history`, `widget history`, `garden new`, `garden delete`,
# `widget claim`, `proposals`, `widget sign`).
#
# Hermetic, as tests/cli/permission_off_golden.sh: HOME is a throwaway dir,
# every K2_*/K2SO_* var is unset, the daemon is a tiny python stub on
# loopback that answers every request with one fixed refusal and logs the
# request line + headers. No real daemon, no ~/.k2, no keychain.
#
# Normalizer (documented; it never drops a line or a field): the CLI version
# string → <K2_CLI_VERSION>, the stub's port → <PORT>, the sandbox path →
# <SANDBOX> (and its URL-encoded form; percent-escapes upper-cased), a body's
# Content-Length that carries the sandbox path → "<n>+<k>x<SANDBOX>", and
# curl / Python-urllib versions → <VERSION>.
#
# Modes:
#   bash tests/cli/garden_access_off_golden.sh            compare (fails loudly on any diff or a missing fixture)
#   GARDEN_ACCESS_GOLDEN_BLESS=1 bash tests/cli/garden_access_off_golden.sh
#                                                         capture — only from a base WITHOUT Garden access code
#
# Named exceptions (G0a): none on the CLI side.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
GOLDEN="$PROJECT_ROOT/tests/golden/garden_access_off/cli_zen.txt"
BLESS="${GARDEN_ACCESS_GOLDEN_BLESS:-0}"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

CLI_VERSION="$(grep -m1 '^K2_CLI_VERSION=' "$K2" | cut -d'"' -f2)"
[ -n "$CLI_VERSION" ] || { echo "FAIL: no K2_CLI_VERSION in $K2" >&2; exit 1; }

SANDBOX="$(mktemp -d /tmp/k2gao-XXXXXX)"
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
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = sys.argv[1]
BODY = json.dumps({"error": {"code": "owner_only", "hint": "only the server's owner may do this"}}).encode()

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        with open(LOG, "a") as f:
            f.write("> %s %s\n" % (self.command, self.path))
            for k, v in self.headers.items():
                f.write("> %s: %s\n" % (k, v))
            f.write("> body: %s\n" % body)
        self.send_response(403)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(BODY)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(BODY)
    do_GET = handle_any
    do_POST = handle_any
    do_PUT = handle_any
    do_DELETE = handle_any

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
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

# Two homes: an agent's (no owner token on disk) and the person's (the
# daemon's port and owner token on disk, as on their own computer).
AGENT_HOME="$SANDBOX/home-agent"
OWNER_HOME="$SANDBOX/home-owner"
mkdir -p "$AGENT_HOME" "$OWNER_HOME/.k2" "$SANDBOX/ws"
printf '%s' "$PORT" > "$OWNER_HOME/.k2/heartbeat.port"
printf '%s' "owner-golden-token" > "$OWNER_HOME/.k2/heartbeat.token"
cd "$SANDBOX/ws"

normalize() {
    python3 -I -c '
import re, sys
ver, port, sandbox = sys.argv[1], sys.argv[2], sys.argv[3]
from urllib.parse import quote
t = sys.stdin.read()
lines = t.split("\n")
cl_at = None
for i, line in enumerate(lines):
    if re.match(r"> [A-Z]+ /", line):
        cl_at = None
    elif line.lower().startswith("> content-length: "):
        cl_at = i
    elif line.startswith("> body: ") and cl_at is not None:
        k = line.count(sandbox)
        if k:
            n = int(lines[cl_at].split(":", 1)[1].strip()) - k * len(sandbox.encode())
            lines[cl_at] = lines[cl_at].split(":", 1)[0] + ": %d+%dx<SANDBOX>" % (n, k)
        cl_at = None
t = "\n".join(lines)
t = re.sub(r"%[0-9a-fA-F]{2}", lambda m: m.group(0).upper(), t)
for form in (sandbox, quote(sandbox, safe="")):
    t = t.replace(form, "<SANDBOX>")
t = t.replace(ver, "<K2_CLI_VERSION>")
t = re.sub(r"(?<![0-9])" + re.escape(port) + r"(?![0-9])", "<PORT>", t)
t = re.sub(r"curl/[0-9][0-9.]*", "curl/<VERSION>", t)
t = re.sub(r"Python-urllib/[0-9][0-9.]*", "Python-urllib/<VERSION>", t)
sys.stdout.write(t)
' "$CLI_VERSION" "$PORT" "$SANDBOX"
}

GOT="$SANDBOX/got"
: > "$GOT"
cases=0

# capture <mode> -- <args…>
#   offline   help paths; nothing reaches a daemon
#   agent     an agent in a K2 terminal over TCP (K2_PORT + K2_HOOK_TOKEN)
#   owner     the person outside K2 (port + owner token on disk)
capture() {
    local mode="$1"
    shift 2
    local out="$SANDBOX/out" err="$SANDBOX/err" rc
    : > "$LOG"
    set +e
    case "$mode" in
        offline) HOME="$AGENT_HOME" K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2" "$@" </dev/null >"$out" 2>"$err"; rc=$? ;;
        agent) HOME="$AGENT_HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=passport-golden "$K2" "$@" </dev/null >"$out" 2>"$err"; rc=$? ;;
        owner) HOME="$OWNER_HOME" "$K2" "$@" </dev/null >"$out" 2>"$err"; rc=$? ;;
        *) echo "FAIL: bad mode $mode" >&2; exit 1 ;;
    esac
    set -e
    {
        printf '$ [%s] k2' "$mode"
        printf ' %q' "$@"
        printf '\n'
        printf 'exit: %s\n' "$rc"
        printf -- '--- stdout\n'
        cat "$out"
        printf -- '--- stderr\n'
        cat "$err"
        printf -- '--- requests\n'
        cat "$LOG"
        printf '\n'
    } >> "$GOT"
    cases=$((cases + 1))
}

echo "== k2 zen help =="
capture offline -- zen
capture offline -- zen --help
capture offline -- zen help
capture offline -- help zen
capture offline -- zen garden --help
capture offline -- zen widget --help

echo "== Garden and widget verbs, as an agent and as the person =="
for mode in agent owner; do
    capture "$mode" -- zen gardens
    capture "$mode" -- zen garden new "Sales board"
    capture "$mode" -- zen garden delete g-1a2b3c4d
    capture "$mode" -- zen garden rename g-1a2b3c4d "Ops"
    capture "$mode" -- zen garden access g-1a2b3c4d
    capture "$mode" -- zen garden access g-1a2b3c4d ask
    capture "$mode" -- zen garden access g-1a2b3c4d add billing
    capture "$mode" -- zen garden history g-1a2b3c4d
    capture "$mode" -- zen widget history board
    capture "$mode" -- zen widget claim board
    capture "$mode" -- zen widget sign board
    capture "$mode" -- zen proposals
    capture "$mode" -- zen history
done

normalize < "$GOT" > "$SANDBOX/got.norm"
if [ "$BLESS" = "1" ]; then
    mkdir -p "$(dirname "$GOLDEN")"
    cp "$SANDBOX/got.norm" "$GOLDEN"
    echo "GOLDEN-BEGIN cli_zen.txt"
    base64 < "$GOLDEN" | tr -d '\n'
    echo
    echo "GOLDEN-END"
    echo "blessed $GOLDEN ($cases cases)"
    exit 0
fi
if [ ! -f "$GOLDEN" ]; then
    echo "  FAIL: no golden at $GOLDEN (capture it from a base without Garden access)" >&2
    exit 1
fi
if cmp -s "$SANDBOX/got.norm" "$GOLDEN"; then
    echo "garden_access_off_golden: $cases cases, cli_zen.txt identical"
else
    echo "  FAIL: cli_zen.txt differs from 0.46.3:" >&2
    diff -u "$GOLDEN" "$SANDBOX/got.norm" >&2 || true
    exit 1
fi
