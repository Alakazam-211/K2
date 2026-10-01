#!/usr/bin/env bash
# Tests for scripts/arch-build-gate.sh with stub gh and ssh on PATH.
# git is real: each case pushes to a local bare repo. ssh runs the remote
# command locally, so the k2-arch lock is a real file in a temp dir.
# Nothing here talks to GitHub or k2-arch. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GATE="$ROOT/scripts/arch-build-gate.sh"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# ── stubs ──
mkdir -p "$tmp/bin"
cat > "$tmp/bin/ssh" <<'STUB'
#!/usr/bin/env bash
# ssh [-o opt]... host command → run command locally.
echo "ssh $*" >> "$STUB_CALLS"
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) shift 2 ;;
        -*) shift ;;
        *) break ;;
    esac
done
shift # host
if [ "${STUB_SSH_FAIL:-0}" = 1 ]; then
    echo "ssh: connect to host: Connection timed out" >&2
    exit 255
fi
exec bash -c "$*"
STUB
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
echo "gh $*" >> "$STUB_CALLS"
case "$1 $2" in
    "workflow run")
        if [ -e "$K2_ARCH_GATE_LOCK" ]; then echo "LOCK_PRESENT_AT_DISPATCH" >> "$STUB_CALLS"; fi
        ;;
    "run list")
        echo 4242
        ;;
    "run view")
        # One state per call; the last line repeats.
        n="$(cat "$STUB_STATE_N" 2>/dev/null || echo 0)"
        n=$((n + 1))
        echo "$n" > "$STUB_STATE_N"
        total="$(wc -l < "$STUB_STATES" | tr -d ' ')"
        [ "$n" -le "$total" ] || n="$total"
        sed -n "${n}p" "$STUB_STATES"
        ;;
    "run cancel")
        ;;
    *)
        echo "gh stub: unexpected $*" >&2
        exit 1
        ;;
esac
STUB
chmod +x "$tmp/bin/ssh" "$tmp/bin/gh"

# case_setup <name> → sets CASE (dir), WORK (clone), SHA; exports stub env.
case_setup() {
    CASE="$tmp/$1"
    mkdir -p "$CASE"
    git init --quiet --bare "$CASE/remote.git"
    git init --quiet "$CASE/work"
    git -C "$CASE/work" -c user.name=t -c user.email=t@example.invalid \
        commit --quiet --allow-empty -m "release preview"
    git -C "$CASE/work" remote add origin "$CASE/remote.git"
    WORK="$CASE/work"
    SHA="$(git -C "$WORK" rev-parse HEAD)"
    export STUB_CALLS="$CASE/calls"
    export STUB_STATES="$CASE/states"
    export STUB_STATE_N="$CASE/state-n"
    export K2_ARCH_GATE_LOCK="$CASE/k2-release.lock"
    : > "$STUB_CALLS"
    rm -f "$STUB_STATE_N"
}

run_gate() {
    local rc=0
    (
        cd "$WORK"
        PATH="$tmp/bin:$PATH" \
        K2_ARCH_GATE_REPO=test/repo \
        K2_ARCH_GATE_POLL_SEC=0 \
        K2_ARCH_GATE_FIND_SEC=5 \
        "$GATE" "$@"
    ) >"$CASE/out" 2>"$CASE/err" || rc=$?
    echo "$rc"
}

gate_branches() {
    git -C "$CASE/remote.git" for-each-ref --format='%(refname)' refs/heads/arch-gate/
}

called() { grep -Fq -- "$1" "$STUB_CALLS"; }

echo "== arch-build-gate.sh =="

# ── success ──
case_setup ok
printf 'queued \nin_progress \ncompleted success\n' > "$STUB_STATES"
rc="$(run_gate HEAD v9.9.9)"
if [ "$rc" = 0 ]; then pass "success exits 0"; else fail "success exits 0" "rc=$rc err=$(cat "$CASE/err")"; fi
if called "gh workflow run arch-package.yml --repo test/repo --ref arch-gate/v9.9.9-"; then
    pass "dispatches arch-package.yml on the throwaway branch"
else
    fail "dispatches arch-package.yml on the throwaway branch" "$(cat "$STUB_CALLS")"
fi
if called "-f ref=${SHA} -f upload=false -f gate_id=v9.9.9-"; then
    pass "dispatch passes ref=<commit> upload=false gate_id"
else
    fail "dispatch passes ref=<commit> upload=false gate_id" "$(cat "$STUB_CALLS")"
fi
if called "LOCK_PRESENT_AT_DISPATCH"; then pass "lock is held at dispatch"; else fail "lock is held at dispatch" "$(cat "$STUB_CALLS")"; fi
if called "ssh -o BatchMode=yes -o ConnectTimeout=20 k2-ci@40.160.54.134"; then
    pass "lock goes over ssh to k2-ci@40.160.54.134"
else
    fail "lock goes over ssh to k2-ci@40.160.54.134" "$(cat "$STUB_CALLS")"
fi
if [ ! -e "$K2_ARCH_GATE_LOCK" ]; then pass "success removes the lock"; else fail "success removes the lock" "$(cat "$K2_ARCH_GATE_LOCK")"; fi
if [ -z "$(gate_branches)" ]; then pass "success deletes the throwaway branch"; else fail "success deletes the throwaway branch" "$(gate_branches)"; fi
if called "gh run cancel"; then fail "success does not cancel" "$(cat "$STUB_CALLS")"; else pass "success does not cancel"; fi

# ── failed build ──
case_setup failed
printf 'in_progress \ncompleted failure\n' > "$STUB_STATES"
rc="$(run_gate HEAD v9.9.9)"
if [ "$rc" != 0 ] && grep -Fq "Arch build gate FAILED: run 4242 concluded 'failure'" "$CASE/err"; then
    pass "failed run fails the gate and says so"
else
    fail "failed run fails the gate and says so" "rc=$rc err=$(cat "$CASE/err")"
fi
if [ ! -e "$K2_ARCH_GATE_LOCK" ]; then pass "failure removes the lock"; else fail "failure removes the lock" "still there"; fi
if [ -z "$(gate_branches)" ]; then pass "failure deletes the throwaway branch"; else fail "failure deletes the throwaway branch" "$(gate_branches)"; fi

# ── timeout ──
case_setup timeout
printf 'in_progress \n' > "$STUB_STATES"
rc="$(K2_ARCH_GATE_TIMEOUT_SEC=1 run_gate HEAD v9.9.9)"
if [ "$rc" != 0 ] && grep -Fq "TIMED OUT" "$CASE/err"; then
    pass "timeout fails the gate"
else
    fail "timeout fails the gate" "rc=$rc err=$(cat "$CASE/err")"
fi
if called "gh run cancel 4242 --repo test/repo"; then pass "timeout cancels the run"; else fail "timeout cancels the run" "$(cat "$STUB_CALLS")"; fi
if [ ! -e "$K2_ARCH_GATE_LOCK" ]; then pass "timeout removes the lock"; else fail "timeout removes the lock" "still there"; fi

# ── SIGTERM (release.sh exit trap) ──
case_setup term
printf 'in_progress \n' > "$STUB_STATES"
(
    cd "$WORK"
    PATH="$tmp/bin:$PATH" K2_ARCH_GATE_REPO=test/repo K2_ARCH_GATE_POLL_SEC=1 \
        K2_ARCH_GATE_FIND_SEC=5 exec "$GATE" HEAD v9.9.9
) >"$CASE/out" 2>"$CASE/err" &
gpid=$!
for _ in $(seq 1 50); do
    called "gh run view" && break
    sleep 0.2
done
kill -TERM "$gpid"
rc=0
wait "$gpid" || rc=$?
if [ "$rc" = 143 ]; then pass "TERM exits 143"; else fail "TERM exits 143" "rc=$rc err=$(cat "$CASE/err")"; fi
if called "gh run cancel 4242"; then pass "TERM cancels the run"; else fail "TERM cancels the run" "$(cat "$STUB_CALLS")"; fi
if [ ! -e "$K2_ARCH_GATE_LOCK" ]; then pass "TERM removes the lock"; else fail "TERM removes the lock" "still there"; fi
if [ -z "$(gate_branches)" ]; then pass "TERM deletes the throwaway branch"; else fail "TERM deletes the throwaway branch" "$(gate_branches)"; fi

# ── lock already there (operator wrote it) ──
case_setup prelock
printf 'operator HOLD\n' > "$K2_ARCH_GATE_LOCK"
printf 'completed success\n' > "$STUB_STATES"
rc="$(run_gate HEAD v9.9.9)"
if [ "$rc" = 0 ] && [ "$(cat "$K2_ARCH_GATE_LOCK")" = "operator HOLD" ]; then
    pass "existing lock is left in place"
else
    fail "existing lock is left in place" "rc=$rc lock=$(cat "$K2_ARCH_GATE_LOCK" 2>/dev/null || echo gone)"
fi

# ── ssh down: no dispatch ──
case_setup sshdown
printf 'completed success\n' > "$STUB_STATES"
rc="$(STUB_SSH_FAIL=1 run_gate HEAD v9.9.9)"
if [ "$rc" != 0 ] && grep -Fq "could not write" "$CASE/err"; then
    pass "ssh failure fails the gate"
else
    fail "ssh failure fails the gate" "rc=$rc err=$(cat "$CASE/err")"
fi
if called "gh workflow run"; then fail "ssh failure does not dispatch" "$(cat "$STUB_CALLS")"; else pass "ssh failure does not dispatch"; fi
if [ -z "$(gate_branches)" ]; then pass "ssh failure pushes no branch"; else fail "ssh failure pushes no branch" "$(gate_branches)"; fi

# ── skip ──
case_setup skip
printf 'completed success\n' > "$STUB_STATES"
rc="$(K2_SKIP_ARCH_GATE=1 run_gate HEAD v9.9.9)"
if [ "$rc" = 0 ] && grep -Fq "ARCH BUILD GATE SKIPPED" "$CASE/out" && [ ! -s "$STUB_CALLS" ]; then
    pass "K2_SKIP_ARCH_GATE=1 warns and touches nothing"
else
    fail "K2_SKIP_ARCH_GATE=1 warns and touches nothing" "rc=$rc calls=$(cat "$STUB_CALLS")"
fi

echo
if [ "$fail" -ne 0 ]; then
    echo "FAIL: $fail  PASS: $pass" >&2
    exit 1
fi
echo "PASS: $pass"
