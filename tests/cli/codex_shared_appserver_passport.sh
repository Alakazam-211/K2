#!/usr/bin/env bash
# Codex shared app-server — the CLI refuses a K2 session passport that is not
# this tab's, with a clear error instead of a bare 401, and never falls back
# to the owner token.
#
# Codex 0.155+ runs tool commands under ONE detached `codex app-server
# --listen unix://` per CODEX_HOME. It keeps the env of whichever K2 session
# started it, so a Codex tool in another tab carries that session's (or a
# dead session's) K2_HOOK_SOCK / K2_HOOK_TOKEN.
#
# Covers:
#   1. Pure ancestry matcher over fake `ps` tables: z3mbpZ (0.161) and
#      teachcast (0.160) shared-server shapes match; a --no-daemon TUI and a
#      private stdio app-server do not.
#   2. e2e: ID verb under a process named like the shared app-server (with
#      CODEX_THREAD_ID) → exit 3 + "shared Codex app-server", no request
#      reaches the daemon, owner token never sent.
#   3. e2e: K2_HOOK_SOCK gone + daemon rejects the scoped token → exit 3 +
#      "credentials are stale"; only the scoped whoami probe was sent.
#   4. e2e: K2_HOOK_SOCK gone but the daemon accepts the token (socket bind
#      failed) → the verb proceeds with the scoped token.
#   5. Open verbs and non-cell shells are untouched.
#
# No daemon build; HTTP is a tiny python stub. HOME is sandboxed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if [[ "$hay" == *"$needle"* ]]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2
        fail=$((fail + 1))
    fi
}
assert_absent() {
    local label="$1" hay="$2" needle="$3"
    if [[ "$hay" != *"$needle"* ]]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (found $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    fi
}

# ── 1. Pure ancestry matcher ─────────────────────────────────────────
echo "== ancestry matcher =="
eval "$(sed -n '/^# BEGIN_CLI_STALE_PASSPORT/,/^# END_CLI_STALE_PASSPORT/p' "$K2_CLI")"

matches() {
    if printf '%s\n' "$2" | _cli_ancestry_has_shared_codex_server "$1"; then echo yes; else echo no; fi
}

# z3mbpZ, Codex 0.161: pid-update-loop parent, unix listener child.
Z3="$(cat <<'EOF'
    1     0 /sbin/launchd
  200     1 /Users/u/.codex/packages/app-server-daemon/releases/0.161.0-x/bin/codex app-server daemon pid-update-loop
  300   200 /Users/u/.codex/packages/app-server-daemon/releases/0.161.0-x/bin/codex app-server --listen unix://
  400   300 /bin/zsh -lc k2 thread k2/cortana hi
  500   400 /bin/bash /usr/local/bin/k2 thread k2/cortana hi
EOF
)"
assert_eq "0.161 shared server matches" "$(matches 500 "$Z3")" "yes"

# teachcast, Codex 0.160: detached managed daemon.
TC="$(cat <<'EOF'
    1     0 /sbin/init
  300     1 codex app-server --listen unix:// --managed-daemon
  400   300 /bin/bash -c k2 msg x hi
  500   400 /bin/bash /usr/bin/k2 msg x hi
EOF
)"
assert_eq "0.160 managed daemon matches" "$(matches 500 "$TC")" "yes"

# K2-launched Codex with --no-daemon: tool shell → TUI → k2-daemon.
ND="$(cat <<'EOF'
    1     0 /sbin/launchd
  100     1 /Applications/K2.app/Contents/MacOS/k2-daemon
  250   100 codex --no-daemon --yolo resume 0199
  400   250 /bin/zsh -lc k2 thread k2/cortana hi
  500   400 /bin/bash /usr/local/bin/k2 thread k2/cortana hi
EOF
)"
assert_eq "--no-daemon TUI does not match" "$(matches 500 "$ND")" "no"

# A private stdio app-server owned by the TUI is this session's.
PS="$(cat <<'EOF'
    1     0 /sbin/launchd
  100     1 /Applications/K2.app/Contents/MacOS/k2-daemon
  250   100 codex --no-daemon
  300   250 codex app-server --listen stdio://
  500   300 /bin/bash /usr/local/bin/k2 msg x hi
EOF
)"
assert_eq "private stdio app-server does not match" "$(matches 500 "$PS")" "no"

# Unknown start pid → no match (never loops).
assert_eq "unknown pid does not match" "$(matches 999 "$Z3")" "no"

# ── e2e setup ────────────────────────────────────────────────────────
WORK="$(mktemp -d -t k2-codex-shared-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-disk-token-aaaa"
SCOPED="sess42.scoped-secret-bbbb"
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj"
echo "9999" >"$HOME/.k2/heartbeat.port"
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
# `reject` present → the stub answers whoami with the daemon's auth refusal.
: >"$WORK/requests.log"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {
            "method": method,
            "path": parsed.path,
            "token_query": (q.get("token") or [""])[0],
            "authorization": self.headers.get("Authorization") or "",
            "body": body.decode("utf-8", "replace"),
        }
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        if parsed.path == "/cli/whoami" and os.path.exists(os.path.join(work, "reject")):
            data = json.dumps({"error": "Invalid or missing auth token"}).encode()
            self.send_response(403)
        else:
            data = json.dumps({"ok": True, "delivered": True, "role": "agent"}).encode()
            self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

GONE_SOCK="$WORK/run/cells/dead-session.sock"

# Run k2 in a cell env. Extra env assignments come first as NAME=VALUE args.
cell() {
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX \
        K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_PROJECT_PATH="$WORK/proj" "$@"
}
requests() { cat "$WORK/requests.log"; }

# ── 2. Under a shared Codex app-server ───────────────────────────────
echo "== under a shared Codex app-server =="
: >"$WORK/requests.log"
# `exec -a` names the parent bash like the shared server; the second
# command keeps bash from exec'ing k2 in its place.
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" CODEX_THREAD_ID="thread-1" K2="$K2_CLI" \
    bash -c 'exec -a "codex app-server --listen unix://" bash -c "\"\$K2\" thread k2/cortana hello; exit \$?"' \
    >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
assert_eq "shared server: exit 3" "$rc" "3"
assert_contains "shared server: names the cause" "$(cat "$WORK/err")" "shared Codex app-server started by another session"
assert_contains "shared server: says what to do" "$(cat "$WORK/err")" "Restart Codex in this tab"
assert_eq "shared server: nothing reached the daemon" "$(requests)" ""
assert_absent "shared server: no bare token error" "$(cat "$WORK/err")" "Invalid or missing auth token"

# Same tree without CODEX_* (not a Codex tool) → no ancestry refusal.
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" K2="$K2_CLI" \
    bash -c 'exec -a "codex app-server --listen unix://" bash -c "\"\$K2\" thread k2/cortana hello; exit \$?"' \
    >"$WORK/out" 2>"$WORK/err"
set -e
assert_absent "no CODEX_* env: no shared-server refusal" "$(cat "$WORK/err")" "shared Codex app-server started by another session"

# ── 3. Dead session: socket gone + token rejected ────────────────────
echo "== dead session passport =="
: >"$WORK/requests.log"
touch "$WORK/reject"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" "$K2_CLI" thread k2/cortana hello >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
assert_eq "dead session: exit 3" "$rc" "3"
assert_contains "dead session: names the cause" "$(cat "$WORK/err")" "credentials are stale"
assert_contains "dead session: names the socket" "$(cat "$WORK/err")" "$GONE_SOCK"
assert_contains "dead session: Codex hint" "$(cat "$WORK/err")" "restart Codex in this tab"
n_req="$(grep -c . "$WORK/requests.log" || true)"
assert_eq "dead session: only the whoami probe was sent" "$n_req" "1"
assert_contains "dead session: probe is whoami" "$(requests)" '"path": "/cli/whoami"'
assert_contains "dead session: probe carries the scoped Bearer" "$(requests)" "Bearer $SCOPED"
assert_absent "dead session: owner token never sent" "$(requests)" "$OWNER"

# ID verb from mail too (locked tool) → same refusal.
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" "$K2_CLI" msg cortana hello >/dev/null 2>"$WORK/err"
rc=$?
set -e
assert_eq "dead session msg: exit 3" "$rc" "3"
assert_absent "dead session msg: owner token never sent" "$(requests)" "$OWNER"

# ── 4. Socket gone but token still valid (bind failed) → proceeds ────
echo "== socket missing, token valid =="
rm -f "$WORK/reject"
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" "$K2_CLI" thread k2/cortana hello >"$WORK/out" 2>"$WORK/err"
set -e
assert_absent "valid token: no stale refusal" "$(cat "$WORK/err")" "credentials are stale"
n_req="$(grep -c . "$WORK/requests.log" || true)"
if [ "$n_req" -ge 2 ]; then
    echo "  PASS: valid token: the verb's own request followed the probe"; pass=$((pass + 1))
else
    echo "  FAIL: valid token: expected probe + verb request, got $n_req ($(requests))" >&2
    fail=$((fail + 1))
fi
assert_absent "valid token: owner token never sent" "$(requests)" "$OWNER"

# ── 5. Open verbs and non-cell shells are untouched ──────────────────
echo "== open verbs / non-cell =="
touch "$WORK/reject"
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" "$K2_CLI" activity >/dev/null 2>"$WORK/err"
set -e
assert_absent "open verb: no stale refusal" "$(cat "$WORK/err")" "credentials are stale"
assert_absent "open verb: no whoami probe" "$(requests)" '"/cli/whoami"'

: >"$WORK/requests.log"
set +e
env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2_HOOK_TOKEN -u K2SO_HOOK_TOKEN -u K2SO_PORT \
    K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WORK/proj" \
    "$K2_CLI" thread k2/cortana hello >/dev/null 2>"$WORK/err"
set -e
assert_absent "external shell: no stale refusal" "$(cat "$WORK/err")" "credentials are stale"
assert_absent "external shell: no whoami probe" "$(requests)" '"/cli/whoami"'

echo
echo "codex_shared_appserver_passport: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
