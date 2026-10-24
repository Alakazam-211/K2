#!/usr/bin/env bash
# Tests for the K2 Compute CI gate (scripts/ci/checks.sh, run-checks.sh,
# post-status.sh, k2-ci-poll.sh) with a stub `k2` on PATH, a local bare
# repo as "origin" and a local HTTP stub for GitHub. Nothing here talks to
# a daemon, a node, GitHub or the keychain. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
srv_pid=""
cleanup() { if [ -n "$srv_pid" ]; then kill "$srv_pid" 2>/dev/null; wait "$srv_pid" 2>/dev/null || true; fi; rm -rf "$tmp"; }
trap cleanup EXIT
export PYTHONDONTWRITEBYTECODE=1
unset K2_CI_STATUS_TOKEN K2_CI_DRY_RUN || true

echo "checks.sh mirrors checks.yml:"
python3 - "$ROOT/.github/workflows/checks.yml" "$ROOT/scripts/ci/checks.sh" > "$tmp/drift" <<'PY' && pass "every checks.yml run command is in checks.sh" || fail "drift" "$(cat "$tmp/drift")"
import re, sys
yml = open(sys.argv[1]).read().splitlines()
sh = " ".join(open(sys.argv[2]).read().replace("\\\n", " ").split())
cmds, i = [], 0
while i < len(yml):
    line = yml[i]
    m = re.match(r"^(\s*)run:\s*(.*)$", line)
    if m:
        indent, rest = len(m.group(1)), m.group(2).strip()
        if rest and rest != "|":
            cmds.append(rest)
        else:
            i += 1
            while i < len(yml) and (not yml[i].strip() or len(yml[i]) - len(yml[i].lstrip()) > indent):
                cmds.append(yml[i].strip())
                i += 1
            continue
    i += 1
missing = []
for c in cmds:
    # Comments, and the runner's own apt setup (nodes get packages via --needs).
    if not c or c.startswith("#") or c.startswith("sudo ") or c.startswith("build-essential"):
        continue
    if " ".join(c.split()) not in sh:
        missing.append(c)
for m in missing:
    print("missing from checks.sh: " + m)
sys.exit(1 if missing else 0)
PY
out="$(bash "$ROOT/scripts/ci/checks.sh" nope 2>&1 || true)"
[[ "$out" == *usage* ]] && pass "checks.sh refuses an unknown part" || fail "checks usage" "$out"

echo "post-status.sh:"
SHA="$(git -C "$ROOT" rev-parse HEAD)"
out="$(K2_CI_DRY_RUN=1 bash "$ROOT/scripts/ci/post-status.sh" "$SHA" success k2-compute/web "job abc on k2n4-vin, 9m")"
[[ "$out" == *'"state": "success"'* && "$out" == *'"context": "k2-compute/web"'* ]] && pass "dry run prints the status" || fail "dry run" "$out"
for bad in "nothex success k2-compute/web d" "$SHA green k2-compute/web d" "$SHA success web d" "$SHA success k2-compute/a/b d"; do
    # shellcheck disable=SC2086
    rc=0; K2_CI_DRY_RUN=1 bash "$ROOT/scripts/ci/post-status.sh" $bad >/dev/null 2>&1 || rc=$?
    [ "$rc" = 2 ] && pass "refused: $bad" || fail "not refused: $bad" "rc=$rc"
done
mkdir -p "$tmp/nokey"
printf '#!/bin/sh\nexit 44\n' > "$tmp/nokey/security"; chmod +x "$tmp/nokey/security"
rc=0; out="$(PATH="$tmp/nokey:$PATH" bash "$ROOT/scripts/ci/post-status.sh" "$SHA" pending k2-compute/web x 2>&1)" || rc=$?
[ "$rc" = 1 ] && [[ "$out" == *"no statuses token"* ]] && pass "no token: refuses loudly" || fail "no token" "rc=$rc $out"
# A local GitHub stub: records the request.
cat > "$tmp/srv.py" <<'PY'
import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        open(sys.argv[2], "a").write(json.dumps({"path": self.path, "auth": self.headers.get("Authorization"), "body": json.loads(body)}) + "\n")
        self.send_response(201); self.end_headers(); self.wfile.write(b"{}")
    def log_message(self, *a): pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
open(sys.argv[1], "w").write(str(s.server_port))
s.serve_forever()
PY
python3 "$tmp/srv.py" "$tmp/port" "$tmp/reqs" &
srv_pid=$!
for _ in $(seq 50); do [ -s "$tmp/port" ] && break; sleep 0.1; done
API="http://127.0.0.1:$(cat "$tmp/port")"
K2_CI_API="$API" K2_CI_STATUS_TOKEN="tok-secret" bash "$ROOT/scripts/ci/post-status.sh" "$SHA" failure k2-compute/rust-test "job x: exit 100" >/dev/null
python3 - "$tmp/reqs" "$SHA" <<'PY' && pass "posts state/context/description with the bearer token" || fail "post" "$(cat "$tmp/reqs")"
import json, sys
r = json.loads(open(sys.argv[1]).read().splitlines()[-1])
assert r["path"] == "/repos/Alakazam-211/K2/statuses/" + sys.argv[2], r
assert r["auth"] == "Bearer tok-secret", r
assert r["body"] == {"state": "failure", "context": "k2-compute/rust-test", "description": "job x: exit 100"}, r
assert "target_url" not in r["body"]
PY

echo "run-checks.sh (stub k2):"
mkdir -p "$tmp/bin"
cat > "$tmp/bin/k2" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
echo "k2 $*" >> "$STUB_CALLS"
[ "$1" = compute ] || exit 2
case "$2" in
    run)
        part="${*: -1}"
        if [ "${STUB_REFUSE:-}" = "$part" ]; then
            echo '{"error": "not_granted", "hint": "no grant"}' >&2; exit 3
        fi
        n=$(( $(cat "$STUB_DIR/n" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$STUB_DIR/n"
        job="$(printf 'cijob%03d%s' "$n" abcdef0123456789abcdef0123)"
        echo "$part" > "$STUB_DIR/$job"
        node="$3"; [ "$node" = --any ] && node=k2n9-any
        echo "$node" > "$STUB_DIR/$job.node"
        printf '{"job": "%s"}\n' "$job"
        ;;
    wait) exit 0 ;;
    get)
        job="$3"; part="$(cat "$STUB_DIR/$job")"
        ex=0; st=done
        [ "${STUB_FAIL:-}" = "$part" ] && ex=100
        [ "${STUB_LOST:-}" = "$part" ] && { st=interrupted; ex=null; }
        printf '{"job": "%s", "short": "%s", "node": "%s", "state": "%s", "exit": %s, "durationSecs": 600, "reason": "node_lost"}\n' \
            "$job" "${job:0:8}" "$(cat "$STUB_DIR/$job.node")" "$st" "$ex"
        ;;
    *) exit 2 ;;
esac
STUB
chmod +x "$tmp/bin/k2"
export K2_BIN="$tmp/bin/k2" K2_CI_DRY_RUN=1

run_checks() {
    local name="$1"; shift
    export STUB_DIR="$tmp/s-$name" STUB_CALLS="$tmp/c-$name" K2_CI_STATE="$tmp/state-$name"
    mkdir -p "$STUB_DIR"; : > "$STUB_CALLS"
    local rc=0
    bash "$ROOT/scripts/ci/run-checks.sh" "$SHA" "$@" > "$tmp/o-$name" 2>&1 || rc=$?
    echo "$rc"
}
final() { grep "context\": \"k2-compute/$2\"" "$tmp/o-$1" | tail -1; }

rc="$(run_checks ok)"
[ "$rc" = 0 ] && pass "all parts pass → exit 0" || fail "ok rc=$rc" "$(cat "$tmp/o-ok")"
for p in web rust-check rust-test; do
    [[ "$(final ok "$p")" == *'"state": "success"'* ]] && pass "$p: success status" || fail "$p success" "$(final ok "$p")"
done
grep -q "k2 compute run k2n4-vin --sha $SHA .*-- bash scripts/ci/checks.sh web" "$tmp/c-ok" \
    && grep -q "k2 compute run k2n6-hil --sha $SHA .*-- bash scripts/ci/checks.sh rust-test" "$tmp/c-ok" \
    && pass "parts go to their nodes at the exact sha" || fail "submit lines" "$(cat "$tmp/c-ok")"
[ "$(grep -c '"state": "pending"' "$tmp/o-ok")" -ge 3 ] && pass "pending posted before the results" || fail "pending" "$(cat "$tmp/o-ok")"
python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); assert v["ok"] and len(v["parts"])==3, v' "$tmp/state-ok/runs/$SHA.json" \
    && pass "run record written" || fail "record" "$(cat "$tmp/state-ok/runs/$SHA.json")"
! grep -q 'target_url' "$tmp/o-ok" && pass "no target_url (logs stay in K2)" || fail "target_url" "$(cat "$tmp/o-ok")"

export STUB_FAIL=rust-test
rc="$(run_checks fail)"
unset STUB_FAIL
[ "$rc" = 1 ] && [[ "$(final fail rust-test)" == *'"state": "failure"'* ]] && [[ "$(final fail rust-test)" == *"exit 100"* ]] \
    && pass "failing tests → failure status, exit 1" || fail "fail" "rc=$rc $(final fail rust-test)"
[[ "$(final fail web)" == *'"state": "success"'* ]] && pass "the other parts still report success" || fail "others" "$(final fail web)"

export STUB_LOST=rust-check
rc="$(run_checks lost)"
unset STUB_LOST
[ "$rc" = 1 ] && [[ "$(final lost rust-check)" == *'"state": "error"'* ]] && pass "lost node → error status (infra, not failure)" \
    || fail "lost" "rc=$rc $(final lost rust-check)"

export STUB_REFUSE=web
rc="$(run_checks refuse)"
unset STUB_REFUSE
[ "$rc" = 1 ] && [[ "$(final refuse web)" == *'"state": "error"'* ]] && [[ "$(final refuse web)" == *not_granted* ]] \
    && pass "refused submit → error status with the code" || fail "refuse" "rc=$rc $(final refuse web)"

export K2_CI_NODE_WEB=any
rc="$(run_checks any --parts web)"
unset K2_CI_NODE_WEB
[ "$rc" = 0 ] && grep -q 'k2 compute run --any --os linux --arch x86_64 --sha' "$tmp/c-any" && ! grep -q 'checks.sh rust' "$tmp/c-any" \
    && pass "--parts and node 'any'" || fail "any" "$(cat "$tmp/c-any")"

echo "k2-ci-poll.sh (local origin):"
git init -q --bare "$tmp/origin.git"
git clone -q "$tmp/origin.git" "$tmp/ci" 2>/dev/null
git -C "$tmp/ci" -c user.email=t@t -c user.name=t commit -q --allow-empty -m one
git -C "$tmp/ci" push -q origin HEAD:refs/heads/main
git -C "$tmp/ci" -c user.email=t@t -c user.name=t commit -q --allow-empty -m two
git -C "$tmp/ci" push -q origin HEAD:refs/heads/integ/x
git -C "$tmp/ci" push -q origin HEAD:refs/heads/feature/y
MAIN="$(git -C "$tmp/ci" rev-parse HEAD~1)"
INTEG="$(git -C "$tmp/ci" rev-parse HEAD)"
mkdir -p "$tmp/ci/scripts/ci"
cp "$ROOT"/scripts/ci/*.sh "$tmp/ci/scripts/ci/"
export STUB_DIR="$tmp/s-poll" STUB_CALLS="$tmp/c-poll" K2_CI_STATE="$tmp/state-poll"
mkdir -p "$STUB_DIR"; : > "$STUB_CALLS"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p1" 2>&1
grep -q "checking refs/heads/main @ ${MAIN:0:12}" "$tmp/p1" && pass "first pass checks main's tip" || fail "pass 1" "$(cat "$tmp/p1")"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p2" 2>&1
grep -q "checking refs/heads/integ/x @ ${INTEG:0:12}" "$tmp/p2" && pass "second pass checks integ/x" || fail "pass 2" "$(cat "$tmp/p2")"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p3" 2>&1
! grep -q checking "$tmp/p3" && pass "nothing new: no run (feature/* is not watched)" || fail "pass 3" "$(cat "$tmp/p3")"
[ "$(wc -l < "$K2_CI_STATE/seen.tsv" | tr -d ' ')" = 2 ] && [ -s "$K2_CI_STATE/logs/$MAIN.log" ] \
    && pass "seen.tsv and per-sha logs" || fail "state" "$(cat "$K2_CI_STATE/seen.tsv")"
grep -q '"state": "success"' "$K2_CI_STATE/logs/$INTEG.log" && pass "the poller's run posted statuses (dry run)" || fail "poll statuses" "$(cat "$K2_CI_STATE/logs/$INTEG.log")"
# A live lock holds the pass; a dead one is cleared.
git -C "$tmp/ci" -c user.email=t@t -c user.name=t commit -q --allow-empty -m three
git -C "$tmp/ci" push -q origin HEAD:refs/heads/main
mkdir "$K2_CI_STATE/poll.lock"; echo $$ > "$K2_CI_STATE/poll.lock/pid"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p4" 2>&1
grep -q "another run holds" "$tmp/p4" && ! grep -q checking "$tmp/p4" && pass "a live lock: no second run" || fail "live lock" "$(cat "$tmp/p4")"
echo 999999 > "$K2_CI_STATE/poll.lock/pid"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p5" 2>&1
grep -q "checking refs/heads/main" "$tmp/p5" && [ ! -d "$K2_CI_STATE/poll.lock" ] && pass "a dead poller's lock is cleared" || fail "dead lock" "$(cat "$tmp/p5")"

echo "syntax:"
for s in checks.sh run-checks.sh post-status.sh k2-ci-poll.sh; do
    bash -n "$ROOT/scripts/ci/$s" && pass "bash -n $s" || fail "bash -n $s" syntax
done
plutil -lint "$ROOT/scripts/ci/dev.k2.ci-poll.plist" >/dev/null 2>&1 \
    || python3 -c 'import plistlib,sys; plistlib.load(open(sys.argv[1],"rb"))' "$ROOT/scripts/ci/dev.k2.ci-poll.plist" \
    && pass "LaunchAgent plist parses" || fail "plist" "does not parse"

echo ""
echo "ci-gate: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
