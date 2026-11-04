#!/usr/bin/env bash
# k2 board (Rosson 2026-11-01): what the CLI sends and how it answers,
# against a tiny fake daemon on a free port (never a real daemon, never the
# real ~/.k2). The commands are the ones a blind agent wrote from
# `k2 board --help` and `k2 study board` alone (add, read back, connect,
# delete with the cascade), so this pins that the documented words map to
# the right request bodies, the answers print the new ids, and refusals
# give the documented exit codes. The daemon's rules themselves are tested
# in Rust (k2-core board::tests, k2-daemon board_routes_tests).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
K2="$here/../../cli/k2"
source "$here/_hermetic_cli.sh"
hermetic_cli_env

log="$K2_TEST_HOME/bodies.jsonl"
port_file="$K2_TEST_HOME/port"
python3 - "$log" "$port_file" <<'PY' &
import json, sys, http.server
log, port_file = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        with open(log, "a") as f: f.write(json.dumps({"path": self.path.split("?")[0], "body": body}) + "\n")
        cmd = body.get("cmd")
        status, out = 200, {"ok": True}
        if cmd == "boards":
            out = {"ok": True, "boards": [{"id": "b1a2b3c", "name": "Launch", "project": None, "trashed": False}]}
        elif cmd == "read" and body.get("id") == "h-stale00000":
            status, out = 409, {"ok": False, "error": {"code": "conflict", "hint": "it changed since you read it"}}
        elif cmd == "read" and body.get("id") == "h-newer00000":
            status, out = 409, {"ok": False, "error": {"code": "newer_format", "hint": "a newer K2 made this board"}}
        elif cmd == "read" and body.get("id") == "h-locked0000":
            status, out = 409, {"ok": False, "error": {"code": "locked", "hint": "h-locked0000 is locked"}}
        elif cmd == "read":
            out = {"ok": True, "board": "b1a2b3c", "object": {"id": body["id"], "type": "shape"}}
        elif cmd == "add":
            out = {"ok": True, "board": "b1a2b3c", "rev": 7, "object": {"id": "s-0123456789", "type": "sticky", **body["fields"]}}
        elif cmd == "connect":
            out = {"ok": True, "board": "b1a2b3c", "rev": 8, "object": {"id": "c-0123456789", "type": "connector"}}
        elif cmd == "delete":
            out = {"ok": True, "board": "b1a2b3c", "rev": 9, "deleted": [body["id"], "c-0123456789"]}
        elif cmd == "list" and body.get("board") == "Off":
            status, out = 403, {"ok": False, "error": {"code": "core_off", "hint": "the Board core is off"}}
        elif cmd == "update":
            status, out = 400, {"ok": False, "error": {"code": "too_long", "hint": "text: at most 5000 characters", "max": 5000}}
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
s = http.server.HTTPServer(("127.0.0.1", 0), H)
open(port_file, "w").write(str(s.server_port))
s.serve_forever()
PY
server=$!
trap 'kill $server 2>/dev/null; hermetic_cli_cleanup' EXIT
for _ in $(seq 50); do [ -s "$port_file" ] && break; sleep 0.1; done
export K2_PORT="$(cat "$port_file")"

fail() { echo "FAIL: $*" >&2; exit 1; }
body_of() { sed -n "${1}p" "$log"; }

# The blind agent's commands, word for word.
out="$(bash "$K2" board boards)"
[[ "$out" == *"b1a2b3c  Launch"* ]] || fail "boards: $out"
bash "$K2" board read h-7f3a2b1c9d --board Launch >/dev/null
out="$(bash "$K2" board add sticky --text "Ship the beta" --x 400 --y 300 --color yellow --board Launch)"
[[ "$out" == *'"id": "s-0123456789"'* ]] || fail "add prints the new id: $out"
out="$(bash "$K2" board connect h-7f3a2b1c9d s-0123456789 --label next --board Launch)"
[[ "$out" == *'"id": "c-0123456789"'* ]] || fail "connect prints the new id: $out"
out="$(bash "$K2" board delete h-7f3a2b1c9d --board Launch)"
[ "$out" = "Deleted h-7f3a2b1c9d, c-0123456789" ] || fail "delete lists the cascade: $out"

python3 - "$log" <<'PY' || fail "request bodies"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
assert all(r["path"] == "/cli/zen/data/board" for r in rows), rows
got = [r["body"] for r in rows]
want = [
    {"cmd": "boards"},
    {"cmd": "read", "board": "Launch", "id": "h-7f3a2b1c9d"},
    {"cmd": "add", "board": "Launch", "type": "sticky", "fields": {"text": "Ship the beta", "x": "400", "y": "300", "color": "yellow"}},
    {"cmd": "connect", "board": "Launch", "from": "h-7f3a2b1c9d", "to": "s-0123456789", "fields": {"label": "next"}},
    {"cmd": "delete", "board": "Launch", "id": "h-7f3a2b1c9d"},
]
assert got == want, json.dumps(got, indent=1)
PY

# Refusals keep their documented exit codes.
set +e
bash "$K2" board read h-locked0000 2>/dev/null; [ $? -eq 5 ] || fail "locked is exit 5"
bash "$K2" board read h-stale00000 2>/dev/null; [ $? -eq 1 ] || fail "a conflict is exit 1, as k2 doc"
bash "$K2" board read h-newer00000 2>/dev/null; [ $? -eq 4 ] || fail "newer_format is exit 4"
bash "$K2" board list --board Off 2>/dev/null; [ $? -eq 3 ] || fail "core_off is exit 3"
err="$(bash "$K2" board update s-0123456789 --text x 2>&1 >/dev/null)"; code=$?
[ $code -eq 2 ] || fail "too_long is exit 2 (got $code)"
[[ "$err" == *"Error [too_long]: text: at most 5000 characters"* ]] || fail "the refusal is printed: $err"
bash "$K2" board add 2>/dev/null >/dev/null; [ $? -eq 0 ] || fail "add with no type is a sticky"
bash "$K2" board update 2>/dev/null; [ $? -eq 2 ] || fail "update without an id is usage (exit 2)"
bash "$K2" board zap 2>/dev/null; [ $? -eq 2 ] || fail "an unknown command is usage (exit 2)"
set -e
bash "$K2" board --help | grep -q "an arrow from → to (endCap arrow by default" || fail "help says connect draws an arrow"
bash "$K2" study board | grep -q "Never write board keys with raw k2 store" || fail "study board"
echo "board_cli: ok"
