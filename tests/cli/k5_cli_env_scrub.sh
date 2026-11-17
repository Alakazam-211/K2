#!/usr/bin/env bash
# The front's hand-over variables stop at the CLI: the prelude reads
# K5_CLI_CALLED_AS, K5_CLI_NOTICE_DONE, K5_CLI_VIA and K5_CLI_WOULD_REFUSE
# into its own variables and takes them out of the environment, so a
# process the CLI starts (here curl, and the python3 of an embedded
# urlopen call, through shims first on PATH) sees none of them, while the
# CLI's own requests still say what they said: X-K5-Cli-Via: fallthrough,
# X-K5-Would-Refuse: 1, the noun (X-K5-Cli-Noun) and, as there is no
# passport and the owner token comes from disk, X-K5-Owner-Source: disk.
#
# Runs against either CLI (_cli_under_test.sh). The four are set for the
# bash; the Rust front sets its own, and counts this call because
# CLAUDECODE=1 is set and there is no passport (the owner token on disk
# is used). A stub daemon on TCP logs the four headers. HOME is a temp dir.
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
            "noun": self.headers.get_all("X-K5-Cli-Noun") or [],
            "owner": self.headers.get_all("X-K5-Owner-Source") or [],
        }
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        data = json.dumps([] if rec["path"] == "/cli/presets/list"
                          else {"ok": True, "success": True, "heartbeats": []}).encode()
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

# The curl and python3 the CLI starts write down which of the four they
# see (name=value, one per line), then run the real program.
for prog in curl python3; do
    real="$(command -v "$prog")"
    cat >"$WORK/shim/$prog" <<SHIM
#!/usr/bin/env bash
for n in K5_CLI_CALLED_AS K5_CLI_NOTICE_DONE K5_CLI_VIA K5_CLI_WOULD_REFUSE; do
    if [ -n "\${!n+x}" ]; then printf '%s=%s\n' "\$n" "\${!n}" >>'$WORK/child.env'; fi
done
echo call >>'$WORK/$prog.calls'
exec '$real' "\$@"
SHIM
    chmod +x "$WORK/shim/$prog"
done

# run <label> <path> <noun> <prog> <args…>: one call through the front's
# four variables; the children <prog> started see none of them and every
# request to <path> carries the four headers.
run() {
    local label="$1" path="$2" noun="$3" prog="$4" heads; shift 4
    : >"$WORK/child.env"
    : >"$WORK/curl.calls"
    : >"$WORK/python3.calls"
    : >"$WORK/requests.log"
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOOK_SOCK -u K2_HOOK_TOKEN \
        -u K5_HOOK_SOCK -u K5_HOOK_TOKEN -u K5_PORT -u K5_PROJECT_PATH -u K5_HOST -u K2_HOST \
        -u K5_NO_OWNER_FALLBACK -u K2_NO_OWNER_FALLBACK -u K2SO_NO_OWNER_FALLBACK \
        -u K2_API_CELL -u K2SO_API_CELL -u K2_REMOTE_TOKEN -u K2_SECRETS_DIR -u K2_AIRGAP \
        HOME="$WORK/home" K2_PORT="$PORT" K2_PROJECT_PATH="$WORK/proj" PATH="$WORK/shim:$PATH" \
        CLAUDECODE=1 K5_CLI_CALLED_AS=k5 K5_CLI_NOTICE_DONE=1 K5_CLI_VIA=rust K5_CLI_WOULD_REFUSE=1 \
        "$K2_CLI" "$@" >"$WORK/out" 2>&1 || bad "$label failed: $(tail -3 "$WORK/out")"

    if [ -s "$WORK/$prog.calls" ]; then
        ok "$label: the CLI started $prog ($(wc -l <"$WORK/$prog.calls" | tr -d ' ') calls)"
    else
        bad "$label: the CLI never started $prog: $(tail -3 "$WORK/out")"
    fi
    if [ ! -s "$WORK/child.env" ]; then
        ok "$label: curl and python3 see none of the four"
    else
        bad "$label: a child sees: $(tr '\n' ' ' <"$WORK/child.env")"
    fi
    heads="$(python3 - "$WORK/requests.log" "$path" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
hits = [r for r in rows if r["path"] == sys.argv[2]]
if not hits:
    sys.exit("no request to %s (log: %s)" % (sys.argv[2], rows))
for r in hits:
    print("|".join(",".join(r[k]) or "-" for k in ("via", "refuse", "noun", "owner")))
PY
)" || { bad "$label: no daemon request: $heads"; heads=""; }
    if [ -n "$heads" ] && [ -z "$(grep -vxF "fallthrough|1|$noun|disk" <<<"$heads")" ]; then
        ok "$label: the request carries X-K5-Cli-Via: fallthrough, X-K5-Would-Refuse: 1, X-K5-Cli-Noun: $noun and X-K5-Owner-Source: disk"
    else
        bad "$label: the request's headers: $(tr '\n' ' ' <<<"$heads"), wanted fallthrough|1|$noun|disk"
    fi
}
run "heartbeat list (curl)" /cli/heartbeat/list heartbeat curl heartbeat list --json
run "preset list (python3 urlopen)" /cli/presets/list preset python3 preset list --json

echo
echo "k5_cli_env_scrub: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
