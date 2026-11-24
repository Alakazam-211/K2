#!/usr/bin/env bash
# k2 compute provenance | verify, k2 ci provenance and the statements and
# trust/ files k2 ci evidence writes (prd-compute-build-attestations-v1
# S3): the thin CLI over /cli/compute/jobs/provenance, /cli/compute/verify,
# /cli/ci/provenance and /cli/ci/evidence. A python stub daemon on loopback
# records every request and answers like the daemon; the daemon side is
# crates/k2-daemon/tests/it_compute/compute_provenance_integration.rs.
#
# Covers: help and the study page with no daemon; usage errors (exit 2,
# nothing sent); provenance prints the .intoto.jsonl line byte for byte,
# --out writes it and names the files, a refusal exits 3 with code: hint;
# verify sends {job} or the file's envelope, --file's sha256 computed here
# and --owner-key, prints each check, exits 0 verified / 1 not; a file that
# isn't one statement line is refused before anything is sent; receipt
# still answers; k2 ci provenance and k2 ci evidence write
# provenance/<label>.intoto.jsonl and trust/ (node keys, self-statements,
# owners, rosters).
#
# No daemon build, no real ~/.k2 (HOME is a temp dir), no network.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }

WORK="$(mktemp -d -t k2-prov-cli-XXXXXX)"
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
mkdir -p "$WORK/proj"
cd "$WORK/proj"

echo "== no daemon: help and study =="
help="$("$K2" compute --help)"
has "help: provenance" "$help" "k2 compute provenance <job> [--out FILE]"
has "help: verify" "$help" "k2 compute verify <job | FILE.intoto.jsonl> [--file PATH] [--owner-key FP]"
has "help: receipt" "$help" "k2 compute receipt <job>"
cihelp="$("$K2" ci --help)"
has "ci help: provenance" "$cihelp" "k2 ci provenance <run> --out <dir> [--json]"
has "ci help: evidence writes statements" "$cihelp" "<dir>/provenance/<label>.intoto.jsonl"
study="$("$K2" study provenance)"
for s in "WHAT IT IS" "WORDS" "GET IT" "CHECK IT" "NO STATEMENT?" "SECRETS AND --env" "in-toto" "SLSA Provenance v1" "DSSE" \
    "k2 compute verify <job | file.intoto.jsonl> [--file PATH] [--owner-key FP]" "K2 hashes it here" "ci_preview_off" "rotated" "no_statement" "provenance_secret_in_env" "does not prove the node is honest" "trust/nodes/<keyid>.pem"; do
    has "study: $s" "$study" "$s"
done
n="$(printf '%s\n' "$study" | wc -l | tr -d ' ')"
if [ "$n" -le 90 ]; then ok "study provenance is bounded ($n lines)"; else bad "study provenance is $n lines (max 90)"; fi
has "catalog lists it" "$("$K2" study)" "provenance"
has "ci-release points to it" "$("$K2" study ci-release)" "k2 study provenance"

# ── the stub daemon ──────────────────────────────────────────────────
REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
ENV = {"payloadType": "application/vnd.in-toto+json", "payload": "eyJfdHlwZSI6IngifQ==", "signatures": [{"keyid": "a" * 64, "sig": "MEUCIQ=="}]}
LINE = json.dumps(ENV, separators=(",", ":")) + "\n"
TRUST = {"owners": [{"id": "owner.example.test", "rosterKeyFp": "e" * 64, "rosterSeq": 3}],
         "rosters": [{"body": "{\"seq\":3}", "sig": "r"}],
         "statements": [{"body": "{\"node\":1}", "sig": "s"}],
         "nodeKeys": {"a" * 64: {"node": "rig7", "pem": "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n"}}}
def checks(ok_all, failed=None):
    c = [{"check": "node key", "ok": True, "detail": "signed by rig7 (key aaaaaaaa…aaaa), the key this server pinned when the node enrolled"},
         {"check": "receipt", "ok": True, "detail": "the job's receipt (v1) verifies with the same key"},
         {"check": "statement", "ok": ok_all, "detail": "the signature verifies" if ok_all else failed}]
    return c
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, obj, status=200):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def record(self, body):
        u = urllib.parse.urlparse(self.path)
        with open(work + "/requests.log", "a") as f:
            f.write(json.dumps({"method": self.command, "path": u.path, "query": u.query, "body": body,
                                "auth": self.headers.get("Authorization") or ""}) + "\n")
        return u.path, dict(urllib.parse.parse_qsl(u.query))
    def do_GET(self):
        path, q = self.record("")
        if path == "/cli/compute/jobs/provenance":
            if q.get("job") == "nostmt":
                return self.reply({"error": {"code": "no_statement", "hint": "job nostmt has no build statement: the job kept no files"}}, 409)
            return self.reply({"job": "job1" + "0" * 28, "short": "job10000", "node": "rig7", "keyid": "a" * 64,
                               "builderId": "https://k2.dev/compute/builder/v1/owner/" + "e" * 64,
                               "subjects": [{"name": "dist/acme", "sha256": "4" * 64}], "file": "job10000.intoto.jsonl",
                               "line": LINE, "envelope": ENV, "trust": TRUST})
        if path == "/cli/compute/jobs/receipt":
            return self.reply({"job": "job10000", "verified": True, "receipt": {"v": 1, "artifacts": []}, "body": "{}", "signature": "x",
                               "node_public_key_pem": None, "plan": None, "proves": "which node said it ran which inputs"})
        self.reply({"error": {"code": "not_found", "hint": "GET " + path}}, 404)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        path, q = self.record(body)
        v = json.loads(body or "{}")
        if path == "/cli/compute/verify":
            bad = v.get("job") == "tampered"
            out = {"job": "job1" + "0" * 28, "short": "job10000", "node": "rig7", "verified": not bad,
                   "failed": "provenance: the signature does not verify with that node key" if bad else None,
                   "checks": checks(not bad, "provenance: the signature does not verify with that node key"),
                   "notChecked": "whether the node is honest, and whether this server's records are: a release signer checks again with keys it pinned itself"}
            return self.reply(out)
        if path == "/cli/ci/provenance":
            return self.reply({"workspace": "proj", "run": "ab12cd34" + "0" * 24, "short": "ab12cd34",
                               "statements": {"unit": {"job": "job1" + "0" * 28, "short": "job10000", "line": LINE, "files": 1}},
                               "missing": {"lint": "job lint0000 has no build statement: the job kept no files"}, "trust": TRUST})
        if path == "/cli/ci/evidence":
            return self.reply({"workspace": "proj", "run": "ab12cd34" + "0" * 24, "short": "ab12cd34", "version": "1.2.0", "project": "acme",
                               "file": "evidence-acme-1.2.0.json", "bundle": {"v": 3, "provenance": {"L1": ENV}},
                               "jobs": {"L1": {"job": "job1" + "0" * 28, "short": "job10000", "build": "linux-x86_64", "role": "build", "node": "rig7", "artifacts": []}},
                               "dual": {}, "notes": [], "trust": TRUST})
        self.reply({"error": {"code": "not_found", "hint": "POST " + path}}, 404)
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

# The last request to <path>: its body (POST) or query (GET).
last() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"] == sys.argv[2]]
if not rows:
    sys.exit("no request to " + sys.argv[2])
r = rows[-1]
print(r["body"] if r["method"] == "POST" else r["query"])' "$REQ_LOG" "$1"
}
# qparam <path> <key>: one query value of the last request to <path>,
# or <absent> when the key is not there.
qparam() {
    python3 -c '
import json, sys, urllib.parse
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"] == sys.argv[2]]
if not rows:
    sys.exit("no request to " + sys.argv[2])
q = urllib.parse.parse_qs(rows[-1]["query"], keep_blank_values=True)
print(q[sys.argv[3]][0] if sys.argv[3] in q else "<absent>")' "$REQ_LOG" "$1" "$2"
}
# last_auth <path>: the Authorization header of the last request to <path>.
last_auth() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"] == sys.argv[2]]
if not rows:
    sys.exit("no request to " + sys.argv[2])
print(rows[-1]["auth"])' "$REQ_LOG" "$1"
}
field() { python3 -c 'import json,sys; v=json.loads(sys.argv[1]).get(sys.argv[2]); print(v if isinstance(v, str) else json.dumps(v))' "$1" "$2"; }

# rc <label> <want> <args…>: the exit code; stdout in $WORK/out, stderr in $WORK/err.
rc() {
    local label="$1" want="$2" got
    shift 2
    set +e
    "$K2" "$@" >"$WORK/out" 2>"$WORK/err"
    got=$?
    set -e
    eq "$label exits $want" "$got" "$want"
}

echo "== usage errors send nothing =="
before="$(wc -l <"$REQ_LOG")"
rc "provenance with no job" 2 compute provenance
rc "verify with no job" 2 compute verify
rc "verify --out" 2 compute verify job1 --out x
rc "provenance --owner-key" 2 compute provenance job1 --owner-key "$(printf 'e%.0s' $(seq 1 64))"
rc "an unknown flag" 2 compute verify job1 --frob
rc "two jobs" 2 compute verify job1 job2
rc "--file that isn't there" 2 compute verify job1 --file "$WORK/missing.bin"
printf '{"a":1}\n{"b":2}\n' >"$WORK/two.intoto.jsonl"
rc "a file with two lines" 2 compute verify "$WORK/two.intoto.jsonl"
has "two lines: says why" "$(cat "$WORK/err")" "holds 2 lines"
printf 'not json\n' >"$WORK/junk.intoto.jsonl"
rc "a file that isn't JSON" 2 compute verify "$WORK/junk.intoto.jsonl"
rc "ci provenance with no --out" 2 ci provenance ab12cd34
eq "nothing was sent" "$(wc -l <"$REQ_LOG")" "$before"

echo "== k2 compute provenance =="
"$K2" compute provenance job1 >"$WORK/line.out"
want_line="$(python3 -c 'import json; print(json.dumps({"payloadType": "application/vnd.in-toto+json", "payload": "eyJfdHlwZSI6IngifQ==", "signatures": [{"keyid": "a" * 64, "sig": "MEUCIQ=="}]}, separators=(",", ":")))')"
eq "prints the line byte for byte" "$(cat "$WORK/line.out")" "$want_line"
eq "one line, newline-terminated" "$(wc -l <"$WORK/line.out" | tr -d ' ')" "1"
eq "asks for the job" "$(qparam /cli/compute/jobs/provenance job)" "job1"
eq "provenance: the token rides the Bearer header" "$(last_auth /cli/compute/jobs/provenance)" "Bearer owner-token-for-test"
eq "provenance: no token in the query" "$(qparam /cli/compute/jobs/provenance token)" "<absent>"
out="$("$K2" compute provenance job1 --out "$WORK/job1.intoto.jsonl")"
eq "--out writes the line" "$(cat "$WORK/job1.intoto.jsonl")" "$want_line"
has "--out names the node" "$out" "signed by node rig7, naming 1 file(s)"
has "--out lists each file" "$out" "dist/acme  sha256 4444"
has "--out says how to check" "$out" "k2 compute verify $WORK/job1.intoto.jsonl"
json_out="$("$K2" compute provenance job1 --json)"
eq "--json: the daemon's answer" "$(field "$json_out" builderId)" "https://k2.dev/compute/builder/v1/owner/$(printf 'e%.0s' $(seq 1 64))"
rc "no statement" 3 compute provenance nostmt
has "no statement: code and why" "$(cat "$WORK/err")" '"error": "no_statement"'
has "no statement: the hint" "$(cat "$WORK/err")" "the job kept no files"

echo "== k2 compute verify =="
rc "verify a job" 0 compute verify job1
eq "sends the job" "$(last /cli/compute/verify)" '{"job": "job1"}'
out="$(cat "$WORK/out")"
has "says verified" "$out" "verified: job job10000's build statement (node rig7)"
has "each check" "$out" "  checked  node key   signed by rig7"
has "what isn't checked" "$out" "not checked: whether the node is honest"
rc "a tampered one" 1 compute verify tampered
has "NOT verified, and why" "$(cat "$WORK/out")" "NOT verified: job job10000's build statement (node rig7): provenance: the signature does not verify"
has "the failed line" "$(cat "$WORK/out")" "  FAILED   statement"
printf 'acme binary\n' >"$WORK/acme"
sha="$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$WORK/acme")"
fp="$(printf 'e%.0s' $(seq 1 64))"
rc "verify a file with --file and --owner-key" 0 compute verify "$WORK/job1.intoto.jsonl" --file "$WORK/acme" --owner-key "$fp"
body="$(last /cli/compute/verify)"
eq "sends the envelope from the file" "$(field "$body" envelope)" "$(python3 -c 'import json,sys; print(json.dumps(json.loads(open(sys.argv[1]).read())))' "$WORK/job1.intoto.jsonl")"
eq "sends the file's sha256 (computed here)" "$(field "$body" sha256)" "$sha"
eq "sends the file's name only" "$(field "$body" file)" "acme"
eq "sends the owner key" "$(field "$body" ownerKey)" "$fp"
eq "no job key with a file" "$(field "$body" job)" "null"
json_out="$("$K2" compute verify job1 --json)"
eq "--json: verified" "$(field "$json_out" verified)" "true"

echo "== k2 compute receipt (moved to its own part) =="
rc "receipt" 0 compute receipt job1
eq "receipt asks for the job" "$(qparam /cli/compute/jobs/receipt job)" "job1"
eq "receipt: no token in the query" "$(qparam /cli/compute/jobs/receipt token)" "<absent>"

echo "== k2 ci provenance =="
rc "ci provenance" 0 ci provenance ab12cd34 --out "$WORK/prov"
eq "the run" "$(field "$(last /cli/ci/provenance)" run)" "ab12cd34"
eq "provenance/unit.intoto.jsonl" "$(cat "$WORK/prov/provenance/unit.intoto.jsonl")" "$want_line"
out="$(cat "$WORK/out")"
has "lists the statement" "$out" "unit             job job10000, 1 file(s) -> provenance/unit.intoto.jsonl"
has "lists the one without" "$out" "lint             no statement: job lint0000 has no build statement: the job kept no files"
has "says where trust/ is" "$out" "to check them in $WORK/prov/trust/"
has "trust: the node key" "$(cat "$WORK/prov/trust/nodes/$(printf 'a%.0s' $(seq 1 64)).pem")" "BEGIN PUBLIC KEY"
eq "trust: owners.json" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0]["rosterSeq"])' "$WORK/prov/trust/owners.json")" "3"
eq "trust: the roster" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sig"])' "$WORK/prov/trust/rosters/$fp.json")" "r"
stmt="$(python3 -c 'import hashlib; print(hashlib.sha256(b"{\"node\":1}").hexdigest())')"
eq "trust: the self-statement, by digest" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sig"])' "$WORK/prov/trust/statements/$stmt.json")" "s"

echo "== k2 ci evidence writes the statements and trust/ =="
rc "ci evidence" 0 ci evidence ab12cd34 --out "$WORK/rel" --no-files
eq "provenance/L1.intoto.jsonl from the bundle" "$(cat "$WORK/rel/provenance/L1.intoto.jsonl")" "$want_line"
[ -f "$WORK/rel/evidence-acme-1.2.0.json" ] && ok "the bundle file" || bad "the bundle file"
[ -f "$WORK/rel/trust/nodes/$(printf 'a%.0s' $(seq 1 64)).pem" ] && ok "trust/ beside it" || bad "trust/ beside it"
has "says so" "$(cat "$WORK/out")" "build statements: 1 in $WORK/rel/provenance/"

echo
echo "compute_provenance_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
