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
    # Comments, and the runner's own setup: apt packages and the pinned
    # cargo-nextest download (nodes get both via --needs).
    if not c or c.startswith("#") or c.startswith("sudo ") or c.startswith("build-essential") \
            or c.startswith("curl -LsSf https://get.nexte.st/"):
        continue
    if " ".join(c.split()) not in sh:
        missing.append(c)
for m in missing:
    print("missing from checks.sh: " + m)
sys.exit(1 if missing else 0)
PY
python3 - "$ROOT/scripts/ci/checks.sh" "$ROOT/.github/workflows/checks.yml" "$ROOT/.config/nextest.toml" > "$tmp/nextest" <<'PY' && pass "tests run under cargo-nextest, profile ci (no retries, JUnit), in both files" || fail "nextest" "$(cat "$tmp/nextest")"
import re, sys, tomllib
bad = []
for path in sys.argv[1:3]:
    text = open(path).read().replace("\\\n", " ")
    for line in text.splitlines():
        cmd = line.strip()
        if cmd.startswith("run:"):
            cmd = cmd[4:].strip()
        if re.match(r"cargo test\b", cmd):
            bad.append("%s: plain cargo test: %s" % (path, cmd))
        if cmd.startswith("cargo nextest run") and not cmd.startswith("cargo nextest run --profile ci "):
            bad.append("%s: nextest without --profile ci: %s" % (path, cmd))
    if "cargo nextest run --profile ci" not in text:
        bad.append("%s: no nextest run at all" % path)
cfg = tomllib.load(open(sys.argv[3], "rb"))
profiles = cfg.get("profile", {})
ci = profiles.get("ci")
if ci is None:
    bad.append("nextest.toml: no [profile.ci]")
else:
    if ci.get("retries") != 0:
        bad.append("nextest.toml: profile ci must have retries = 0, has %r" % ci.get("retries"))
    if ci.get("fail-fast") is not False:
        bad.append("nextest.toml: profile ci must run every test (fail-fast = false)")
    if not ci.get("junit", {}).get("path"):
        bad.append("nextest.toml: profile ci writes no JUnit")
if profiles.get("default", {}).get("retries") != 0:
    bad.append("nextest.toml: profile default must have retries = 0")
for b in bad:
    print(b)
sys.exit(1 if bad else 0)
PY
out="$(bash "$ROOT/scripts/ci/checks.sh" nope 2>&1 || true)"
[[ "$out" == *usage* ]] && pass "checks.sh refuses an unknown part" || fail "checks usage" "$out"

echo "post-status.sh:"
# The scripts under test run from a scratch repo of this test's own (copies
# of scripts/ci), never through git on $ROOT: a K2 Compute node's checkout
# belongs to the node user, so git there refuses it ("dubious ownership").
# run-checks.sh needs the sha to be a commit in its own checkout.
mkdir -p "$tmp/rc/scripts/ci" "$tmp/rc/scripts/lib"
cp "$ROOT"/scripts/ci/*.sh "$tmp/rc/scripts/ci/"
cp "$ROOT/scripts/lib/rust-pin.sh" "$ROOT/scripts/lib/compute-tier.sh" "$tmp/rc/scripts/lib/"
git -C "$tmp/rc" init -q
# An old commit without the pin, then one pinned to a version unlike the
# repo's own, so the tests see the pin read AT THE SHA, not from the tree.
git -C "$tmp/rc" add scripts
git -C "$tmp/rc" -c user.email=t@t -c user.name=t commit -q -m checks
OLD_SHA="$(git -C "$tmp/rc" rev-parse HEAD)"
printf '[toolchain]\nchannel = "1.98.7"\ncomponents = ["rustfmt"]\n' > "$tmp/rc/rust-toolchain.toml"
git -C "$tmp/rc" add rust-toolchain.toml
git -C "$tmp/rc" -c user.email=t@t -c user.name=t commit -q -m pin
SHA="$(git -C "$tmp/rc" rev-parse HEAD)"
# The working tree says something else: run-checks must not read it.
printf '[toolchain]\nchannel = "9.9.9"\n' > "$tmp/rc/rust-toolchain.toml"
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
K2_CI_API="$API" CI_STATUSES_TOKEN="tok-secret" K2_CI_STATUSES_TOKEN="tok-hand" bash "$ROOT/scripts/ci/post-status.sh" "$SHA" failure k2-compute/rust-test "job x: exit 100" >/dev/null
python3 - "$tmp/reqs" "$SHA" <<'PY' && pass "posts state/context/description with the bearer token" || fail "post" "$(cat "$tmp/reqs")"
import json, sys
r = json.loads(open(sys.argv[1]).read().splitlines()[-1])
assert r["path"] == "/repos/Alakazam-211/K2/statuses/" + sys.argv[2], r
assert r["auth"] == "Bearer tok-secret", r
assert r["body"] == {"state": "failure", "context": "k2-compute/rust-test", "description": "job x: exit 100"}, r
assert "target_url" not in r["body"]
PY
K2_CI_API="$API" K2_CI_STATUSES_TOKEN="tok-hand" K2_CI_STATUS_TOKEN="tok-old" bash "$ROOT/scripts/ci/post-status.sh" "$SHA" success k2-compute/web "ok" >/dev/null
python3 -c 'import json,sys; r=json.loads(open(sys.argv[1]).read().splitlines()[-1]); assert r["auth"]=="Bearer tok-hand", r' "$tmp/reqs" \
    && pass "token order: CI_STATUSES_TOKEN (the job grant), then K2_CI_STATUSES_TOKEN (by hand)" || fail "token order" "$(cat "$tmp/reqs")"
K2_CI_API="$API" K2_CI_STATUS_TOKEN="tok-old" bash "$ROOT/scripts/ci/post-status.sh" "$SHA" success k2-compute/web "ok" >/dev/null
python3 -c 'import json,sys; r=json.loads(open(sys.argv[1]).read().splitlines()[-1]); assert r["auth"]=="Bearer tok-old", r' "$tmp/reqs" \
    && pass "the old by-hand name K2_CI_STATUS_TOKEN still works" || fail "old name" "$(cat "$tmp/reqs")"
grep -q "CI_STATUSES_TOKEN" "$ROOT/scripts/ci/post-status.sh" && ! grep -q -- "--env-name K2_" "$ROOT/scripts/ci/post-status.sh" "$ROOT/docs/ci-on-k2-compute.md" \
    && pass "the documented job grant uses an env name the vault allows (not K2_*)" || fail "env name" "a K2_* --env-name is refused by the vault"

echo "compute-tier.sh (the CLI one release behind):"
mkdir -p "$tmp/tier-old" "$tmp/tier-new"
printf '#!/bin/sh\n[ "$1 $2" = "compute --help" ] && { echo "k2 compute — help"; echo "  [--gate] [--exclusive]"; exit 0; }\n[ "$1" = --version ] && echo "k2 0.46.4"\n' > "$tmp/tier-old/k2"
printf '#!/bin/sh\n[ "$1 $2" = "compute --help" ] && { echo "k2 compute — help"; echo "  [--tier standard|priority|batch|ci]"; exit 0; }\n' > "$tmp/tier-new/k2"
chmod +x "$tmp/tier-old/k2" "$tmp/tier-new/k2"
tier() { K2_BIN="$1" bash -c 'source "$1"; k2_compute_tier_args "$2" | tr "\n" " "' _ "$ROOT/scripts/lib/compute-tier.sh" "$2"; }
[ "$(tier "$tmp/tier-new/k2" ci)" = "--tier ci " ] && [ "$(tier "$tmp/tier-new/k2" standard)" = "--tier standard " ] \
    && pass "a 0.46.5+ CLI: --tier <tier>" || fail "new cli" "$(tier "$tmp/tier-new/k2" ci)"
[ "$(tier "$tmp/tier-old/k2" ci)" = "--gate " ] && [ "$(tier "$tmp/tier-old/k2" standard)" = "" ] \
    && pass "a 0.46.4 CLI: --gate for ci (the same tier), nothing for standard" || fail "old cli" "$(tier "$tmp/tier-old/k2" ci)"
rc=0; K2_BIN="$tmp/tier-old/k2" bash -c 'source "$1"; k2_compute_tier_args priority' _ "$ROOT/scripts/lib/compute-tier.sh" >/dev/null 2>"$tmp/tier-err" || rc=$?
[ "$rc" = 2 ] && grep -q "has no --tier priority" "$tmp/tier-err" && pass "a 0.46.4 CLI asked for priority: refused" || fail "old priority" "rc=$rc $(cat "$tmp/tier-err")"

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
    --help)
        # A CLI with --avoid (0.46.8+) when STUB_AVOID=1; else a plain help.
        echo "k2 compute — help"; echo "  [--tier standard|priority|batch|ci]"
        [ "${STUB_AVOID:-0}" = 1 ] && echo "  [--avoid <node>]..."
        exit 0
        ;;
    nodes)
        # The nodes this workspace is granted ($STUB_NODES, comma list).
        python3 -c 'import json,sys; print(json.dumps({"nodes": [{"name": n} for n in sys.argv[1].split(",") if n]}))' "${STUB_NODES:-k2n7-vin,k2n8-vin}"
        ;;
    get)
        job="$3"; part="$(cat "$STUB_DIR/$job")"
        ex=0; st=done
        [ "${STUB_FAIL:-}" = "$part" ] && ex=100
        [ "${STUB_LOST:-}" = "$part" ] && { st=interrupted; ex=null; }
        [ "${STUB_RUNNING:-}" = "$part" ] && [ ! -e "$STUB_DIR/done-$part" ] && { st=running; ex=null; }
        reason=node_lost
        [ "${STUB_SUPERSEDED:-}" = "$part" ] && { st=cancelled; ex=null; reason=superseded; }
        printf '{"job": "%s", "short": "%s", "node": "%s", "state": "%s", "exit": %s, "durationSecs": 600, "reason": "%s"}\n' \
            "$job" "${job:0:8}" "$(cat "$STUB_DIR/$job.node")" "$st" "$ex" "$reason"
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
    K2_CI_REAP_PAUSE=0 bash "$tmp/rc/scripts/ci/run-checks.sh" "${RC_SHA:-$SHA}" "$@" > "$tmp/o-$name" 2>&1 || rc=$?
    echo "$rc"
}
final() { grep "context\": \"k2-compute/$2\"" "$tmp/o-$1" | tail -1; }

rc="$(run_checks ok)"
[ "$rc" = 0 ] && pass "all parts pass → exit 0" || fail "ok rc=$rc" "$(cat "$tmp/o-ok")"
for p in web rust-check rust-test audit; do
    [[ "$(final ok "$p")" == *'"state": "success"'* ]] && pass "$p: success status" || fail "$p success" "$(final ok "$p")"
done
[ "$(grep -c "k2 compute run --any --os linux --arch x86_64 --sha $SHA --tier ci .*-- bash scripts/ci/checks.sh " "$tmp/c-ok")" = 4 ] \
    && pass "every part: K2 picks a granted Linux x86 node, at the exact sha, --tier ci" || fail "submit lines" "$(cat "$tmp/c-ok")"
grep -q -- "--needs rust@1.98.7,cargo-nextest,.*checks.sh rust-test" "$tmp/c-ok" \
    && grep -q -- "--needs rust@1.98.7,python@3.12,sys:pkg-config .*checks.sh audit" "$tmp/c-ok" \
    && ! grep -q "rust@stable\|rust@9.9.9" "$tmp/c-ok" \
    && pass "rust need = rust-toolchain.toml at the sha (not the tree, not stable)" || fail "pin at sha" "$(cat "$tmp/c-ok")"
grep -q -- "--mem 24G" "$tmp/c-ok" && ! grep -q -- "--supersede" "$tmp/c-ok" \
    && pass "24G per part; no --supersede without --ref" || fail "mem/supersede" "$(cat "$tmp/c-ok")"
[ ! -e "$tmp/state-ok/inflight/$SHA.tsv" ] && pass "a finished run leaves nothing in flight" || fail "inflight left" "$(ls "$tmp/state-ok/inflight")"
[ "$(grep "checks.sh rust-" "$tmp/c-ok" | grep -c -- "--needs [^ ]*cargo-nextest")" = 2 ] \
    && pass "rust parts ask the node for cargo-nextest" || fail "nextest need" "$(cat "$tmp/c-ok")"
[ "$(grep -c '"state": "pending"' "$tmp/o-ok")" -ge 4 ] && pass "pending posted before the results" || fail "pending" "$(cat "$tmp/o-ok")"
python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); assert v["ok"] and len(v["parts"])==4, v' "$tmp/state-ok/runs/$SHA.json" \
    && pass "run record written" || fail "record" "$(cat "$tmp/state-ok/runs/$SHA.json")"
! grep -q 'target_url' "$tmp/o-ok" && pass "no target_url (logs stay in K2)" || fail "target_url" "$(cat "$tmp/o-ok")"
grep -q -- "--needs rust@1.98.7,cargo-nextest,[^ ]*sys:libssl-dev,sys:libasound2-dev .*checks.sh rust-check" "$tmp/c-ok" \
    && pass "rust parts ask for libssl-dev and ALSA (native-voice crates)" || fail "rust sys needs" "$(grep rust-check "$tmp/c-ok")"
! grep -q -- "--avoid" "$tmp/c-ok" && pass "no --avoid when the CLI has none (before 0.46.8)" || fail "avoid on old cli" "$(cat "$tmp/c-ok")"
rc="$(STUB_AVOID=1 STUB_NODES=k2n7-vin,k2-arch,k2n8-vin run_checks avoid)"
[ "$rc" = 0 ] && [ "$(grep -c -- "k2 compute run --any --os linux --arch x86_64 --avoid k2-arch --sha $SHA " "$tmp/c-avoid")" = 4 ] \
    && pass "k2-arch granted: every part K2 places avoids it (an Arch node has no Debian sys: packages)" || fail "avoid k2-arch" "rc=$rc $(cat "$tmp/c-avoid")"
rc="$(STUB_AVOID=1 STUB_NODES=k2n7-vin,k2n8-vin run_checks avoid-none)"
[ "$rc" = 0 ] && ! grep -q -- "--avoid" "$tmp/c-avoid-none" \
    && pass "k2-arch not granted: no --avoid (K2 refuses --avoid on a node it can't see)" || fail "avoid not granted" "rc=$rc $(cat "$tmp/c-avoid-none")"
rc="$(K2_CI_NODE_AUDIT=k2n8-vin STUB_AVOID=1 STUB_NODES=k2-arch,k2n8-vin run_checks avoid-pinned)"
[ "$rc" = 0 ] && grep -q -- "k2 compute run k2n8-vin --sha $SHA .*checks.sh audit" "$tmp/c-avoid-pinned" \
    && ! grep "checks.sh audit" "$tmp/c-avoid-pinned" | grep -q -- "--avoid" \
    && pass "a part pinned to a node gets no --avoid (K2 refuses both together)" || fail "avoid pinned" "rc=$rc $(cat "$tmp/c-avoid-pinned")"

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

export K2_CI_NODE_WEB=k2n8-vin
rc="$(run_checks named --parts web)"
unset K2_CI_NODE_WEB
[ "$rc" = 0 ] && grep -q 'k2 compute run k2n8-vin --sha' "$tmp/c-named" && ! grep -q 'checks.sh rust' "$tmp/c-named" \
    && pass "--parts, and K2_CI_NODE_WEB pins a part to a node" || fail "named" "$(cat "$tmp/c-named")"

RC_SHA="$OLD_SHA" rc="$(RC_SHA="$OLD_SHA" run_checks old --parts rust-check)"
grep -q -- "--needs rust@stable," "$tmp/c-old" && grep -q "built before the pin" "$tmp/o-old" \
    && pass "a commit from before the pin: rust@stable, with a note" || fail "old commit" "$(cat "$tmp/c-old" "$tmp/o-old")"

export STUB_SUPERSEDED=web
rc="$(run_checks sup --parts web,rust-check --ref refs/heads/integ/x)"
unset STUB_SUPERSEDED
grep -q -- "--supersede k2-ci/integ/x/web " "$tmp/c-sup" && grep -q -- "--supersede k2-ci/integ/x/rust-check " "$tmp/c-sup" \
    && pass "--ref: each job supersedes the branch's older queued job for its part" || fail "supersede key" "$(cat "$tmp/c-sup")"
[ "$rc" = 1 ] && [[ "$(final sup web)" == *'"state": "error"'* && "$(final sup web)" == *superseded* ]] \
    && pass "a superseded job → error \"superseded by a newer push\"" || fail "superseded" "rc=$rc $(final sup web)"

echo "run-checks.sh --submit-only / --reap (never waits):"
export STUB_DIR="$tmp/s-split" STUB_CALLS="$tmp/c-split" K2_CI_STATE="$tmp/state-split" STUB_RUNNING=rust-test
mkdir -p "$STUB_DIR"; : > "$STUB_CALLS"
bash "$tmp/rc/scripts/ci/run-checks.sh" "$SHA" --submit-only --parts web,rust-test > "$tmp/o-split" 2>&1
[ -s "$K2_CI_STATE/inflight/$SHA.tsv" ] && ! grep -q "compute wait\|compute get" "$STUB_CALLS" \
    && pass "--submit-only submits and returns (no wait, no get)" || fail "submit-only" "$(cat "$STUB_CALLS")"
rc=0; bash "$tmp/rc/scripts/ci/run-checks.sh" "$SHA" --submit-only --parts web > "$tmp/o-split2" 2>&1 || rc=$?
[ "$rc" = 1 ] && grep -q "already in flight" "$tmp/o-split2" && pass "a second submit of an in-flight sha is refused" || fail "double submit" "rc=$rc $(cat "$tmp/o-split2")"
rc=0; bash "$tmp/rc/scripts/ci/run-checks.sh" --reap "$SHA" > "$tmp/o-reap1" 2>&1 || rc=$?
[ "$rc" = 10 ] && ! grep -q '"state": "success"' "$tmp/o-reap1" && [ -s "$K2_CI_STATE/inflight/$SHA.tsv" ] \
    && pass "--reap while a part runs: exit 10, no final status yet" || fail "reap running" "rc=$rc $(cat "$tmp/o-reap1")"
touch "$STUB_DIR/done-rust-test"
rc=0; bash "$tmp/rc/scripts/ci/run-checks.sh" --reap "$SHA" > "$tmp/o-reap2" 2>&1 || rc=$?
[ "$rc" = 0 ] && [ "$(grep -c '"state": "success"' "$tmp/o-reap2")" = 2 ] && [ ! -e "$K2_CI_STATE/inflight/$SHA.tsv" ] \
    && [ -s "$K2_CI_STATE/runs/$SHA.json" ] && pass "--reap once done: final statuses, record, nothing in flight" || fail "reap done" "rc=$rc $(cat "$tmp/o-reap2")"
rm -f "$STUB_DIR/done-rust-test"
bash "$tmp/rc/scripts/ci/run-checks.sh" "$SHA" --submit-only --parts rust-test --wait-min 0 > /dev/null 2>&1
rc=0; bash "$tmp/rc/scripts/ci/run-checks.sh" --reap "$SHA" > "$tmp/o-reap3" 2>&1 || rc=$?
unset STUB_RUNNING
[ "$rc" = 1 ] && grep -q '"state": "error"' "$tmp/o-reap3" && grep -q "at the deadline" "$tmp/o-reap3" \
    && pass "past the deadline: error \"still running at the deadline\"" || fail "deadline" "rc=$rc $(cat "$tmp/o-reap3")"
rc=0; bash "$tmp/rc/scripts/ci/run-checks.sh" --reap "$SHA" > /dev/null 2>&1 || rc=$?
[ "$rc" = 1 ] && pass "--reap with nothing in flight fails loudly" || fail "reap nothing" "rc=$rc"

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
mkdir -p "$tmp/ci/scripts/ci" "$tmp/ci/scripts/lib"
cp "$ROOT"/scripts/ci/*.sh "$tmp/ci/scripts/ci/"
cp "$ROOT/scripts/lib/rust-pin.sh" "$ROOT/scripts/lib/compute-tier.sh" "$tmp/ci/scripts/lib/"
export STUB_DIR="$tmp/s-poll" STUB_CALLS="$tmp/c-poll" K2_CI_STATE="$tmp/state-poll"
mkdir -p "$STUB_DIR"; : > "$STUB_CALLS"
K2_CI_MAX_INFLIGHT=1 bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p1" 2>&1
grep -q "checking refs/heads/main @ ${MAIN:0:12}" "$tmp/p1" && grep -q "integ/x @ ${INTEG:0:12} waits" "$tmp/p1" \
    && ! grep -q "compute wait" "$STUB_CALLS" && [ -s "$K2_CI_STATE/inflight/$MAIN.tsv" ] \
    && pass "pass 1: submits main's tip and returns; integ/x waits (max in flight 1)" || fail "pass 1" "$(cat "$tmp/p1" "$STUB_CALLS")"
grep -q -- "--supersede k2-ci/main/web " "$STUB_CALLS" && pass "the poller passes the branch (--supersede k2-ci/main/<part>)" || fail "poll ref" "$(cat "$STUB_CALLS")"
K2_CI_MAX_INFLIGHT=1 bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p2" 2>&1
grep -q "${MAIN:0:12} → pass" "$tmp/p2" && grep -q "checking refs/heads/integ/x @ ${INTEG:0:12}" "$tmp/p2" \
    && pass "pass 2: reaps main (statuses), then submits integ/x" || fail "pass 2" "$(cat "$tmp/p2")"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p3" 2>&1
grep -q "${INTEG:0:12} → pass" "$tmp/p3" && ! grep -q checking "$tmp/p3" \
    && pass "pass 3: reaps integ/x; nothing new (feature/* is not watched)" || fail "pass 3" "$(cat "$tmp/p3")"
[ "$(wc -l < "$K2_CI_STATE/seen.tsv" | tr -d ' ')" = 2 ] && [ -s "$K2_CI_STATE/logs/$MAIN.log" ] && [ -z "$(ls "$K2_CI_STATE/inflight")" ] \
    && pass "seen.tsv, per-sha logs, nothing left in flight" || fail "state" "$(cat "$K2_CI_STATE/seen.tsv")"
grep -q '"state": "success"' "$K2_CI_STATE/logs/$INTEG.log" && grep -q '"state": "pending"' "$K2_CI_STATE/logs/$INTEG.log" \
    && pass "the poller's runs posted pending then final statuses (dry run)" || fail "poll statuses" "$(cat "$K2_CI_STATE/logs/$INTEG.log")"
# A live lock holds the pass; a dead one is cleared.
git -C "$tmp/ci" -c user.email=t@t -c user.name=t commit -q --allow-empty -m three
git -C "$tmp/ci" push -q origin HEAD:refs/heads/main
mkdir "$K2_CI_STATE/poll.lock"; echo $$ > "$K2_CI_STATE/poll.lock/pid"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p4" 2>&1
grep -q "another pass holds" "$tmp/p4" && ! grep -q checking "$tmp/p4" && pass "a live lock: no second pass" || fail "live lock" "$(cat "$tmp/p4")"
echo 999999 > "$K2_CI_STATE/poll.lock/pid"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p5" 2>&1
grep -q "checking refs/heads/main" "$tmp/p5" && [ ! -d "$K2_CI_STATE/poll.lock" ] && pass "a dead poller's lock is cleared" || fail "dead lock" "$(cat "$tmp/p5")"
# --mark-seen: the next tips are recorded, not checked.
git -C "$tmp/ci" -c user.email=t@t -c user.name=t commit -q --allow-empty -m four
git -C "$tmp/ci" push -q origin HEAD:refs/heads/integ/z
: > "$STUB_CALLS"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --mark-seen > "$tmp/p6" 2>&1
grep -q "marked 1 tip" "$tmp/p6" && ! grep -q "compute run" "$STUB_CALLS" && grep -q "refs/heads/integ/z" "$K2_CI_STATE/seen.tsv" \
    && pass "--mark-seen records today's tips without a run" || fail "mark-seen" "$(cat "$tmp/p6" "$STUB_CALLS")"
bash "$tmp/ci/scripts/ci/k2-ci-poll.sh" --once > "$tmp/p7" 2>&1
! grep -q "checking refs/heads/integ/z" "$tmp/p7" && pass "a marked tip is not checked" || fail "marked checked" "$(cat "$tmp/p7")"

echo "gates read files, not git (node checkouts: dubious ownership):"
# A `git` that refuses every repo, the way git refuses a node checkout the
# job user doesn't own. The web part's gates must not need it.
mkdir -p "$tmp/nogit"
printf '#!/bin/sh\necho "fatal: detected dubious ownership in repository" >&2\nexit 128\n' > "$tmp/nogit/git"
chmod +x "$tmp/nogit/git"
for g in k2-home-gate.sh private-docs-gate.sh; do
    rc=0; out="$(cd / && PATH="$tmp/nogit:$PATH" bash "$ROOT/scripts/$g" 2>&1)" || rc=$?
    [ "$rc" = 0 ] && [[ "$out" == *": clean"* || "$out" == *": OK"* ]] && pass "$g passes this checkout with git refusing it" || fail "$g without git" "rc=$rc $out"
done
# ...and still catches what it's for, in a tree of the test's own.
mkdir -p "$tmp/gt/scripts" "$tmp/gt/crates/x" "$tmp/gt/src-tauri" "$tmp/gt/src" "$tmp/gt/cli" "$tmp/gt/.k2/prds" "$tmp/gt/.k2/notes"
cp "$ROOT/scripts/k2-home-gate.sh" "$ROOT/scripts/private-docs-gate.sh" "$tmp/gt/scripts/"
: > "$tmp/gt/scripts/k2-home-allowlist.txt"
printf 'let p = "~/.k2so/x";\n// a comment: ~/.k2so is fine\n' > "$tmp/gt/crates/x/lib.rs"
mkdir -p "$tmp/gt/crates/x/target"; printf 'let p = "~/.k2so/built";\n' > "$tmp/gt/crates/x/target/out.rs"
rc=0; out="$(PATH="$tmp/nogit:$PATH" bash "$tmp/gt/scripts/k2-home-gate.sh" 2>&1)" || rc=$?
[ "$rc" = 1 ] && [[ "$out" == *"VIOLATION: crates/x/lib.rs:1:"* ]] && [[ "$out" != *"lib.rs:2:"* && "$out" != *target* ]] \
    && pass "k2-home-gate flags a ~/.k2so literal (not comments, not build output)" || fail "k2-home-gate catch" "rc=$rc $out"
echo 'crates/x/lib.rs :: ~/.k2so/x' > "$tmp/gt/scripts/k2-home-allowlist.txt"
rc=0; out="$(PATH="$tmp/nogit:$PATH" bash "$tmp/gt/scripts/k2-home-gate.sh" 2>&1)" || rc=$?
[ "$rc" = 0 ] && pass "k2-home-gate honours its allowlist" || fail "k2-home-gate allowlist" "rc=$rc $out"
: > "$tmp/gt/.k2/prds/prd-agent-voice-v1.md"
rc=0; out="$(PATH="$tmp/nogit:$PATH" bash "$tmp/gt/scripts/private-docs-gate.sh" 2>&1)" || rc=$?
[ "$rc" = 0 ] && pass "private-docs-gate: a product PRD is fine" || fail "private-docs ok" "rc=$rc $out"
: > "$tmp/gt/.k2/notes/fleet-supply.md"
rc=0; out="$(PATH="$tmp/nogit:$PATH" bash "$tmp/gt/scripts/private-docs-gate.sh" 2>&1)" || rc=$?
[ "$rc" = 1 ] && [[ "$out" == *".k2/notes/fleet-supply.md"* ]] && pass "private-docs-gate flags a cloud-business doc" || fail "private-docs catch" "rc=$rc $out"

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
