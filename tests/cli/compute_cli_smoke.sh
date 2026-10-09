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
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

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
out="$(k2 compute node add mini --url "http://127.0.0.1:$PORT")"
expect_has "enroll code printed" "$out" "Enroll code for mini"
ENROLL="$(printf '%s\n' "$out" | sed -n '2p' | tr -d ' ')"
[ -n "$ENROLL" ] || { bad "enroll string parsed"; exit 1; }
enrolled="$("$NODE" enroll --controller "http://127.0.0.1:$PORT" --enroll "$ENROLL" --name mini --home "$NODE_HOME" --config-dir "$NODE_CFG")"
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
k2 compute local resume >/dev/null
for _ in $(seq 1 100); do
    k2 compute get "$JOB2" | grep -q ": done" && break
    sleep 0.2
done
expect_has "resume releases it" "$(k2 compute get "$JOB2")" ": done"

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
