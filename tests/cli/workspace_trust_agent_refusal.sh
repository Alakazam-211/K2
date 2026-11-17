#!/usr/bin/env bash
# `k2 workspace trust <folder> --yes|--always|--no` from an agent (a session
# passport): exit 3, and instead of a dead end ("It can't be requested.")
# the refusal says where the owner decides in the app and ends with the
# request line that works: `Ask with: k2 workspace trust request …`. The
# workspace's name comes from the daemon's status when it answers, else a
# plain placeholder. The help says the same.
#
# A python stub daemon answers status; no daemon build, no real ~/.k2.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }
[ -x "$K2" ] || fail "$K2 not found/executable"

WORK="$(mktemp -d -t k2-trust-refusal-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

FOLDER="$WORK/acme ws"
mkdir -p "$FOLDER"
REQ_LOG="$WORK/requests.log"
PORT_FILE="$WORK/port"
: >"$REQ_LOG"

python3 - "$PORT_FILE" "$REQ_LOG" <<'PY' &
import http.server, json, sys
port_file, log = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def _answer(self):
        with open(log, "a") as f:
            f.write(self.command + " " + self.path + "\n")
        if self.path.startswith("/cli/folder-trust/status"):
            body = json.dumps({"state": "undecided", "workspace": "Acme Ops",
                               "decide_in_app": "Settings → Workspaces → Acme Ops → Folder trust"})
            code = 200
        else:
            body, code = json.dumps({"error": "unexpected"}), 500
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    do_GET = _answer
    do_POST = _answer
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
with open(port_file, "w") as f:
    f.write(str(s.server_address[1]))
s.serve_forever()
PY
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$PORT_FILE" ] && break
    sleep 0.1
done
[ -s "$PORT_FILE" ] || fail "the stub daemon never started"

run_refusal() {
    local rc=0
    "$K2" workspace trust "$FOLDER" --yes >"$WORK/out" 2>"$WORK/err" || rc=$?
    [ "$rc" -eq 3 ] || fail "exit $rc, want 3: $(cat "$WORK/err")"
    [ ! -s "$WORK/out" ] || fail "stdout should be empty: $(cat "$WORK/out")"
    grep -q "^Error: only the owner decides folder trust; an agent or a K2 terminal can't\.$" "$WORK/err" \
        || fail "the refusal line: $(cat "$WORK/err")"
    grep -q "It can't be requested" "$WORK/err" && fail "still a dead end: $(cat "$WORK/err")"
    local last
    last="$(tail -n 1 "$WORK/err")"
    [ "$last" = "Ask with: k2 workspace trust request $(printf '%q' "$FOLDER") --why \"<what you need it for>\"" ] \
        || fail "the closing line is the request: $last"
}

# 1. The daemon answers status: the app line names the workspace.
export K2_PORT
K2_PORT="$(cat "$PORT_FILE")"
run_refusal
grep -qF "The owner can also decide in the app: Settings → Workspaces → Acme Ops → Folder trust, or by opening that agent." "$WORK/err" \
    || fail "the app line with the workspace: $(cat "$WORK/err")"
grep -q "^GET /cli/folder-trust/status" "$REQ_LOG" || fail "status was asked: $(cat "$REQ_LOG")"
grep -q "decide" "$REQ_LOG" && fail "an agent's CLI never sends decide: $(cat "$REQ_LOG")"

# 2. No daemon: the same lines, with a placeholder for the workspace.
K2_PORT=1
run_refusal
grep -qF "The owner can also decide in the app: Settings → Workspaces / Agents → (that workspace) → Agent → Folder trust, or by opening that agent." "$WORK/err" \
    || fail "the app line without a daemon: $(cat "$WORK/err")"

# 3. The help says the same.
help="$("$K2" workspace trust --help)"
grep -qF "another workspace's" <<<"$help" || fail "help: request covers another workspace's folder: $help"
grep -qF "Settings → Workspaces / Agents → <workspace> → Agent → Folder trust, or by opening that" <<<"$help" || fail "help: the app line: $help"

echo "PASS: workspace trust refusal for an agent"
