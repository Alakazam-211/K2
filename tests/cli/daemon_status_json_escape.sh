#!/usr/bin/env bash
# `k2 daemon status --json` is valid JSON whatever its values hold.
#
# Regression (0.46.17): it built the object with printf '%s', so a quote,
# a backslash or a newline in a value (the home folder in the log and
# plist paths, the port file, the PID, the daemon's ptyd answer) broke
# the JSON. Each value now goes through the CLI's JSON string helper
# (_setup_jstr, pure bash).
#
# Rows, on Linux (systemctl) and macOS (launchctl), each output parsed
# with python3's json:
#   - plain values print byte for byte as before (one line, no spaces);
#   - a HOME with a quote and a backslash, a PID and an exit code with
#     both, and a ptyd answer with a quote, a backslash and a newline:
#     one line, valid JSON, every value exactly as given;
#   - a port file with a quote and a backslash: the same.
#
# How: stand-ins first on PATH for systemctl and launchctl report a
# running unit with the PID the row sets; a python stub serves
# /boot-status on a free loopback port. HOME is a temp dir; status only
# reads. Never touches the real ~/.k2 or the running daemon. Loud.
# Run with: bash tests/cli/daemon_status_json_escape.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-daemon-json-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT
STUBS="$WORK/stubs"
mkdir -p "$STUBS"
OS="$(uname -s)"

cat >"$STUBS/systemctl" <<'EOF'
#!/bin/sh
case "$*" in
    "--user cat k2-daemon.service") exit 0 ;;
    *is-active*) echo active ;;
    *MainPID*) printf '%s\n' "$STUB_PID_VALUE" ;;
    *) exit 1 ;;
esac
EOF
cat >"$STUBS/launchctl" <<'EOF'
#!/bin/sh
case "$1" in
    print) printf '\tpid = %s\n\tlast exit code = %s\n' "$STUB_PID_VALUE" "$STUB_EXIT_VALUE" ;;
    *) exit 1 ;;
esac
EOF
chmod +x "$STUBS/systemctl" "$STUBS/launchctl"

BODY_FILE="$WORK/body"
printf '%s' '{}' >"$BODY_FILE"
cat >"$WORK/stub.py" <<'PY'
import os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        data = open(os.environ["BODY_FILE"], "rb").read()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *_a):
        pass
srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
BODY_FILE="$BODY_FILE" python3 "$WORK/stub.py" "$WORK/stub.port" &
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/stub.port" ] && break
    sleep 0.1
done
[ -s "$WORK/stub.port" ] || { echo "FAIL: the stub never wrote its port" >&2; exit 1; }
SPORT="$(cat "$WORK/stub.port")"

# status <home> <pid> <exit code>: k2 daemon status --json; sets OUT, ERR, RC.
status() {
    set +e
    OUT="$(env -i PATH="$STUBS:/usr/bin:/bin" HOME="$1" STUB_PID_VALUE="$2" STUB_EXIT_VALUE="$3" \
        K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2" daemon status --json 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# expect <label> <home> <pid> <exit code> <port> <ptyd> <adopted>: OUT is
# one line of JSON holding exactly these values.
expect() {
    local label="$1"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0): $ERR"
    [ "$(printf '%s\n' "$OUT" | wc -l | tr -d ' ')" = 1 ] && ok "$label: one line" \
        || { bad "$label: not one line"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    if python3 - "$OS" "$OUT" "$2" "$3" "$4" "$5" "$6" "$7" <<'PY'
import json, sys
os_, out, home, pid, code, port, ptyd, adopted = sys.argv[1:]
d = json.loads(out)
want = {"pid": pid, "port": port, "ptyd": ptyd, "adopted": adopted,
        "running": "yes", "token": "no", "log": home + "/.k2/daemon.stdout.log"}
if os_ == "Darwin":
    want.update({"installed": "no", "last_exit_code": code,
                 "plist": home + "/Library/LaunchAgents/dev.k2.daemon.plist"})
else:
    want.update({"installed": "yes", "unit": "k2-daemon.service", "scope": "user"})
bad = {k: (d.get(k), v) for k, v in want.items() if d.get(k) != v}
extra = set(d) - set(want)
if bad or extra:
    print("      | differs: %r, extra keys: %r" % (bad, sorted(extra)), file=sys.stderr)
    sys.exit(1)
PY
    then
        ok "$label: valid JSON, every value as given"
    else
        bad "$label: not the JSON wanted"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2
    fi
}

echo "== plain values: byte for byte as before =="
H="$WORK/plain"
mkdir -p "$H/.k2"
status "$H" 4242 0
if [ "$OS" = "Darwin" ]; then
    WANT="{\"installed\":\"no\",\"running\":\"yes\",\"pid\":\"4242\",\"port\":\"\",\"token\":\"no\",\"last_exit_code\":\"0\",\"plist\":\"$H/Library/LaunchAgents/dev.k2.daemon.plist\",\"log\":\"$H/.k2/daemon.stdout.log\",\"ptyd\":\"\",\"adopted\":\"\"}"
else
    WANT="{\"installed\":\"yes\",\"running\":\"yes\",\"pid\":\"4242\",\"port\":\"\",\"token\":\"no\",\"unit\":\"k2-daemon.service\",\"scope\":\"user\",\"log\":\"$H/.k2/daemon.stdout.log\",\"ptyd\":\"\",\"adopted\":\"\"}"
fi
[ "$OUT" = "$WANT" ] && ok "plain: the same bytes as before" \
    || { bad "plain: bytes differ"; printf 'got:  %s\nwant: %s\n' "$OUT" "$WANT" | sed 's/^/      | /' >&2; }
expect "plain" "$H" 4242 0 "" "" ""

echo "== a quote, a backslash and a newline in the values =="
H="$WORK/ho\"me\\x"
mkdir -p "$H/.k2"
printf '%s\n' "$SPORT" >"$H/.k2/daemon.port"
python3 -c 'import json,sys; open(sys.argv[1],"w").write(json.dumps({"ptyd": "on \"q\" back\\slash\nline two", "adopted": 2}))' "$BODY_FILE"
status "$H" '42"4\2' '1 "x\y"'
expect "odd values" "$H" '42"4\2' '1 "x\y"' "$SPORT" $'on "q" back\\slash\nline two' 2

echo "== a quote and a backslash in the port file =="
H="$WORK/port-odd"
mkdir -p "$H/.k2"
printf '%s\n' '12"3\4' >"$H/.k2/daemon.port"
status "$H" 4242 0
expect "odd port" "$H" 4242 0 '12"3\4' "" ""

echo ""
echo "daemon_status_json_escape: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
