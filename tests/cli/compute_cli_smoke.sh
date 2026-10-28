#!/usr/bin/env bash
# k2 compute — CLI smoke: a sandbox daemon (temp HOME, K2_COMPUTE=1,
# air-gapped) + the REAL k2-node binary (--dev, as this user, temp home and
# config dir) + this checkout's cli/k2, all on loopback.
#
# Covers: node add → k2-node enroll → confirm with the SAS the node printed;
# grant; an attached run returns the job's exit code and streams output;
# a refusal exits 3; --detach + jobs + logs + receipt verified; logs
# --failures; the machine owner's local pause (control file) holds new
# jobs and resume releases them; node remove revokes the node.
#
# Build first:  cargo build -p k2-daemon -p k2-node
# Never touches the real ~/.k2, the production daemon or the keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
NODE="$TARGET/debug/k2-node"
for b in "$DAEMON" "$NODE"; do
    [ -x "$b" ] || { echo "FAIL: $b not built (cargo build -p k2-daemon -p k2-node)" >&2; exit 1; }
done

# Session variables reach the production daemon: drop them all.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
# A here-string, not printf | grep -q: under pipefail, grep -q quitting at
# the first match can SIGPIPE printf on a long page (k2 study compute) and
# fail a check whose text is there.
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2cs-XXXXXX)"
DAEMON_PID=""
NODE_PID=""
cleanup() {
    [ -n "$NODE_PID" ] && kill "$NODE_PID" 2>/dev/null || true
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "$NODE_PID" ] && kill -9 "$NODE_PID" 2>/dev/null || true
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -30 "$SANDBOX/daemon.log" >&2 || true
        echo "--- node log tail ---" >&2; tail -30 "$SANDBOX/node.log" >&2 || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

DHOME="$SANDBOX/home"
NODE_HOME="$SANDBOX/node/home"
NODE_CFG="$SANDBOX/node/etc"
mkdir -p "$DHOME" "$NODE_HOME" "$NODE_CFG"
cat > "$NODE_CFG/policy.toml" <<'EOF'
availability = "always"
foreign_locks = []
write_smoke_lock = ""
disk_floor_gb = 0
max_parallel = 2
EOF

echo "== sandbox daemon =="
# Own TMPDIR / XDG_RUNTIME_DIR: the daemon's private runtime dir (rendered
# frpc.toml) must never be the real one (2026-10-14 incident).
mkdir -p "$DHOME/tmp" "$DHOME/run"
TMPDIR="$DHOME/tmp" XDG_RUNTIME_DIR="$DHOME/run" K2_RUNTIME_DIR= \
    HOME="$DHOME" K2_COMPUTE=1 K2_AIRGAP=1 K2_NODE_HOME="$NODE_HOME" K2_NODE_CONFIG_DIR="$NODE_CFG" \
    "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do
    [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon wrote its port"; exit 1; }
PORT="$(cat "$DHOME/.k2/heartbeat.port")"
OWNER_TOKEN="$(cat "$DHOME/.k2/heartbeat.token")"
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"
# The CLI finds its daemon under $HOME/.k2: only ever the sandbox's.
k2() { HOME="$DHOME" "$K2" "$@"; }

echo "== workspace =="
WS="$SANDBOX/ws"
mkdir -p "$WS"
g() { git -C "$WS" -c user.name=t -c user.email=t@t -c commit.gpgsign=false -c init.defaultBranch=main "$@"; }
g init -q
echo "hello from the workspace" > "$WS/a.txt"
g add a.txt
g commit -q -m one
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")" \
    "http://127.0.0.1:$PORT/cli/projects/add-from-path?token=$OWNER_TOKEN")"
expect_has "workspace registered" "$reg" "\"path\":\"$WS\""
cd "$WS"
export K2_PROJECT_PATH="$WS"

echo "== preview switch =="
expect_has "preview on" "$(k2 compute preview on)" "Compute nodes (preview): on"
expect_has "preview status reads it back" "$(k2 compute preview status)" ": on"

echo "== enroll =="
out="$(k2 compute nodes)"
expect_has "no nodes yet" "$out" "(no compute nodes"
# --json (2026-10-28): {code, expiresAt, name, controller}; the code goes to
# k2-node on stdin from a 0600 file, never in its argv.
out="$(k2 compute node add mini --url "http://127.0.0.1:$PORT" --json)"
expect_eq "node add --json keys" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(",".join(sorted(json.load(sys.stdin))))')" "code,controller,expiresAt,name"
expect_eq "node add --json name" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["name"])')" "mini"
ENROLL_FILE="$SANDBOX/enroll-code"
(umask 077; printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["code"])' > "$ENROLL_FILE")
[ -s "$ENROLL_FILE" ] || { bad "enroll code from --json"; exit 1; }
enrolled="$("$NODE" enroll --controller "http://127.0.0.1:$PORT" --enroll - --name mini --home "$NODE_HOME" --config-dir "$NODE_CFG" < "$ENROLL_FILE")"
expect_has "node enrolled" "$enrolled" "Enrolled as mini (pending)"
SAS="$(printf '%s\n' "$enrolled" | sed -n 's/.*Code: \([0-9]\{3\} [0-9]\{3\}\).*/\1/p')"
out="$(k2 compute nodes)"
expect_has "controller shows the same code" "$out" "pending (code $SAS)"
set +e
k2 compute node confirm mini 000001 >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
[ "$(echo "$SAS" | tr -d ' ')" = "000001" ] || { expect_eq "wrong code exits 3" "$rc" "3"; expect_has "sas_mismatch" "$(cat "$SANDBOX/err")" "sas_mismatch"; }
out="$(k2 compute node confirm mini "$SAS")"
expect_has "confirmed" "$out" "mini is active"

"$NODE" run --dev --home "$NODE_HOME" --config-dir "$NODE_CFG" > "$SANDBOX/node.log" 2>&1 &
NODE_PID=$!
for _ in $(seq 1 100); do
    k2 compute nodes | grep -q "online/loopback" && break
    sleep 0.2
done
expect_has "node online" "$(k2 compute nodes)" "online/loopback"

echo "== grant + run =="
set +e
k2 compute run mini --workspace "$WS" -- true >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
# The owner token may run without a grant; agents can't (headless test).
expect_eq "owner run works before a grant" "$rc" "0"
out="$(k2 compute grant mini --workspace "$WS" --max-parallel 2)"
expect_has "granted" "$out" "granted mini"
out="$(k2 compute nodes)"
expect_has "nodes shows unreserved CPU" "$out" "unreserved"
expect_has "nodes shows this workspace's grant in human units" "$out" "grant 2h/job 2 parallel 10 queued"
expect_has "info shows the grant in human units" "$(k2 compute info mini)" "2h/job  2 parallel"
set +e
k2 compute run mini --timeout 3h -- true >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "a --timeout over the grant is a bad request" "$rc" "2"
expect_has "timeout_over_grant names the grant and the owner command" "$(cat "$SANDBOX/err")" "(grant: 2h/job, 2 parallel, 10 queued, 60 GB disk). Ask your owner: k2 compute grant mini --workspace"
help="$(k2 compute --help)"
for want in "k2 compute wait <job>..." "[--workspace WS]" "cancel <job> [--force]" "≤ the grant's max" "--wait [--failures] [--max-wait 9m]" "K2_COMPUTE_JOB"; do
    expect_has "--help lists $want" "$help" "$want"
done
study="$(k2 study compute)"
for want in "K2_COMPUTE_JOB=<job id>" "CANCEL" "WORKTREES" "k2 compute wait <id>" "sh -ec"; do
    expect_has "study compute covers $want" "$study" "$want"
done
set +e
stdout="$(k2 compute run mini -- sh -c 'cat a.txt; echo to-stderr >&2; exit 7' 2>"$SANDBOX/err")"; rc=$?
set -e
expect_eq "attached run exits with the job's code" "$rc" "7"
expect_eq "stdout is the job's stdout" "$stdout" "hello from the workspace"
expect_has "stderr carries the job's stderr" "$(cat "$SANDBOX/err")" "to-stderr"
expect_has "stderr has the exit line" "$(cat "$SANDBOX/err")" "[k2 compute] exit 7"
# The exit line names THIS job (not a shifted timestamp) and how long it took.
SHORT="$(sed -n 's/^\[k2 compute\] job \([0-9a-f]\{8\}\) on mini.*/\1/p' "$SANDBOX/err" | head -1)"
expect_has "exit line says how long it took" "$(cat "$SANDBOX/err")" "after 0m"
expect_has "exit line points at this job's logs" "$(cat "$SANDBOX/err")" "k2 compute logs $SHORT --failures"
[ -n "$SHORT" ] && ok "job line parsed" || bad "job line parsed"
set +e
k2 compute run nosuch -- true >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "unknown node exits 3" "$rc" "3"
expect_has "node_not_found" "$(cat "$SANDBOX/err")" "node_not_found"
set +e
k2 compute run mini --env K2_HOOK_TOKEN=x -- true >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "a daemon-reaching --env is a bad request" "$rc" "2"
set +e
k2 compute run mini --needs 'node@22;reboot' -- true >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "a hostile --needs is a bad request" "$rc" "2"
expect_has "need_invalid" "$(cat "$SANDBOX/err")" "need_invalid"
set +e
k2 compute run mini --needs sys:k2-smoke-no-such-pkg -- true >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "a missing system package fails before the job runs" "$rc" "3"
expect_has "and says what the owner should do" "$(cat "$SANDBOX/err")" "package_"
st="$(k2 compute local status --json)"
expect_has "local status reports the home check" "$st" '"homeCheck"'
expect_has "local status reports where k2-node comes from" "$st" '"binary"'

echo "== run from a git worktree: K2_* unset, no --workspace (A1 + A3) =="
# Worktrees carry tracked .k2/ files, so the CLI's walk-up stops at the
# worktree root and sends ITS path as the workspace: the daemon must map
# it back to the workspace (it used to answer workspace_not_found).
mkdir -p "$WS/.k2/notes"
echo note > "$WS/.k2/notes/n.md"
g add -f .k2/notes/n.md
g commit -q -m notes
WT="$WS/.claude/worktrees/x"
g worktree add -q -b wt "$WT"
echo "from the worktree" > "$WT/wt.txt"
git -C "$WT" -c user.name=t -c user.email=t@t -c commit.gpgsign=false add wt.txt
git -C "$WT" -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q -m wt
WT_HEAD="$(git -C "$WT" rev-parse HEAD)"
[ -d "$WT/.k2" ] && ok "the worktree has its own .k2 (walk-up stops there)" || bad "the worktree has its own .k2"
echo "dirty main checkout" > "$WS/main-untracked.txt"
wt_k2() { (cd "$WT" && unset K2_PROJECT_PATH && HOME="$DHOME" "$K2" "$@"); }
jfield() { python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get(sys.argv[1]) if v.get(sys.argv[1]) is not None else "")' "$1"; }
# The daemon's full job row (what `get --json` printed before compute B made
# it the result object): srcCommit, detach, … straight from the route.
job_row() { curl -sf "http://127.0.0.1:$PORT/cli/compute/jobs/get?job=$1&token=$OWNER_TOKEN"; }
set +e
wt_k2 compute run mini --no-source -- true >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "run --no-source from the worktree works (no workspace_not_found)" "$rc" "0"
set +e
stdout="$(wt_k2 compute run mini -- cat wt.txt 2>"$SANDBOX/err")"; rc=$?
set -e
expect_eq "attached run from the worktree exits 0" "$rc" "0"
expect_eq "the job ran the worktree's HEAD (its file is there)" "$stdout" "from the worktree"
expect_has "the source line names the worktree" "$(cat "$SANDBOX/err")" "source: worktree .claude/worktrees/x @ ${WT_HEAD:0:8}"
case "$(cat "$SANDBOX/err")" in *"uncommitted changes"*) bad "no false uncommitted-changes note from a clean worktree" ;; *) ok "no false uncommitted-changes note from a clean worktree" ;; esac
JOBW="$(wt_k2 compute run mini --detach -- true 2>/dev/null)"
for _ in $(seq 1 100); do
    wt_k2 compute get "$JOBW" | grep -q ": done" && break
    sleep 0.2
done
# `get --json` prints the one result object (compute B); its `commit` is the
# job row's srcCommit. The full row (srcCommit itself) is the daemon's body.
expect_eq "result.commit is the worktree HEAD" "$(wt_k2 compute get "$JOBW" --json | jfield commit)" "$WT_HEAD"
expect_eq "job.srcCommit is the worktree HEAD" "$(job_row "$JOBW" | jfield srcCommit)" "$WT_HEAD"
echo 'echo dirty-ok' > "$WT/untracked.sh"
set +e
stdout="$(wt_k2 compute run mini --dirty -- sh untracked.sh 2>"$SANDBOX/err")"; rc=$?
set -e
expect_eq "--dirty from the worktree sends its untracked script" "$stdout" "dirty-ok"
expect_has "the source line counts the worktree's changes" "$(cat "$SANDBOX/err")" "+dirty (0 changed, 1 untracked)"
set +e
wt_k2 compute run mini --dirty --sha HEAD~1 -- true >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "--dirty --sha <not HEAD> is a bad request" "$rc" "2"
expect_has "dirty_needs_head" "$(cat "$SANDBOX/err")" "dirty_needs_head"
rm -f "$WT/untracked.sh" "$WS/main-untracked.txt"

echo "== detach, jobs, logs, receipt =="
JOB="$(k2 compute run mini --detach -- sh -c 'echo test result: FAILED. 1 passed; 2 failed; echo detached-done')"
for _ in $(seq 1 100); do
    k2 compute get "$JOB" | grep -q ": done" && break
    sleep 0.2
done
expect_has "detached job done" "$(k2 compute get "$JOB")" ": done"
expect_has "jobs lists it" "$(k2 compute jobs)" "$JOB"
expect_has "logs" "$(k2 compute logs "$JOB")" "detached-done"
fails="$(k2 compute logs "$JOB" --failures)"
expect_has "logs --failures keeps the failure line" "$fails" "test result: FAILED"
case "$fails" in *detached-done*) bad "logs --failures drops other lines" ;; *) ok "logs --failures drops other lines" ;; esac
for _ in $(seq 1 50); do
    k2 compute receipt "$JOB" >/dev/null 2>&1 && break
    sleep 0.2
done
expect_has "receipt verified" "$(k2 compute receipt "$JOB")" "verified: yes"

echo "== the machine owner's pause =="
out="$(k2 compute local pause)"
expect_has "local pause" "$out" "paused"
# Wait until the controller has heard the pause (the node polls its control
# file), not a fixed second: a fixed sleep flaked once.
for _ in $(seq 1 100); do
    k2 compute nodes | grep -q "paused" && break
    sleep 0.1
done
expect_has "controller sees the pause" "$(k2 compute nodes)" "paused"
JOB2="$(k2 compute run mini --detach -- echo after-resume 2>/dev/null)"
sleep 1.5
expect_has "paused node holds the job" "$(k2 compute get "$JOB2")" "local_paused"
st="$(k2 compute status)"
expect_has "status shows the paused node" "$st" "mini"
expect_has "status lists the queued job" "$st" "queued  $JOB2"
expect_has "status has the controller row" "$st" "controller "
expect_has "status says what needs node reporting" "$st" "not yet: hand-run lock files"
expect_has "status --json carries the controller machine" "$(k2 compute status --json)" '"controllerMachine"'
expect_has "status <job> is still get" "$(k2 compute status "$JOB2")" "job $JOB2 on mini"
JOB3="$(k2 compute run mini --detach -- echo never-runs 2>/dev/null)"
expect_eq "cancel of a queued job needs no --force" "$(k2 compute cancel "$JOB3")" "cancelled $JOB3 (was queued on mini)"
expect_has "get names who cancelled" "$(k2 compute get "$JOB3")" "cancel asked by owner"
k2 compute local resume >/dev/null
for _ in $(seq 1 100); do
    k2 compute get "$JOB2" | grep -q ": done" && break
    sleep 0.2
done
expect_has "resume releases it" "$(k2 compute get "$JOB2")" ": done"
expect_has "the cancelled job stayed cancelled" "$(k2 compute get "$JOB3")" ": cancelled"

echo "== cancel a started job (A4: --force) =="
JOBC="$(k2 compute run mini --detach -- sleep 30 2>/dev/null)"
for _ in $(seq 1 100); do
    k2 compute get "$JOBC" | grep -q ": running" && break
    sleep 0.2
done
set +e
k2 compute cancel "$JOBC" >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "cancel of a started job without --force exits 3" "$rc" "3"
expect_has "job_started" "$(cat "$SANDBOX/err")" "job_started"
expect_has "the refusal teaches --force" "$(cat "$SANDBOX/err")" "k2 compute cancel $JOBC --force"
expect_has "it kept running" "$(k2 compute get "$JOBC")" ": running"
expect_has "status lists the running job" "$(k2 compute status)" "running $JOBC"
set +e
k2 compute cancel "$JOBC" --bogus >/dev/null 2>&1; rc=$?
set -e
expect_eq "an unknown cancel flag is a bad request" "$rc" "2"
expect_has "--force after the id stops it" "$(k2 compute cancel "$JOBC" --force)" "stopping $JOBC (was running on mini for"
for _ in $(seq 1 100); do
    k2 compute get "$JOBC" | grep -q ": cancelled" && break
    sleep 0.2
done
expect_has "the job ended cancelled" "$(k2 compute get "$JOBC")" ": cancelled"

echo "== wait and run --wait (A5) =="
JW0="$(k2 compute run mini --detach -- true 2>/dev/null)"
set +e
k2 compute wait "$JW0" >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "wait on a passing job exits 0" "$rc" "0"
expect_has "wait prints the end line" "$(cat "$SANDBOX/err")" "[k2 compute] job $JW0 on mini: exit 0"
JW1="$(k2 compute run mini --detach -- sh -c 'echo "test result: FAILED. 1 passed; 2 failed"; exit 101' 2>/dev/null)"
set +e
fout="$(k2 compute wait "$JW1" --failures 2>"$SANDBOX/err")"; rc=$?
set -e
expect_eq "wait passes the job's exit 101 through" "$rc" "101"
expect_has "--failures prints the failure lines" "$fout" "test result: FAILED"
expect_eq "cancelled job waits to 3" "$(set +e; k2 compute wait "$JOBC" >/dev/null 2>"$SANDBOX/err"; echo $?)" "3"
expect_has "cancelled job error JSON" "$(cat "$SANDBOX/err")" '{"error": "cancelled"'
set +e
k2 compute wait "$JW0" "$JW1" "$JOBC" >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "several jobs: the first failing job's exit" "$rc" "101"
JWS="$(k2 compute run mini --detach -- sleep 20 2>/dev/null)"
set +e
k2 compute wait "$JWS" --max-wait 2s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "--max-wait expiry exits 3" "$rc" "3"
expect_has "and says still_running with the next wait line" "$(cat "$SANDBOX/err")" "\"hint\": \"k2 compute wait $JWS\""
k2 compute cancel "$JWS" --force >/dev/null
set +e
k2 compute wait nosuchjob >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "wait on a missing job exits 4" "$rc" "4"
set +e
rout="$(k2 compute run mini --wait -- sh -c 'echo never-streamed; exit 5' 2>"$SANDBOX/err")"; rc=$?
set -e
expect_eq "run --wait exits with the job's code" "$rc" "5"
RW="$(printf '%s\n' "$rout" | head -1)"
expect_eq "run --wait prints only the job id on stdout" "$rout" "$RW"
expect_has "the id is the job's" "$(k2 compute get "$RW")" "job $RW on mini: done"
case "$rout$(cat "$SANDBOX/err")" in *never-streamed*) bad "run --wait streams no log text" ;; *) ok "run --wait streams no log text" ;; esac
expect_has "run --wait posted detach:false" "$(job_row "$RW")" '"detach":false'
set +e
k2 compute run mini --wait --detach -- true >/dev/null 2>&1; rc=$?
set -e
expect_eq "--wait with --detach is a bad request" "$rc" "2"

echo "== remove =="
out="$(k2 compute node remove mini)"
expect_has "removed" "$out" "removed mini"
for _ in $(seq 1 50); do
    grep -q '"revoked"' "$NODE_HOME/run/status.json" 2>/dev/null && break
    sleep 0.2
done
expect_has "the node knows it was removed" "$(cat "$NODE_HOME/run/status.json")" "revoked"

echo
echo "compute_cli_smoke: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
