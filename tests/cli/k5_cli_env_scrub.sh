#!/usr/bin/env bash
# The front's hand-over variables stop at the CLI: the prelude reads
# K5_CLI_CALLED_AS, K5_CLI_NOTICE_DONE, K5_CLI_VIA and K5_CLI_WOULD_REFUSE
# into its own variables and takes them out of the environment, so a
# process the CLI starts (here curl, through a shim first on PATH) sees
# none of them, while the CLI's own request still says what they said:
# X-K5-Cli-Via: fallthrough and X-K5-Would-Refuse: 1.
#
# Runs against either CLI (_cli_under_test.sh). The four are set for the
# bash; the Rust front sets its own, and counts this call because
# CLAUDECODE=1 is set and there is no passport (the owner token on disk
# is used). A stub daemon on TCP logs the two headers. HOME is a temp dir.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"
K2_CLI="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d /tmp/k2scrub-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT
mkdir -p "$WORK/home/.k2" "$WORK/proj" "$WORK/shim"
echo "owner-test-token" >"$WORK/home/.k2/heartbeat.token"
chmod 600 "$WORK/home/.k2/heartbeat.token"
: >"$WORK/requests.log"

python3 - "$WORK" <<'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def _go(self):
        n = int(self.headers.get("Content-Length", 0) or 0)
        if n:
            self.rfile.read(n)
        rec = {
            "path": self.path.split("?")[0],
            "via": self.headers.get_all("X-K5-Cli-Via") or [],
            "refuse": self.headers.get_all("X-K5-Would-Refuse") or [],
        }
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        data = json.dumps({"ok": True, "success": True, "heartbeats": []}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self): self._go()
    def do_POST(self): self._go()
    def log_message(self, *a): pass
srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
PORT="$(cat "$WORK/stub.port")"

# The curl the CLI starts writes down which of the four it sees (name=value,
# one per line), then runs the real curl.
REAL_CURL="$(command -v curl)"
cat >"$WORK/shim/curl" <<SHIM
#!/usr/bin/env bash
for n in K5_CLI_CALLED_AS K5_CLI_NOTICE_DONE K5_CLI_VIA K5_CLI_WOULD_REFUSE; do
    if [ -n "\${!n+x}" ]; then printf '%s=%s\n' "\$n" "\${!n}" >>'$WORK/child.env'; fi
done
echo call >>'$WORK/child.calls'
exec '$REAL_CURL' "\$@"
SHIM
chmod +x "$WORK/shim/curl"

: >"$WORK/child.env"
: >"$WORK/child.calls"
env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK -u K2_HOOK_TOKEN \
    -u K5_HOOK_SOCK -u K5_HOOK_TOKEN -u K5_PORT -u K5_PROJECT_PATH -u K5_HOST -u K2_HOST \
    -u K5_NO_OWNER_FALLBACK -u K2_NO_OWNER_FALLBACK -u K2SO_NO_OWNER_FALLBACK \
    -u K2_API_CELL -u K2SO_API_CELL -u K2_REMOTE_TOKEN -u K2_SECRETS_DIR -u K2_AIRGAP \
    HOME="$WORK/home" K2_PORT="$PORT" K2_PROJECT_PATH="$WORK/proj" PATH="$WORK/shim:$PATH" \
    CLAUDECODE=1 K5_CLI_CALLED_AS=k5 K5_CLI_NOTICE_DONE=1 K5_CLI_VIA=rust K5_CLI_WOULD_REFUSE=1 \
    "$K2_CLI" heartbeat list --json >"$WORK/out" 2>&1 || bad "heartbeat list failed: $(tail -3 "$WORK/out")"

if [ -s "$WORK/child.calls" ]; then
    ok "the CLI started curl ($(wc -l <"$WORK/child.calls" | tr -d ' ') calls)"
else
    bad "the CLI never started curl: $(tail -3 "$WORK/out")"
fi
if [ ! -s "$WORK/child.env" ]; then
    ok "curl sees none of the four"
else
    bad "curl sees: $(tr '\n' ' ' <"$WORK/child.env")"
fi
heads="$(python3 - "$WORK/requests.log" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
hits = [r for r in rows if r["path"] == "/cli/heartbeat/list"]
if not hits:
    sys.exit("no request to /cli/heartbeat/list (log: %s)" % rows)
for r in hits:
    print("%s|%s" % (",".join(r["via"]) or "-", ",".join(r["refuse"]) or "-"))
PY
)" || { bad "no daemon request: $heads"; heads=""; }
if [ -n "$heads" ] && [ -z "$(grep -vxF 'fallthrough|1' <<<"$heads")" ]; then
    ok "the CLI's request carries X-K5-Cli-Via: fallthrough and X-K5-Would-Refuse: 1"
else
    bad "the request's headers: $(tr '\n' ' ' <<<"$heads"), wanted fallthrough|1"
fi

echo
echo "k5_cli_env_scrub: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
