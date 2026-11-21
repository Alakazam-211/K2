#!/usr/bin/env bash
# k2 compute fetch writes each artifact into --out with no link followed.
# A python stub daemon on loopback serves one job's two files in chunks
# of a few bytes (so each file is one fresh write and several appends).
#
# Covers: a normal fetch writes every file with its bytes; a second fetch
# into the same --out rewrites them (an old file there is replaced, not
# appended to); a link inside --out on an artifact's way (a linked folder
# pointing outside, or a linked artifact file) is refused with exit 3 and
# one code: hint line, and nothing is written through it (the link's
# target is as it was).
#
# No daemon build, no real ~/.k2 (HOME is a temp dir), no network.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }

WORK="$(mktemp -d -t k2-fetch-nf-XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
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

python3 - "$WORK" <<'PYEOF' &
import base64, hashlib, json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
FILES = {"b.txt": b"bravo bytes\n", "sub/a.txt": b"alpha, a little longer\n"}
STEP = 5
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s\n" % (u.path, q.get("name", "-")))
        status, out = 200, None
        if u.path != "/cli/compute/jobs/artifact" or q.get("job") != "j1":
            status, out = 404, {"error": {"code": "not_found", "hint": u.path}}
        elif "name" not in q:
            out = {"job": "j1", "short": "j1", "attempt": 1,
                   "artifacts": [{"name": k, "bytes": len(v), "sha256": hashlib.sha256(v).hexdigest()} for k, v in FILES.items()]}
        else:
            data = FILES[q["name"]]
            off = int(q.get("offset", "0"))
            chunk = data[off:off + STEP]
            out = {"name": q["name"], "offset": off, "data": base64.b64encode(chunk).decode(),
                   "eof": off + len(chunk) >= len(data), "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
        raw = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
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

# rc <label> <want> <args…>: the exit code; stdout in $WORK/out, stderr in $WORK/err.
rc() {
    local label="$1" want="$2" got
    shift 2
    set +e
    "$K2" "$@" >"$WORK/out" 2>"$WORK/err"
    got=$?
    set -e
    eq "$label: exits $want" "$got" "$want"
}
refusal() {
    python3 -c 'import json, sys; print(json.dumps({"error": "artifact_path_unsafe", "hint": sys.argv[1]}))' "$1"
}

OUT="$WORK/box/out"
mkdir -p "$WORK/box/elsewhere"

echo "== a normal fetch, then again into the same --out =="
rc "fetch" 0 compute fetch j1 --out "$OUT"
eq "fetch: b.txt" "$(cat "$OUT/b.txt")" "bravo bytes"
eq "fetch: sub/a.txt" "$(cat "$OUT/sub/a.txt")" "alpha, a little longer"
eq "fetch: no stderr" "$(cat "$WORK/err")" ""
printf 'an old file, longer than the new one\n' >"$OUT/b.txt"
rc "fetch again" 0 compute fetch j1 --out "$OUT"
eq "fetch again: b.txt rewritten, not appended to" "$(cat "$OUT/b.txt")" "bravo bytes"
eq "fetch again: sub/a.txt rewritten" "$(cat "$OUT/sub/a.txt")" "alpha, a little longer"
eq "fetch again: plain files only" "$(find "$OUT" ! -type f ! -type d | wc -l | tr -d ' ')" "0"
rm -rf "$OUT"

echo "== a linked folder inside --out =="
mkdir -p "$OUT"
ln -s ../elsewhere "$OUT/sub"
rc "a linked folder" 3 compute fetch j1 --out "$OUT"
eq "a linked folder: says why" "$(cat "$WORK/err")" "$(refusal "sub/a.txt: $OUT/sub is a link, not followed; nothing written there")"
eq "a linked folder: nothing in its target" "$(ls -A "$WORK/box/elsewhere")" ""
eq "a linked folder: the link is as it was" "$(readlink "$OUT/sub")" "../elsewhere"
rm -rf "$OUT"

echo "== a linked artifact file inside --out =="
mkdir -p "$OUT"
printf 'keep\n' >"$WORK/box/target.txt"
ln -s ../target.txt "$OUT/b.txt"
rc "a linked file" 3 compute fetch j1 --out "$OUT"
eq "a linked file: says why" "$(cat "$WORK/err")" "$(refusal "b.txt: $OUT/b.txt is a link, not followed; nothing written there")"
eq "a linked file: its target is untouched" "$(cat "$WORK/box/target.txt")" "keep"
eq "a linked file: the link is as it was" "$(readlink "$OUT/b.txt")" "../target.txt"
eq "nothing beside --out" "$(cd "$WORK/box" && ls -A | tr '\n' ' ')" "elsewhere out target.txt "

echo
echo "compute_fetch_nofollow: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
