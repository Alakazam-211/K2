#!/usr/bin/env bash
# Tests for scripts/release-lock.sh (release.sh's /var/tmp/k2-release.lock
# and the wait for a running compute-node job). The lock path, node binary
# and node home point at temp files and stubs. No sudo, no real node.
# Runs on the Mac and on ubuntu-latest. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LIB="$ROOT/scripts/release-lock.sh"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
sleeper=""
trap '[ -n "$sleeper" ] && kill "$sleeper" 2>/dev/null; rm -rf "$tmp"' EXIT

export K2_RELEASE_LOCK="$tmp/k2-release.lock"
export K2_RELEASE_NODE_BIN="$tmp/no-such-k2-node"
export K2_RELEASE_NODE_HOME="$tmp/node-home"
export K2_RELEASE_NODE_POLL_SECS=0
mkdir -p "$K2_RELEASE_NODE_HOME"

# in_child <script> → runs bash with the library sourced; prints its stderr+stdout.
in_child() {
    bash -c "set -euo pipefail; source '$LIB'; $1" 2>&1
}

echo "== lock write and cleanup =="
out="$(in_child 'k2_release_lock_acquire 9.9.9; cat "$K2_RELEASE_LOCK"; echo "SELF=$$"; k2_release_lock_release')"
self="$(printf '%s\n' "$out" | sed -n 's/^SELF=//p')"
if printf '%s\n' "$out" | grep -q '^version=9.9.9$' \
   && printf '%s\n' "$out" | grep -qx "pid=${self}" \
   && printf '%s\n' "$out" | grep -Eq '^started=[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z$'; then
    pass "lock holds start time, version and pid"
else
    fail "lock holds start time, version and pid" "$out"
fi
[ ! -e "$K2_RELEASE_LOCK" ] && pass "release removes our lock" || fail "release removes our lock" "still there"

for sig in exit INT TERM; do
    case "$sig" in
        exit) body='exit 3' ;;
        *) body="kill -$sig \$\$; sleep 5" ;;
    esac
    in_child "trap k2_release_lock_release EXIT; trap 'exit 130' INT; trap 'exit 143' TERM; k2_release_lock_acquire 9.9.9; $body" >/dev/null || true
    [ ! -e "$K2_RELEASE_LOCK" ] && pass "lock removed on $sig" || { fail "lock removed on $sig" "$(cat "$K2_RELEASE_LOCK")"; rm -f "$K2_RELEASE_LOCK"; }
done

out="$(in_child 'k2_release_lock_acquire 9.9.9; printf "started=x\nversion=9.9.8\npid=1\n" > "$K2_RELEASE_LOCK"; k2_release_lock_release')"
if [ -e "$K2_RELEASE_LOCK" ] && grep -qx 'pid=1' "$K2_RELEASE_LOCK" && printf '%s' "$out" | grep -q "no longer ours"; then
    pass "a lock that is no longer ours is left in place"
else
    fail "not ours left in place" "$out"
fi
rm -f "$K2_RELEASE_LOCK"

sleep 60 &
sleeper=$!
printf 'started=x\nversion=9.9.8\npid=%s\n' "$sleeper" > "$K2_RELEASE_LOCK"
if out="$(in_child 'k2_release_lock_acquire 9.9.9')"; then
    fail "a live holder stops the release" "acquired: $out"
elif printf '%s' "$out" | grep -q "held by a running process (pid ${sleeper})" && grep -qx 'version=9.9.8' "$K2_RELEASE_LOCK"; then
    pass "a live holder stops the release and its lock is untouched"
else
    fail "a live holder stops the release" "$out"
fi
kill "$sleeper" 2>/dev/null || true
wait "$sleeper" 2>/dev/null || true
sleeper=""

# The holder is gone now: stale lock.
out="$(in_child 'k2_release_lock_acquire 9.9.9; echo "SELF=$$"; k2_release_lock_release')"
printf '%s' "$out" | grep -q "Removing a stale" && printf '%s' "$out" | grep -q "Release lock set" \
    && pass "a stale lock is replaced" || fail "stale lock replaced" "$out"
[ ! -e "$K2_RELEASE_LOCK" ] || { fail "stale case cleaned up" "$(cat "$K2_RELEASE_LOCK")"; rm -f "$K2_RELEASE_LOCK"; }

echo "== wait for a running node job =="
out="$(in_child 'k2_release_wait_node_idle')"
[ -z "$out" ] && pass "no node binary: no wait, no output" || fail "no node binary" "$out"

stub="$tmp/k2-node"
export K2_RELEASE_NODE_BIN="$stub"
cat > "$stub" <<'STUB'
#!/usr/bin/env bash
[ "$1" = status ] && [ "$2" = --home ] && [ -n "$3" ] || { echo "bad args: $*" >&2; exit 2; }
n="$(cat "$STUB_N" 2>/dev/null || echo 0)"; n=$((n + 1)); echo "$n" > "$STUB_N"
echo "node: z3mbpZ"
if [ "$n" -lt "${STUB_BUSY_FOR:-0}" ]; then echo "running: 4f2a91c0 (cargo test)"; else echo "running: -"; fi
echo "queued: 0"
STUB
chmod +x "$stub"

export STUB_N="$tmp/n1" STUB_BUSY_FOR=3
out="$(in_child 'k2_release_wait_node_idle')"
if [ "$(cat "$STUB_N")" = 3 ] && printf '%s' "$out" | grep -q "Waiting for the compute node job to finish (running: 4f2a91c0 (cargo test)" \
   && printf '%s' "$out" | grep -q "Compute node idle (running: -)"; then
    pass "waits until running: - (polled 3 times, one waiting line)"
else
    fail "waits until idle" "n=$(cat "$STUB_N") $out"
fi
[ "$(printf '%s\n' "$out" | grep -c 'Waiting for the compute node')" = 1 ] \
    && pass "waiting line throttled to every 30 s" || fail "throttle" "$out"

export STUB_N="$tmp/n2" STUB_BUSY_FOR=1000000
out="$(K2_RELEASE_NODE_WAIT_MIN=0 in_child 'k2_release_wait_node_idle; echo CONTINUED')"
printf '%s' "$out" | grep -q "still running after 0 min (running: 4f2a91c0 (cargo test)). Not killing it" \
    && printf '%s' "$out" | grep -q CONTINUED \
    && pass "timeout: clear message, job not killed, release continues" || fail "timeout" "$out"

printf '#!/bin/sh\necho "open /var/k2node/node.db: permission denied" >&2\nexit 1\n' > "$stub"
out="$(in_child 'k2_release_wait_node_idle; echo CONTINUED')"
printf '%s' "$out" | grep -q "k2-node status\` failed (open /var/k2node/node.db: permission denied); lock is set" \
    && printf '%s' "$out" | grep -q CONTINUED \
    && pass "status needs sudo: logs it and continues" || fail "status needs sudo" "$out"

chmod 000 "$K2_RELEASE_NODE_HOME"
if [ -r "$K2_RELEASE_NODE_HOME" ]; then
    # root can read a 000 dir; the check below would be meaningless.
    chmod 755 "$K2_RELEASE_NODE_HOME"
    export K2_RELEASE_NODE_HOME="$tmp/missing-home"
fi
out="$(in_child 'k2_release_wait_node_idle; echo CONTINUED')"
chmod 755 "$tmp/node-home" 2>/dev/null || true
printf '%s' "$out" | grep -q "not readable without sudo; lock is set" && printf '%s' "$out" | grep -q CONTINUED \
    && pass "unreadable node home: logs it and continues" || fail "unreadable home" "$out"

echo "== release.sh wiring =="
RS="$ROOT/scripts/release.sh"
grep -q 'source "$(cd "$(dirname "$0")" && pwd)/release-lock.sh"' "$RS" \
    && grep -q 'k2_release_lock_acquire "$VERSION"' "$RS" \
    && grep -q "trap 'k2_release_lock_release' EXIT" "$RS" \
    && grep -q '^    k2_release_wait_node_idle$' "$RS" \
    && pass "release.sh takes the lock and waits for the node" || fail "release.sh wiring" "missing lines"
# The Arch gate's EXIT trap replaces the early one, so it must release too.
awk '/^k2_arch_gate_on_exit\(\) \{/,/^\}/' "$RS" | grep -q 'k2_release_lock_release' \
    && pass "the Arch gate exit trap also releases the lock" || fail "arch gate trap releases" "not found"
# The lock comes before any build step.
lock_line="$(grep -n 'k2_release_lock_acquire "$VERSION"' "$RS" | cut -d: -f1)"
cargo_line="$(grep -n 'command -v cargo' "$RS" | head -1 | cut -d: -f1)"
[ "$lock_line" -lt "$cargo_line" ] && pass "lock is taken at the very start" || fail "lock first" "$lock_line vs $cargo_line"

echo ""
echo "release-lock: ${pass} passed, ${fail} failed"
[ "$fail" = 0 ]
