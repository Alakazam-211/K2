#!/usr/bin/env bash
# `k2 doc cores on|off <core>` prints its success line for every core the
# daemon knows, and exits 0.
#
# The cores are read from crates/k2-core/src/docs_cores.rs (the CORES
# table), so a new core there is tested here without editing this file.
# A python stub daemon answers POST /cli/settings/update the way the real
# one does (multiplayerCores plus multiplayerCoresReport, the core in
# "changed") and records each body. The CLI runs as a person: no session
# passport in the env, the owner token from $HOME/.k2/heartbeat.token.
#
# No daemon build, no real ~/.k2 (HOME is a temp dir), no network.
# Requires python3; a missing python3 is a FAIL, not a skip.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
CORES_RS="$PROJECT_ROOT/crates/k2-core/src/docs_cores.rs"

fail() { echo "FAIL: $*" >&2; exit 1; }
[ -x "$K2" ] || fail "$K2 not found/executable"
[ -f "$CORES_RS" ] || fail "$CORES_RS not found"
command -v python3 >/dev/null || fail "python3 is required"

# A person in a terminal outside K2: no session or identity from the caller.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d -t k2-doc-cores-every-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj"

# Every core id in the CORES table, in its order.
CORES="$(python3 - "$CORES_RS" <<'PY'
import re, sys
src = open(sys.argv[1], encoding="utf-8").read()
consts = dict(re.findall(r'^pub const ([A-Z_]+): &str = "([^"]+)";', src, re.M))
m = re.search(r"^pub const CORES: &\[Core\] = &\[(.*?)^\];", src, re.M | re.S)
if not m:
    sys.exit("no CORES table in " + sys.argv[1])
ids = []
for ref in re.findall(r"\bid: ([A-Z_]+|\"[^\"]+\"),", m.group(1)):
    ids.append(ref.strip('"') if ref.startswith('"') else consts[ref])
print(" ".join(ids))
PY
)"
n_cores="$(printf '%s\n' $CORES | grep -c .)"
[ "$n_cores" -ge 5 ] || fail "read only $n_cores cores from $CORES_RS: $CORES (the parser is out of date)"
case " $CORES " in *" slides "*) ;; *) fail "slides is not among the cores read: $CORES" ;; esac

python3 - "$WORK" <<'PY' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _reply(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        self._reply(404, {"error": "unexpected GET " + self.path})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        u = urllib.parse.urlparse(self.path)
        token = (urllib.parse.parse_qs(u.query).get("token") or [""])[0] or self.headers.get("Authorization", "")
        with open(work + "/requests.log", "a") as f:
            f.write(json.dumps({"path": u.path, "token": token, "body": body}) + "\n")
        if u.path != "/cli/settings/update":
            return self._reply(404, {"error": "unexpected POST " + u.path})
        want = json.loads(body)["multiplayerCores"]
        (core, on), = want.items()
        report = {"changed": [{"core": core, "on": on}], "closed": 0, "written": 0, "failed": [], "paths": []}
        self._reply(200, {"multiplayerCores": want, "multiplayerCoresReport": report})
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || fail "stub daemon never wrote its port"
cp "$WORK/port" "$HOME/.k2/heartbeat.port"
printf 'owner-token-for-test' >"$HOME/.k2/heartbeat.token"
cd "$WORK/proj"

pass=0
for core in $CORES; do
    for action in on off; do
        : >"$WORK/requests.log"
        rc=0
        out="$("$K2" doc cores "$action" "$core" 2>"$WORK/err")" || rc=$?
        err="$(cat "$WORK/err")"
        [ "$rc" = 0 ] || fail "k2 doc cores $action $core: exit $rc; stdout: $out; stderr: $err"
        [ -z "$err" ] || fail "k2 doc cores $action $core: stderr not empty: $err"
        python3 - "$WORK/requests.log" "$core" "$action" <<'PY' || fail "k2 doc cores $action $core: the daemon was not asked for it"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
core, on = sys.argv[2], sys.argv[3] == "on"
posts = [r for r in rows if r["path"] == "/cli/settings/update"]
assert len(posts) == 1, rows
assert json.loads(posts[0]["body"]) == {"multiplayerCores": {core: on}}, posts[0]["body"]
assert posts[0]["token"] in ("owner-token-for-test", "Bearer owner-token-for-test"), posts[0]["token"]
PY
        lower="$(printf '%s' "$out" | tr 'A-Z' 'a-z')"
        case "$lower" in
            "the "*"$core"*" is $action."*) ;;
            *) fail "k2 doc cores $action $core: want \"The ... $core ... is $action.\", got: $out" ;;
        esac
        pass=$((pass + 1))
    done
done
echo "PASS: k2 doc cores on|off: $pass runs, every core the daemon knows ($CORES), exit 0 with the success line"
