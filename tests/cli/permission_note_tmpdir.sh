#!/usr/bin/env bash
# The permission note with an unusable TMPDIR. The CLI keeps a refusal's
# closing line ("Ask with: k2 permission request ...") in a small file and
# prints it when the command ends, only when the command failed. With
# TMPDIR a file, a read-only folder or a missing folder, the note goes to
# /tmp (or /var/tmp) instead: no redirection error (one used to name the
# shell's pid), the closing line only after a refusal and at its normal
# place (last), never under a success. A usable TMPDIR works as before.
#
# k2 setup is the command: its answer goes through _permission_note. The
# daemon is a stub. The CLI runs with a clean environment (env -i, a temp
# HOME), so it can only reach the stub.
# Run with: bash tests/cli/permission_note_tmpdir.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required" >&2; exit 1; }
[ -d /tmp ] && [ -w /tmp ] || { echo "FAIL: /tmp must be a writable folder for this test" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }

WORK="$(mktemp -d -t k2-note-tmpdir-XXXXXX)"
STUB_PID=""
KEYS=()
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    local k
    for k in ${KEYS[@]+"${KEYS[@]}"}; do rm -f "/tmp/k2-bulletin.$k" "/var/tmp/k2-bulletin.$k"; done
    chmod -R u+w "$WORK" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

PASSPORT="sess5.passport9d0f"
MODE_FILE="$WORK/mode"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit
MODE_FILE = os.environ["MODE_FILE"]
class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def _answer(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        mode, key = (open(MODE_FILE).read().split() + [""])[:2]
        perm = {"command": "k2 permission request setup.show"}
        if mode == "ok":
            # A success that still carries a permission object (a refusal met
            # on the way) and an unread bulletin.
            return self._send(200, json.dumps({"enabled": True, "rows": [], "permission": perm,
                "bulletin": {"day": "2026-11-18", "unread": True, "key": key}}, separators=(",", ":")))
        if mode == "refused":
            return self._send(403, json.dumps({"ok": False, "error": {"code": "forbidden", "hint": "not yours"},
                "permission": perm}, separators=(",", ":")))
        # "passport": the session's passport is refused everywhere (whoami too).
        return self._send(401, '{"error":"Invalid or missing auth token"}')
    def do_GET(self):
        self._answer()
    def do_POST(self):
        self._answer()
    def log_message(self, *_a):
        pass
HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY
PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo ok >"$MODE_FILE"
MODE_FILE="$MODE_FILE" PORT="$PORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/up"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

mkdir -p "$WORK/home/.k2"
printf '%s' "owner-tok-2b7c" >"$WORK/home/.k2/heartbeat.token"
chmod 600 "$WORK/home/.k2/heartbeat.token"
printf '%s' "$PORT" >"$WORK/home/.k2/heartbeat.port"

# The TMPDIR kinds. A read-only folder is still writable by root; the
# result must be the same either way.
mkdir -p "$WORK/tmp-ok" "$WORK/tmp-ro"
chmod 555 "$WORK/tmp-ro"
: >"$WORK/tmp-file"
KINDS=("usable|$WORK/tmp-ok" "a file|$WORK/tmp-file" "read-only|$WORK/tmp-ro" "missing|$WORK/no-such-dir" "empty|")

PYDIR="$(dirname "$PY")"
n=0
run() { # tmpdir, mode, env words..., then --, then the k2 args
    local tmp="$1" mode="$2" key envs=()
    shift 2
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    n=$((n + 1))
    key="$(printf 'beef%04x%04x' "$$" "$n")"
    KEYS+=("$key")
    echo "$mode $key" >"$MODE_FILE"
    BULLETIN_KEY="$key"
    set +e
    out="$(cd "$WORK" && env -i PATH="$PYDIR:/usr/bin:/bin" HOME="$WORK/home" TMPDIR="$tmp" ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" 2>"$WORK/err" </dev/null)"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}

ASK="Ask with: k2 permission request setup.show"
BULLETIN="K2 bulletin for 2026-11-18 is unread: k2 organization bulletin"
OK_BODY_PREFIX='{"enabled":true,"rows":[],"permission":'
for kind in "${KINDS[@]}"; do
    label="${kind%%|*}"; tmp="${kind#*|}"
    echo "== TMPDIR $label =="

    run "$tmp" ok -- setup --json
    assert_eq "$label: success: exit 0" "$rc" "0"
    case "$out" in "$OK_BODY_PREFIX"*) ok "$label: success: the daemon's answer on stdout" ;; *) bad "$label: success: stdout $(printf %q "$out")" ;; esac
    # The bulletin line is a success's own; no closing line, no error.
    assert_eq "$label: success: stderr is the bulletin line only" "$err" "$BULLETIN"

    run "$tmp" ok -- setup --json
    assert_eq "$label: success again: exit 0" "$rc" "0"
    assert_eq "$label: success again: the bulletin line (a new key)" "$err" "$BULLETIN"

    run "$tmp" refused -- setup show
    assert_eq "$label: refused: exit 3" "$rc" "3"
    assert_eq "$label: refused: the error, then the closing line last, once" "$err" "$(printf '%s\n%s' "Error: forbidden: not yours" "$ASK")"
    assert_eq "$label: refused: nothing on stdout" "$out" ""

    run "$tmp" passport K2_PORT="$PORT" K2_HOOK_SOCK="$WORK/cell.sock" K2_HOOK_TOKEN="$PASSPORT" -- setup
    assert_eq "$label: passport refused: exit 3" "$rc" "3"
    assert_eq "$label: passport refused: the error, then the passport line" "$err" "$(printf '%s\n%s' "Error: Invalid or missing auth token" "This session's passport isn't valid on this daemon. Restart this tab to get a new one.")"
done

# The bulletin stamp still limits the line to one per 30 minutes per key.
echo "== the bulletin stamp, TMPDIR a file =="
run "$WORK/tmp-file" ok -- setup --json
k="$BULLETIN_KEY"
assert_eq "first: the bulletin line" "$err" "$BULLETIN"
echo "ok $k" >"$MODE_FILE"
set +e
err2="$(cd "$WORK" && env -i PATH="$PYDIR:/usr/bin:/bin" HOME="$WORK/home" TMPDIR="$WORK/tmp-file" "$K2_CLI" setup --json 2>&1 >/dev/null </dev/null)"
set -e
assert_eq "same key again: no line (the stamp was kept)" "$err2" ""

echo
echo "permission_note_tmpdir: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
