#!/usr/bin/env bash
# k2 compute fetch on a job with many artifacts gets every file.
#
# A job that kept 464 files died after about 260 of them with
# "python3: Argument list too long": fetch kept the list of fetched files
# as one JSON string and handed it to Python in argv, and one argument
# over 128 KiB fails on Linux. The list now reaches Python on stdin.
#
# A local fake daemon serves 2,000 small artifacts with names like a real
# job's (JUnit files per shard, nextest lists, coverage files, a few names
# with spaces or accents). The CLI must write every file with the right
# bytes, exit 0, print one line per file in name order and nothing on
# stderr. --json (on a 50-file job, to keep the test short) gives the
# files in the same text as before, and a job that kept nothing still
# says so. The argv limit is per argument on Linux, so the 2,000-file run
# fails there with the old code; macOS has no per-argument limit.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; CLI="$CLI_UNDER_TEST"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-fetch-many-XXXXXX)"
FAKE_PID=""
cleanup() {
    if [ -n "$FAKE_PID" ]; then kill "$FAKE_PID" 2>/dev/null || true; wait "$FAKE_PID" 2>/dev/null || true; fi
    rm -rf "$FAKE_DIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

N=2000
cat >"$FAKE_DIR/fake.py" <<'PY'
import base64, hashlib, http.server, json, sys, urllib.parse
log_path, port_path, list_path, n = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
CRATES = ["k2-core", "k2-daemon", "k2-client", "k2-ptyd", "k5-cli"]
def name_of(i):
    c = CRATES[i % len(CRATES)]
    k = i % 4
    if k == 0:
        return "target/nextest/ci/shard-%02d/junit-%s-%04d.xml" % (i % 16, c, i)
    if k == 1:
        return "out/list-%s-run-%04d.json" % (c, i)
    if k == 2:
        return "coverage/%s/src/compute/module_%04d.rs.lcov" % (c, i)
    if i % 100 == 3:
        return "reports/Build Log café %04d.txt" % i
    return "logs/%s/test output %04d.log" % (c, i)
def body_of(i):
    size = 16 + (i * 37) % 400
    out, seed = b"", str(i).encode()
    while len(out) < size:
        seed = hashlib.sha256(seed).digest()
        out += seed
    return out[:size]
FILES = {}
for i in range(n):
    data = body_of(i)
    FILES[name_of(i)] = (data, hashlib.sha256(data).hexdigest())
with open(list_path, "w") as f:
    json.dump([{"name": k, "bytes": len(v[0]), "sha256": v[1]} for k, v in FILES.items()], f)
JOB = "ab12cd34ef"
EMPTY = "ee12cd34ef"
SMALL = "cc12cd34ef"  # the first 50 files, for --json
SMALL_NAMES = list(FILES)[:50]
with open(list_path + ".small", "w") as f:
    json.dump([{"name": k, "bytes": len(FILES[k][0]), "sha256": FILES[k][1]} for k in SMALL_NAMES], f)
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        u = urllib.parse.urlsplit(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        with open(log_path, "a") as f:
            f.write("%s %s\n" % (u.path, q.get("name", "-")))
        status, out = 200, None
        if u.path != "/cli/compute/jobs/artifact" or q.get("job") not in (JOB[:8], EMPTY[:8], SMALL[:8]):
            status, out = 404, {"error": {"code": "not_found", "hint": u.path}}
        elif q.get("job") == EMPTY[:8]:
            out = {"job": EMPTY, "short": EMPTY[:8], "attempt": 1, "artifacts": []}
        elif "name" not in q and q.get("job") == SMALL[:8]:
            out = {"job": SMALL, "short": SMALL[:8], "attempt": 1,
                   "artifacts": [{"name": k, "bytes": len(FILES[k][0]), "sha256": FILES[k][1]} for k in SMALL_NAMES]}
        elif "name" not in q:
            out = {"job": JOB, "short": JOB[:8], "attempt": 1,
                   "artifacts": [{"name": k, "bytes": len(v[0]), "sha256": v[1]} for k, v in FILES.items()]}
        elif q["name"] not in FILES:
            status, out = 404, {"error": {"code": "artifact_not_found", "hint": q["name"]}}
        else:
            data, sha = FILES[q["name"]]
            off, mx = int(q.get("offset", "0")), int(q.get("max", "4194304"))
            chunk = data[off:off + mx]
            out = {"name": q["name"], "offset": off, "data": base64.b64encode(chunk).decode(),
                   "eof": off + len(chunk) >= len(data), "sha256": sha, "bytes": len(data)}
        raw = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
open(port_path, "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$FAKE_DIR/fake.py" "$FAKE_DIR/requests.log" "$FAKE_DIR/port" "$FAKE_DIR/list.json" "$N" &
FAKE_PID=$!
for _ in $(seq 1 100); do [ -s "$FAKE_DIR/port" ] && break; sleep 0.1; done
[ -s "$FAKE_DIR/port" ] || fail "fake daemon did not start"
export K2_PORT="$(cat "$FAKE_DIR/port")"
export K2_HOOK_TOKEN=owner-token

run() {
    set +e
    "$CLI" "$@" >"$FAKE_DIR/stdout" 2>"$FAKE_DIR/stderr"
    RC=$?
    set -e
}

echo "== fetch: $N artifacts =="
OUTDIR="$FAKE_DIR/got"
: >"$FAKE_DIR/requests.log"
run compute fetch ab12cd34 --out "$OUTDIR"
if grep -q "Argument list too long" "$FAKE_DIR/stderr"; then
    fail "fetch hit the argv limit: $(grep -m1 "Argument list too long" "$FAKE_DIR/stderr")"
fi
[ "$RC" = "0" ] || fail "fetch exits 0 (got $RC: $(head -c 2000 "$FAKE_DIR/stderr"))"
[ ! -s "$FAKE_DIR/stderr" ] || fail "fetch prints nothing on stderr (got: $(head -c 2000 "$FAKE_DIR/stderr"))"
python3 -I - "$FAKE_DIR/list.json" "$OUTDIR" "$FAKE_DIR/stdout" "$N" <<'PY' || fail "fetch: every file, its bytes and the printed lines"
import hashlib, json, os, sys
listing, outdir, stdout, n = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
files = json.load(open(listing))
assert len(files) == n, (len(files), n)
names = sorted(f["name"] for f in files)
for f in files:
    p = os.path.join(outdir, *f["name"].split("/"))
    assert os.path.isfile(p), "missing: %s" % p
    data = open(p, "rb").read()
    assert len(data) == f["bytes"], (p, len(data), f["bytes"])
    assert hashlib.sha256(data).hexdigest() == f["sha256"], p
got = sum(len(fs) for _, _, fs in os.walk(outdir))
assert got == n, "%d files written, want %d" % (got, n)
by = {f["name"]: f for f in files}
want = "".join("%s  %d bytes  sha256 %s…\n" % (os.path.join(outdir, *k.split("/")), by[k]["bytes"], by[k]["sha256"][:12]) for k in names)
have = open(stdout, encoding="utf-8").read()
if have != want:
    hl, wl = have.splitlines(), want.splitlines()
    first = next((i for i in range(min(len(hl), len(wl))) if hl[i] != wl[i]), min(len(hl), len(wl)))
    raise AssertionError("stdout differs at line %d of %d (want %d): %r vs %r" % (
        first + 1, len(hl), len(wl), hl[first] if first < len(hl) else None, wl[first] if first < len(wl) else None))
print("fetch: %d files, bytes and sha256 match, %d lines in name order" % (n, len(names)))
PY
asked="$(grep -c '^/cli/compute/jobs/artifact ' "$FAKE_DIR/requests.log")"
[ "$asked" = "$((N + 1))" ] || fail "fetch asks for the list once and each file once ($((N + 1)) requests, got $asked)"

echo "== fetch --json: the same shape (50 files) =="
OUTJ="$FAKE_DIR/gotjson"
run compute fetch cc12cd34 --out "$OUTJ" --json
[ "$RC" = "0" ] || fail "fetch --json exits 0 (got $RC: $(head -c 2000 "$FAKE_DIR/stderr"))"
[ ! -s "$FAKE_DIR/stderr" ] || fail "fetch --json prints nothing on stderr (got: $(head -c 2000 "$FAKE_DIR/stderr"))"
python3 -I - "$FAKE_DIR/list.json.small" "$OUTJ" "$FAKE_DIR/stdout" <<'PY' || fail "fetch --json: job, files, their order and fields"
import json, os, sys
listing, outdir, stdout = sys.argv[1:4]
files = {f["name"]: f for f in json.load(open(listing))}
names = sorted(files)
want = {"job": "cc12cd34", "files": [{"name": k, "path": os.path.join(outdir, *k.split("/")),
                                      "bytes": files[k]["bytes"], "sha256": files[k]["sha256"]} for k in names]}
have = open(stdout, encoding="utf-8").read()
assert have == json.dumps(want, indent=2) + "\n", "the --json answer differs from the expected text"
assert len(names) == 50, len(names)
print("fetch --json: %d files, same text as before" % len(names))
PY

echo "== fetch: a job that kept nothing =="
run compute fetch ee12cd34 --out "$FAKE_DIR/none"
[ "$RC" = "0" ] || fail "fetch of an empty job exits 0 (got $RC: $(cat "$FAKE_DIR/stderr"))"
[ "$(cat "$FAKE_DIR/stdout")" = "(no artifacts kept for this job)" ] || fail "an empty job says so (got: $(cat "$FAKE_DIR/stdout"))"
run compute fetch ee12cd34 --out "$FAKE_DIR/none" --json
[ "$RC" = "0" ] || fail "fetch --json of an empty job exits 0 (got $RC)"
[ "$(cat "$FAKE_DIR/stdout")" = "$(printf '{\n  "job": "ee12cd34",\n  "files": []\n}')" ] || fail "an empty job's --json lists no files (got: $(cat "$FAKE_DIR/stdout"))"

echo "== fetch NAME: one file, and a name the job did not keep =="
one="out/list-k2-daemon-run-0001.json"
run compute fetch ab12cd34 "$one" --out "$FAKE_DIR/one"
[ "$RC" = "0" ] || fail "fetch NAME exits 0 (got $RC: $(cat "$FAKE_DIR/stderr"))"
[ "$(wc -l <"$FAKE_DIR/stdout" | tr -d ' ')" = "1" ] || fail "fetch NAME prints one line (got: $(cat "$FAKE_DIR/stdout"))"
[ -f "$FAKE_DIR/one/$one" ] || fail "fetch NAME writes that file"
run compute fetch ab12cd34 "out/not-kept.json" --out "$FAKE_DIR/one"
[ "$RC" = "3" ] || fail "a name the job did not keep exits 3 (got $RC)"
grep -q '"artifact_not_found"' "$FAKE_DIR/stderr" || fail "and says artifact_not_found (got: $(head -c 300 "$FAKE_DIR/stderr"))"

echo "compute_fetch_many_artifacts: ok"
