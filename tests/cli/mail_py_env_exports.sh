#!/usr/bin/env bash
# _mail_py hands every input to its python3 heredoc as an M_* environment
# variable listed on the python3 command line. A caller's `local M_X=...`
# is NOT exported, so a variable the heredoc reads but the list omits
# arrives empty — silently. That dropped `--in/--at` on `k2 mail send`
# (scheduled mail went out at once), `--label` on `hostmail app-password
# add`, and the selector on `hostmail dkim retire`.
#
# 1. Static: every M_* the heredoc reads must be on the export list.
# 2. Live: those verbs, against a local echo server, must put the value
#    in the request body. No daemon; throwaway HOME.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"

WORK="$(mktemp -d -t k2-mail-py-env-XXXXXX)"
SRV_PID=""
cleanup() {
    if [ -n "$SRV_PID" ]; then kill "$SRV_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT
mkdir -p "$WORK/home"

cat >"$WORK/audit.py" <<'PY'
import re, sys
s = open(sys.argv[1]).read()
i = s.index("_mail_py() {")
j = s.index("python3 - <<'PYEOF'", i)
k = s.index("\nPYEOF", j)
exported = set(re.findall(r'\b(M_[A-Z_]+)="', s[i:j]))
used = set(re.findall(r"""(?:env\(|environ\.get\(|environ\[)["'](M_[A-Z_]+)["']""", s[j:k]))
if not used:
    sys.exit("found no M_* reads in the _mail_py heredoc")
print(" ".join(sorted(used - exported)))
PY
python3 "$WORK/audit.py" "$K2" >"$WORK/missing"
missing="$(cat "$WORK/missing")"
[ -z "$missing" ] || fail "_mail_py reads but does not export: $missing"

cat >"$WORK/srv.py" <<'PY'
import http.server, json, sys
log = open(sys.argv[1], "a")
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        log.write("%s %s\n" % (self.path.split("?")[0], self.rfile.read(n).decode()))
        log.flush()
        out = json.dumps({"ok": True}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
: >"$WORK/port"
: >"$WORK/requests"
python3 "$WORK/srv.py" "$WORK/requests" >"$WORK/port" &
SRV_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -s "$WORK/port" ] || fail "echo server never reported its port"
PORT="$(cat "$WORK/port")"

k2() {
    env -i PATH="$PATH" HOME="$WORK/home" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token \
        "$K2" "$@" >/dev/null
}

k2 hostmail dkim retire example.test sel1 --json
k2 hostmail app-password add a@example.test --label phone --json
k2 mail send --to b@example.test --subject s --body hi --in 2h --json
k2 mail send --to b@example.test --subject s --body hi --at 2030-01-01T09:00:00Z --json

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = [l.split(" ", 1) for l in open(sys.argv[1]).read().splitlines()]
got = [(p, json.loads(b)) for p, b in rows]
want = [
    ("/cli/mail/dkim/retire", "selector", "sel1"),
    ("/cli/mail/app-password", "label", "phone"),
    ("/cli/mail/send", "sendIn", "2h"),
    ("/cli/mail/send", "sendAt", "2030-01-01T09:00:00Z"),
]
if len(got) != len(want):
    sys.exit("FAIL: expected %d requests, got %d: %r" % (len(want), len(got), got))
for (path, body), (wpath, key, val) in zip(got, want):
    if path != wpath:
        sys.exit("FAIL: expected POST %s, got %s" % (wpath, path))
    if body.get(key) != val:
        sys.exit("FAIL: %s body must carry %s=%r, got %r" % (path, key, val, body))
PY
python3 "$WORK/check.py" "$WORK/requests"

echo "OK: _mail_py exports every M_* it reads"
