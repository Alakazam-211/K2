#!/usr/bin/env bash
# k2 ci evidence and k2 ci provenance write files under --out whose names
# come from the daemon's answer: the bundle's file name, each statement
# label, node key and roster key, and each job label (the folder a job's
# files are fetched into). A python stub daemon on loopback answers each
# run with a crafted answer and records every request.
#
# Covers: a name that is absolute, has a ".." part, holds a newline or a
# NUL, or reaches a link inside --out (a linked folder, a linked file) is
# refused with exit 3 and one code: hint line, and NOTHING is written:
# not under --out, not beside it, and --out itself is not made; no other
# request is sent (no fetch). One bad name among good ones still writes
# nothing. Good names write as before, and a second run into the same
# --out writes the files again.
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

WORK="$(mktemp -d -t k2-ci-names-XXXXXX)"
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
mkdir -p "$WORK/proj" "$WORK/answers"
cd "$WORK/proj"
# Everything a run may touch lives under BOX: --out is $BOX/out, and the
# rest of BOX is the outside that must stay as it was.
BOX="$WORK/box"
mkdir -p "$BOX/elsewhere"
printf 'keep\n' >"$BOX/target.json"

# ── the answers, one per run id ─────────────────────────────────────
python3 - "$WORK/answers" "$BOX" <<'PYEOF'
import json, os, sys
dest, box = sys.argv[1:3]
ENV = {"payloadType": "application/vnd.in-toto+json", "payload": "e30=", "signatures": [{"keyid": "k", "sig": "s"}]}
LINE = json.dumps(ENV, separators=(",", ":")) + "\n"
TRUST = {"nodeKeys": {"fp1": {"pem": "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n"}},
         "statements": [{"body": "{\"node\":1}", "sig": "s"}],
         "owners": [{"rosterKeyFp": "rk1"}], "rosters": [{"seq": 3}]}
def prov(statements=None, trust=None):
    return {"short": "r1", "statements": statements if statements is not None else {"unit": {"short": "j1", "files": 1, "line": LINE}},
            "missing": {}, "trust": trust if trust is not None else TRUST}
def ev(file="evidence-r9.json", bundle_prov=None, jobs=None, receipts=None, trust=None):
    return {"short": "r9", "project": "acme", "version": "1.2.3", "file": file,
            "bundle": {"v": 3, "provenance": bundle_prov if bundle_prov is not None else {"L1": ENV},
                       "receipts": receipts if receipts is not None else {"L1": {"job": "j1"}}},
            "jobs": jobs if jobs is not None else {"L1": {"job": "j1", "role": "build", "build": "b", "short": "j1", "node": "n1", "artifacts": []}},
            "dual": {}, "notes": [], "trust": trust if trust is not None else TRUST}
def job(j):
    return {"job": j, "role": "build", "build": "b", "short": j, "node": "n1", "artifacts": []}
answers = {
    "good-prov": prov(),
    "good-ev": ev(),
    "abs-file": ev(file=box + "/abs.json"),
    "dotdot-file": ev(file="../x.json"),
    "dotdot-label": prov(statements={"../x": {"short": "j", "files": 1, "line": LINE}}),
    "climb-job": ev(jobs={"a/../../x": job("j1")}),
    "dotdot-receipt": ev(receipts={"..": {"job": "j1"}}),
    "newline-label": prov(statements={"a\nb": {"short": "j", "files": 1, "line": LINE}}),
    "nul-node": prov(trust=dict(TRUST, nodeKeys={"a\0b": {"pem": "P"}})),
    "abs-node": prov(trust=dict(TRUST, nodeKeys={box + "/node": {"pem": "P"}})),
    "one-bad-roster": prov(statements={"u%d" % i: {"short": "j", "files": 1, "line": LINE} for i in range(20)},
                           trust=dict(TRUST, owners=[{"rosterKeyFp": "rk1"}, {"rosterKeyFp": box + "/roster"}], rosters=[{}, {}])),
}
for run, a in answers.items():
    with open(os.path.join(dest, run + ".json"), "w") as f:
        json.dump(a, f)
PYEOF

# ── the stub daemon ──────────────────────────────────────────────────
REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, body, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def record(self, body):
        u = urllib.parse.urlparse(self.path)
        with open(work + "/requests.log", "a") as f:
            f.write(json.dumps({"method": self.command, "path": u.path, "body": body}) + "\n")
        return u.path
    def do_GET(self):
        path = self.record("")
        self.reply(json.dumps({"error": {"code": "not_found", "hint": "GET " + path}}).encode(), 404)
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0)).decode()
        path = self.record(body)
        if path in ("/cli/ci/evidence", "/cli/ci/provenance"):
            f = os.path.join(work, "answers", json.loads(body)["run"] + ".json")
            if os.path.isfile(f):
                return self.reply(open(f, "rb").read())
        self.reply(json.dumps({"error": {"code": "not_found", "hint": "POST " + path}}).encode(), 404)
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

# Every path under BOX with its kind and its bytes' digest (or a link's
# target): what "nothing written" is checked against.
snap() {
    python3 - "$BOX" <<'PYEOF'
import hashlib, os, sys
box = sys.argv[1]
for top, dirs, files in os.walk(box):
    dirs.sort()
    for n in sorted(dirs + files):
        p = os.path.join(top, n)
        rel = os.path.relpath(p, box)
        if os.path.islink(p):
            print("link %s -> %s" % (rel, os.readlink(p)))
        elif os.path.isdir(p):
            print("dir  %s" % rel)
        else:
            print("file %s %s" % (rel, hashlib.sha256(open(p, "rb").read()).hexdigest()))
PYEOF
}
nreq() { wc -l <"$REQ_LOG" | tr -d ' '; }
# The exact refusal line (<NUL> in the hint stands for a NUL, which argv
# can't carry).
refusal() {
    python3 -c 'import json, sys; print(json.dumps({"error": "ci_name_unsafe", "hint": sys.argv[1].replace("<NUL>", "\0")}))' "$1"
}

# refused <label> <hint> <args…>: exit 3, the one refusal line on stderr,
# nothing on stdout, nothing under BOX changed, one request (the ci POST).
refused() {
    local label="$1" hint="$2" before n0 got after
    shift 2
    before="$(snap)"
    n0="$(nreq)"
    set +e
    "$K2" "$@" >"$WORK/out" 2>"$WORK/err"
    got=$?
    set -e
    eq "$label: exits 3" "$got" "3"
    eq "$label: says why" "$(cat "$WORK/err")" "$(refusal "$hint")"
    eq "$label: prints nothing" "$(cat "$WORK/out")" ""
    after="$(snap)"
    if [ "$after" = "$before" ]; then
        ok "$label: nothing written"
    else
        bad "$label: written outside the rules: $(diff <(printf '%s\n' "$before") <(printf '%s\n' "$after") | grep '^[<>]' | tr '\n' ';')"
    fi
    eq "$label: only the ci request" "$(($(nreq) - n0))" "1"
}

OUT="$BOX/out"

echo "== refused: nothing written, --out not made =="
refused "an absolute file name" "file name $BOX/abs.json is an absolute path: refused, nothing written to $OUT" \
    ci evidence abs-file --out "$OUT" --no-files
refused "a file name with a .. part" "file name ../x.json has a .. part: refused, nothing written to $OUT" \
    ci evidence dotdot-file --out "$OUT" --no-files
refused "a statement label with a .. part" "statement label ../x has a .. part: refused, nothing written to $OUT" \
    ci provenance dotdot-label --out "$OUT"
refused "a job label a/../../x (the fetch's folder)" "job label a/../../x has a .. part: refused, nothing written to $OUT" \
    ci evidence climb-job --out "$OUT"
refused "a receipt label .." "job label .. has a .. part: refused, nothing written to $OUT" \
    ci evidence dotdot-receipt --out "$OUT" --no-files
refused "a label with a newline" "statement label a
b holds a NUL or a newline: refused, nothing written to $OUT" \
    ci provenance newline-label --out "$OUT"
refused "a node key with a NUL" "node key a<NUL>b holds a NUL or a newline: refused, nothing written to $OUT" \
    ci provenance nul-node --out "$OUT"
refused "an absolute node key" "node key $BOX/node is an absolute path: refused, nothing written to $OUT" \
    ci provenance abs-node --out "$OUT"
refused "one bad roster key among twenty good statements" "roster key $BOX/roster is an absolute path: refused, nothing written to $OUT" \
    ci provenance one-bad-roster --out "$OUT"
[ ! -e "$OUT" ] && ok "--out was never made" || bad "--out was made by a refused answer"

echo "== links inside --out =="
mkdir -p "$OUT"
ln -s ../elsewhere "$OUT/trust"
refused "a linked folder (trust -> outside)" "node key fp1 goes through the link $OUT/trust: refused, nothing written to $OUT" \
    ci provenance good-prov --out "$OUT"
refused "a linked folder, evidence" "node key fp1 goes through the link $OUT/trust: refused, nothing written to $OUT" \
    ci evidence good-ev --out "$OUT" --no-files
[ -z "$(ls -A "$BOX/elsewhere")" ] && ok "nothing in the link's target" || bad "wrote through the link: $(ls -A "$BOX/elsewhere")"
rm "$OUT/trust"
mkdir -p "$OUT/trust"
ln -s ../../target.json "$OUT/trust/owners.json"
refused "a linked file (trust/owners.json -> outside)" "trust/owners.json goes through the link $OUT/trust/owners.json: refused, nothing written to $OUT" \
    ci provenance good-prov --out "$OUT"
eq "the link's target is untouched" "$(cat "$BOX/target.json")" "keep"
rm -rf "$OUT"

echo "== good names write as before =="
want_line='{"payloadType":"application/vnd.in-toto+json","payload":"e30=","signatures":[{"keyid":"k","sig":"s"}]}'
for round in 1 2; do
    set +e
    "$K2" ci provenance good-prov --out "$OUT" >"$WORK/out" 2>"$WORK/err"
    got=$?
    set -e
    eq "provenance (run $round): exits 0" "$got" "0"
    eq "provenance (run $round): no stderr" "$(cat "$WORK/err")" ""
    eq "provenance (run $round): the statement" "$(cat "$OUT/provenance/unit.intoto.jsonl")" "$want_line"
    eq "provenance (run $round): the node key" "$(head -1 "$OUT/trust/nodes/fp1.pem")" "-----BEGIN PUBLIC KEY-----"
    eq "provenance (run $round): the roster" "$(cat "$OUT/trust/rosters/rk1.json")" '{"seq": 3}'
    eq "provenance (run $round): owners.json" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["rosterKeyFp"])' "$OUT/trust/owners.json")" "rk1"
    stmt="$(python3 -c 'import hashlib; print(hashlib.sha256(b"{\"node\":1}").hexdigest())')"
    [ -f "$OUT/trust/statements/$stmt.json" ] && ok "provenance (run $round): the self-statement" || bad "provenance (run $round): the self-statement"
done
for round in 1 2; do
    set +e
    "$K2" ci evidence good-ev --out "$OUT" --no-files >"$WORK/out" 2>"$WORK/err"
    got=$?
    set -e
    eq "evidence (run $round): exits 0" "$got" "0"
    eq "evidence (run $round): the bundle" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["v"])' "$OUT/evidence-r9.json")" "3"
    eq "evidence (run $round): the statement from the bundle" "$(cat "$OUT/provenance/L1.intoto.jsonl")" "$want_line"
done
eq "every file is a plain file" "$(find "$OUT" ! -type f ! -type d | wc -l | tr -d ' ')" "0"
eq "nothing beside --out" "$(cd "$BOX" && ls -A | tr '\n' ' ')" "elsewhere out target.json "

echo
echo "ci_out_names: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
