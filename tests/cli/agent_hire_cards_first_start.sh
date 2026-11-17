#!/usr/bin/env bash
# A template hire's cards wait for the new agent's first start
# (prd-agent-templates-v1 §9, T3), end to end against a real k2-daemon:
#
#   1. An agent ("Boss", its pinned chat started on a shim harness, so its
#      passport is a real main session) hires a Developer. Boss can't give
#      compute.use, permission cards are on, and the new agent has no
#      Thread yet: the plan says "card", the lock keeps compute.use in
#      cards_pending, and no request exists yet.
#   2. The new agent's first chat starts (the same start every session
#      passes, v2_session_map::register): the card is filed on THAT agent's
#      Thread, for exactly the switches the lock held, and the lock clears.
#   3. A second start files nothing more.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped; the agent
# programs are shims that only read their input. Never touches ~/.k2.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-hirecards-shim-XXXXXX)"
for cli in claude codex; do
    cat >"$SHIM_DIR/$cli" <<EOF
#!/bin/sh
case "\$1" in --version|-v|version|--help|app-server) echo "0.0.0-shim"; exit 0 ;; esac
pwd -P >>"$SHIM_DIR/$cli.cwd"
env >"$SHIM_DIR/$cli.env"
printf '\033[?2004h'
exec cat >>"$SHIM_DIR/$cli.stdin"
EOF
    chmod +x "$SHIM_DIR/$cli"
done
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" "$K2_CLI" "$@"
}
PASSPORT=""
as_boss() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_HOST=127.0.0.1 \
        K2_PORT="$K2SO_PORT" K2_HOOK_TOKEN="$PASSPORT" \
        K2_HOOK_SOCK="$SHIM_DIR/not-a-socket.sock" K2_PROJECT_PATH="$BOSS" "$K2_CLI" "$@"
}

pass=0
fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail" >&2; tail -40 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
starts_in() {
    local n
    n="$(grep -Fxc -- "$(cd "$1" && pwd -P)" "$SHIM_DIR/claude.cwd" 2>/dev/null || true)"
    echo "${n:-0}"
}
wait_start_in() {
    local dir="$1" want="$2" i
    for i in $(seq 1 150); do
        [ "$(starts_in "$dir")" -ge "$want" ] && return 0
        sleep 0.1
    done
    fail "want $want start(s) in $dir, got $(starts_in "$dir")"
}
# Waiting requests a person may answer (the owner sees every one).
pending_for() {
    curl -s "http://127.0.0.1:$K2SO_PORT/cli/permission/pending?token=$K2SO_TOKEN" \
        | jq -c --arg a "$1" '[.requests[] | select(.agent == $a)]'
}
lock_pending() { sed -n 's/^cards_pending = //p' "$1/.k2/agent/template.lock"; }

echo "== Boss: an agent that may hire, with its pinned chat running =="
BOSS="$SANDBOX_HOME/work/boss-$RANDOM$RANDOM"
mkdir -p "$SANDBOX_HOME/work"
k2 agent hire "$BOSS" --name "Boss" --json >"$SANDBOX_HOME/boss.json" || fail "hire Boss: $(cat "$SANDBOX_HOME/boss.json")"
k2 permission beta on >/dev/null || fail "permission cards on (server)"
k2 permission agent "$BOSS" on >/dev/null || fail "Boss's own cards switch"
k2 agent access set "$BOSS" agents_hire on >/dev/null || fail "Boss may hire"
k2 agent restart "$BOSS" --harness claude --json >/dev/null || fail "start Boss's pinned chat"
wait_start_in "$BOSS" 1
for i in $(seq 1 100); do
    PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SHIM_DIR/claude.env" 2>/dev/null | head -1 || true)"
    [ -n "$PASSPORT" ] && break
    sleep 0.1
done
[ -n "$PASSPORT" ] || fail "no passport for Boss's chat"
ok "Boss runs, with its passport"

echo "== Boss hires a Developer: compute.use is a card, waiting for the first start =="
hired="$(as_boss agent hire --template developer --name "Dev" --agent claude --json)" || fail "hire as Boss: $hired"
DEV="$(jq -r .path <<<"$hired")"
[ "$DEV" = "$SANDBOX_HOME/agents/dev" ] || fail "Dev under the hire root: $DEV"
rows() { jq -c '[.actions[] | select(.step == "apply-template") | .rows[] | select(.step == "permission") | {target, status}]' <<<"$hired"; }
[ "$(rows)" = '[{"target":"permission-cards","status":"done"},{"target":"compute.use","status":"card"}]' ] \
    || fail "Boss passes on its cards switch; compute.use is a card: $(rows)"
jq -e '.actions[] | select(.step == "apply-template") | .rows[] | select(.target == "compute.use") | .reason | test("when it first starts")' <<<"$hired" >/dev/null \
    || fail "the card waits for the first start: $hired"
[ "$(lock_pending "$DEV")" = '["compute.use"]' ] || fail "the lock keeps it: $(cat "$DEV/.k2/agent/template.lock")"
[ "$(pending_for dev)" = "[]" ] || fail "nothing filed before Dev has a Thread: $(pending_for dev)"
[ "$(starts_in "$DEV")" = 0 ] || fail "the hire started nothing"
ok "card waiting in the lock, nothing filed yet"

echo "== Dev's first start files exactly that card, on Dev's Thread =="
k2 agent restart "$DEV" --harness claude --json >/dev/null || fail "start Dev's chat"
wait_start_in "$DEV" 1
got="[]"
for i in $(seq 1 300); do
    got="$(pending_for dev)"
    [ "$got" != "[]" ] && break
    sleep 0.2
done
[ "$(jq -c '[.[].action]' <<<"$got")" = '["compute.use"]' ] || fail "exactly the lock's switches: $got"
[ "$(jq -r '.[0].addr' <<<"$got")" = "dev" ] || fail "on Dev's own Thread: $got"
[ -z "$(lock_pending "$DEV")" ] || fail "the lock clears once filed: $(cat "$DEV/.k2/agent/template.lock")"
grep -q '"compute.use"' <(sed -n 's/^carded = //p' "$DEV/.k2/agent/template.lock") || fail "the lock still says it was asked"
ok "filed on Dev's Thread at its first start"

echo "== a second start files nothing more =="
k2 agent restart "$DEV" --harness claude --json >/dev/null || fail "restart Dev"
wait_start_in "$DEV" 2
sleep 3
[ "$(pending_for dev | jq length)" = 1 ] || fail "still one card: $(pending_for dev)"
ok "once"

echo
echo "Results: $pass passed"
