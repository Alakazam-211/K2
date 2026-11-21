#!/usr/bin/env bash
# A flag that takes a value, as the last word, is a usage error on the
# most-used verbs: "Error: <flag> takes a value" on stderr, exit 2, nothing
# on stdout, nothing sent.
#
# Regression (0.46.17): those option loops read the value with a plain
# `shift 2`. As the last word there is one word left, the shift fails, and
# set -e ends the command on exit 1 with nothing said (or bash's own
# "$2: unbound variable" line). They now call _need_value "$@" first.
#
# Every run is under a time guard (20 s), so a form that spins fails its
# row instead of hanging the job. A python stub daemon on a free loopback
# port records every request. HOME is a temp dir, no real daemon, no
# network. Loud. The stub is known to see a request: the last rows send
# the plain forms and check that it did.
#
# Run with: bash tests/cli/need_value.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-need-value-XXXXXX)"
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

REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"
printf '0.46.3\n' >"$WORK/proj/.k2/.garden-changes-hint"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        out = json.dumps({"success": True, "entries": [], "messages": []}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def do_GET(self):
        self._handle()
    def do_POST(self):
        self._handle()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

# run <args...>: sets OUT, ERR, RC. stdin is empty. A run past 20 s is
# killed (RC 124).
run() {
    local pid killer
    set +e
    "$K2" "$@" </dev/null >"$WORK/out" 2>"$WORK/err" &
    pid=$!
    ( sleep 20; kill -9 "$pid" 2>/dev/null && : >"$WORK/killed" ) &
    killer=$!
    rm -f "$WORK/killed"
    wait "$pid"
    RC=$?
    kill "$killer" 2>/dev/null
    wait "$killer" 2>/dev/null
    [ -e "$WORK/killed" ] && RC=124
    set -e
    OUT="$(cat "$WORK/out")"
    ERR="$(cat "$WORK/err")"
}

# last <args...>: the last word is a value flag. Exit 2, exactly
# "Error: <that flag> takes a value" on stderr, nothing on stdout, nothing
# sent. One PASS line per row; each failed check prints its own FAIL.
rows=0
last() {
    local flag label want before
    for flag in "$@"; do :; done
    label="k2 $*"
    want="Error: $flag takes a value"
    rows=$((rows + 1))
    : >"$REQ_LOG"
    run "$@"
    before=$fail
    [ "$RC" -ne 124 ] || bad "$label: still running after 20 s (killed)"
    [ "$RC" -eq 2 ] || bad "$label: exit $RC (want 2)"
    if [ "$ERR" != "$want" ]; then
        bad "$label: stderr is not \"$want\""
        printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
    fi
    if [ -n "$OUT" ]; then
        bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2
    fi
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request"; sed 's/^/      | /' "$REQ_LOG" >&2
    fi
    if [ "$fail" -eq "$before" ]; then
        ok; echo "  PASS: $label: exit 2, the line, nothing sent"
    fi
}

echo "== msg =="
last msg ws1 hi --from
last msg ws1 hi --command
last msg ws1 hi --wake-timeout
last msg ws1 hi --via
last msg ws1 hi --from me --wake-timeout

echo "== talk =="
for f in --from --command --lines --agent --session --token --answer --respond --answers --wake-timeout; do
    last talk ws1 hi "$f"
done

echo "== thread =="
last thread bob --from
last thread bob --limit
last thread bob --before-seq
last thread bob "hello" --limit 5 --from
last thread unread --kind
last thread export --to

echo "== compute =="
last compute run --tag
last compute run --avoid
last compute run --cache
last compute run --os linux --cpus 2 --only
last compute test --needs

echo "== workspace =="
last workspace remove ws1 --mode
last workspace remove ws1 --apps

echo "== agent =="
last agent complete --agent
last agent complete --agent a1 --file
last agent team link --kind
last agent team link t w --scope
last agent team --workspace
last agent team served --workspace
last agent instances hire r.json --key
last agent instances --key

echo "== vault =="
last vault fetch https://example.invalid/ --max-bytes
last vault fetch https://example.invalid/ --timeout
last vault fetch https://example.invalid/ --dst
last vault fetch https://example.invalid/ --fmt

echo "== app =="
last app pairing setup --ios
last app pairing setup --host
last app requests acme --since
last app key mint acme --label
last app data put acme x --to
last app resources --app
last app resources trends --window
last app access ws --core

echo "== publish =="
last publish subdomain create --target
last publish subdomain point staging --target
last publish subdomain target staging --target

echo "== clone =="
last clone proposals --why
last clone preview ws1 --host
last clone to https://h.example ws1 --token
last clone ws1 --dest

echo "== skin =="
last skin user chat bob --app

echo "== inbox =="
last inbox list --folder
last inbox compose --title
last inbox compose --title t --body

echo "== heartbeat =="
last heartbeat add --name
last heartbeat add --name hb1 --instructions
last heartbeat log --limit
last heartbeat log -n
last heartbeat schedule add --name

echo "== daemon install =="
last daemon install --version
last daemon install --bin-dir
last daemon install --dry-run --manifest-url

echo "== mail =="
last mail watch inboxes --add
last mail watch inboxes --wake

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"
run heartbeat log --limit 3
if [ -s "$REQ_LOG" ]; then
    ok; echo "  PASS: k2 heartbeat log --limit 3 still asks the daemon"
else
    bad "k2 heartbeat log --limit 3 sent no request (rc $RC)"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
fi

echo ""
echo "need_value: $rows rows, $pass passed, $fail failed"
[ "$fail" -eq 0 ]
