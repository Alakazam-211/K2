#!/usr/bin/env bash
# A session whose passport the daemon re-minted ($K2_SECRETS_DIR/.k2-passport)
# keeps working with no tab restart, and a dead passport says what to do.
#
# Covers:
#   1. The re-minted passport is used from the start: the env's is never sent.
#   2. A call refused before the file existed retries once with it and the
#      command succeeds (the daemon was restarting when the call started).
#   3. Ignored: another session's token, a planted link, junk, no dir. The
#      env's passport is sent, and the owner token never is.
#   4. A passport the daemon rejects (whoami agrees) with nothing to switch to:
#      one line says restart this tab; the exit code is the command's own.
#   5. A good passport refused on a route agents can't use: no such line.
#   6. An owner token (no `.`) is never replaced.
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
        echo "  FAIL: $label (found $(printf %q "$needle") in $(printf %q "$hay"))" >&2
        fail=$((fail + 1))
    fi
}

WORK="$(mktemp -d -t k2-passport-healed-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-disk-token-aaaa"
OLD="sess42.oldsecretaaaa"
NEW="sess42.newsecretbbbb"
OTHER="sess99.othersecretcccc"
HINT="This session's passport isn't valid on this daemon. Restart this tab to get a new one."
SECRETS="$WORK/secrets"
PASSPORT="$SECRETS/.k2-passport"
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj" "$SECRETS"
echo "9999" >"$HOME/.k2/heartbeat.port"
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
: >"$WORK/requests.log"

# The daemon: `$WORK/valid` lists the passports it accepts. With
# `$WORK/heal-on-refusal`, the first refusal also writes the new passport into
# the session's folder and accepts it, like an adoption finishing meanwhile.
# With `$WORK/agents-only`, /cli/whoami is the only route the passport may use.
python3 - "$WORK" "$NEW" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, new = sys.argv[1], sys.argv[2]

def flag(name):
    return os.path.exists(os.path.join(work, name))

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        presented = (q.get("token") or [""])[0] or (self.headers.get("Authorization") or "").removeprefix("Bearer ")
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps({"path": parsed.path, "token": presented}) + "\n")
        with open(os.path.join(work, "valid")) as f:
            valid = f.read().split()
        ok = presented in valid
        if ok and flag("agents-only") and parsed.path != "/cli/whoami":
            ok = False
        if not ok and flag("heal-on-refusal"):
            os.remove(os.path.join(work, "heal-on-refusal"))
            with open(os.path.join(work, "secrets", ".k2-passport"), "w") as f:
                f.write(new + "\n")
            with open(os.path.join(work, "valid"), "w") as f:
                f.write(new + "\n")
        if ok:
            data = json.dumps({"ok": True, "delivered": True, "role": "canonical", "workspace": "w", "address": "w", "primary": "w", "session": "sess42"}).encode()
            self.send_response(200)
        else:
            data = json.dumps({"error": "Invalid or missing auth token"}).encode()
            self.send_response(403)
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

# k2 in a session's env, with `$1` as its K2_HOOK_TOKEN.
cell() {
    local token="$1"; shift
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX \
        K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$token" K2_SECRETS_DIR="$SECRETS" \
        K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
requests() { cat "$WORK/requests.log"; }
reset() {
    : >"$WORK/requests.log"
    rm -f "$PASSPORT" "$WORK/heal-on-refusal" "$WORK/agents-only"
    printf '%s\n' "$@" >"$WORK/valid"
}
run() {
    set +e
    "$@" >"$WORK/out" 2>"$WORK/err"
    rc=$?
    set -e
}

echo "== the re-minted passport, from the start =="
reset "$NEW"
printf '%s\n' "$NEW" >"$PASSPORT"
run cell "$OLD" whoami --json
assert_eq "whoami succeeds" "$rc" "0"
assert_contains "the new passport was sent" "$(requests)" "$NEW"
assert_absent "the env's dead passport never was" "$(requests)" "$OLD"
assert_absent "no restart line" "$(cat "$WORK/err")" "$HINT"
assert_absent "the owner token never was" "$(requests)" "$OWNER"

# A k2 this run starts inherits it.
reset "$NEW"
printf '%s\n' "$NEW" >"$PASSPORT"
run cell "$OLD" thread k2/cortana hello
assert_eq "a thread post succeeds" "$rc" "0"
assert_absent "the thread post never sent the dead passport" "$(requests)" "$OLD"

echo "== refused first, the file appears, one retry =="
reset "$NEW"
touch "$WORK/heal-on-refusal"
run cell "$OLD" whoami --json
assert_eq "whoami succeeds on the retry" "$rc" "0"
assert_eq "two requests: the refusal and the retry" "$(grep -c . "$WORK/requests.log")" "2"
assert_contains "the first used the env's passport" "$(sed -n 1p "$WORK/requests.log")" "$OLD"
assert_contains "the retry used the new one" "$(sed -n 2p "$WORK/requests.log")" "$NEW"
assert_absent "no restart line" "$(cat "$WORK/err")" "$HINT"

echo "== what the CLI won't take from that file =="
for case_name in other junk link; do
    reset "$OLD"
    case "$case_name" in
        other) printf '%s\n' "$OTHER" >"$PASSPORT" ;;
        junk) printf 'not a passport at all\n' >"$PASSPORT" ;;
        link) printf '%s\n' "$NEW" >"$WORK/bait"; ln -s "$WORK/bait" "$PASSPORT" ;;
    esac
    run cell "$OLD" whoami --json
    assert_eq "$case_name: the env's passport still works" "$rc" "0"
    assert_contains "$case_name: the env's passport was sent" "$(requests)" "$OLD"
    assert_absent "$case_name: nothing else was sent" "$(requests)" "$OTHER"
    assert_absent "$case_name: not the link's target" "$(requests)" "$NEW"
done

echo "== a dead passport says what to do =="
reset "$NEW"
run cell "$OLD" whoami --json
assert_eq "whoami --json keeps its exit code" "$rc" "1"
assert_contains "one line says restart this tab" "$(cat "$WORK/err")" "$HINT"
assert_eq "exactly one such line" "$(grep -c "isn't valid on this daemon" "$WORK/err")" "1"
assert_absent "the owner token was never sent" "$(requests)" "$OWNER"

# Its other session's folder holds a passport: still dead, still says so.
reset "$NEW"
printf '%s\n' "$OTHER" >"$PASSPORT"
run cell "$OLD" whoami --json
assert_contains "another session's file doesn't help, and it says so" "$(cat "$WORK/err")" "$HINT"

echo "== a good passport on a route agents can't use =="
reset "$OLD"
touch "$WORK/agents-only"
run cell "$OLD" thread k2/cortana hello
assert_absent "no restart line: the passport is fine" "$(cat "$WORK/err")" "$HINT"
assert_contains "whoami was asked" "$(requests)" '"/cli/whoami"'

echo "== an owner token is not a passport =="
reset "$OWNER"
printf '%s\n' "$NEW" >"$PASSPORT"
run cell "$OWNER" whoami --json
assert_eq "owner call succeeds" "$rc" "0"
assert_absent "the file's token was not used" "$(requests)" "$NEW"
assert_absent "no restart line" "$(cat "$WORK/err")" "$HINT"

echo
echo "passport_healed: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
