#!/usr/bin/env bash
# GH #17: `k2 agent restart <name>` bounces the agent's pinned chat so it
# reloads its charter / .mcp.json, and reports the old and new session ids.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped. The agent
# CLI is a shim (`exec cat`) that records its pid, so nothing real runs and
# the test can prove the old process is gone. Loud: every check must pass.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-restart-shim-XXXXXX)"
PIDS="$SHIM_DIR/pids"
: >"$PIDS"
for cli in claude codex grok gemini cursor-agent pi; do
    printf '#!/bin/sh\necho "$$ $0 $*" >>"%s.log"\ncase "$1" in --version|-v|version) echo "0.0.0-shim"; exit 0 ;; esac\necho $$ >>"%s"\nexec cat\n' "$PIDS" "$PIDS" >"$SHIM_DIR/$cli"
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

pass=0
fail() { echo "FAIL: $*" >&2; cat "$PIDS.log" >&2; echo "--- daemon log tail" >&2; tail -30 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

wait_pids() {
    local want="$1" i
    for i in $(seq 1 50); do
        [ "$(grep -c . "$PIDS" || true)" -ge "$want" ] && return 0
        sleep 0.1
    done
    fail "want $want shim start(s), got: $(cat "$PIDS")"
}

dir="$SANDBOX_HOME/work/desk-$RANDOM$RANDOM"
mkdir -p "$(dirname "$dir")"
k2 agent hire "$dir" --name "Desk" --json >"$SANDBOX_HOME/hire.json" \
    || fail "hire failed: $(cat "$SANDBOX_HOME/hire.json")"

echo "== help and usage =="
help="$(k2 agent --help)"
grep -Fq "restart <name>" <<<"$help" || fail "k2 agent --help must list restart: $help"
rhelp="$(k2 agent restart --help)"
grep -Fq "SAME conversation" <<<"$rhelp" || fail "restart --help text: $rhelp"
rc=0; k2 agent restart >/dev/null 2>&1 || rc=$?
[ "$rc" = 2 ] || fail "missing name must exit 2, got $rc"
rc=0; k2 agent restart no-such-agent-xyz >/dev/null 2>&1 || rc=$?
[ "$rc" = 4 ] || fail "unknown agent must exit 4, got $rc"
ok "help lists restart; usage exit 2; unknown exit 4"

echo "== dry-run with no live session changes nothing =="
out="$(k2 agent restart "$dir" --dry-run --json)" || fail "dry-run failed: $out"
[ "$(jq -r .dryRun <<<"$out")" = true ] || fail "dryRun flag: $out"
[ "$(jq -r .oldSessionId <<<"$out")" = null ] || fail "no live session yet: $out"
[ "$(grep -c . "$PIDS" || true)" = 0 ] || fail "dry-run spawned: $(cat "$PIDS")"
ok "dry-run spawns nothing"

echo "== first restart starts the session =="
out="$(k2 agent restart "$dir" --json)" || fail "restart #1 failed: $out"
s1="$(jq -r .sessionId <<<"$out")"
[ -n "$s1" ] && [ "$s1" != null ] || fail "restart #1 sessionId: $out"
[ "$(jq -r .oldSessionId <<<"$out")" = null ] || fail "restart #1 old id must be null: $out"
wait_pids 1
pid1="$(sed -n 1p "$PIDS")"
kill -0 "$pid1" || fail "first shim $pid1 not running"
live="$(k2 agent conf "$dir" --json | jq -r .live.sessionId)"
[ "$live" = "$s1" ] || fail "conf live.sessionId $live want $s1"
ok "started $s1 (pid $pid1)"

echo "== dry-run with a live session names it =="
out="$(k2 agent restart "$dir" --dry-run --json)" || fail "dry-run #2 failed: $out"
[ "$(jq -r .oldSessionId <<<"$out")" = "$s1" ] || fail "dry-run old id: $out"
[ "$(grep -c . "$PIDS")" = 1 ] || fail "dry-run #2 spawned: $(cat "$PIDS")"
ok "dry-run names $s1"

echo "== second restart replaces the session and stops the old process =="
out="$(k2 agent restart "$dir" --json)" || fail "restart #2 failed: $out"
[ "$(jq -r .oldSessionId <<<"$out")" = "$s1" ] || fail "restart #2 old id: $out"
s2="$(jq -r .sessionId <<<"$out")"
[ -n "$s2" ] && [ "$s2" != null ] && [ "$s2" != "$s1" ] || fail "restart #2 new id: $out"
wait_pids 2
pid2="$(sed -n 2p "$PIDS")"
kill -0 "$pid2" || fail "new shim $pid2 not running"
for i in $(seq 1 50); do kill -0 "$pid1" 2>/dev/null || break; sleep 0.1; done
if kill -0 "$pid1" 2>/dev/null; then fail "old shim $pid1 still running after restart"; fi
live="$(k2 agent conf "$dir" --json | jq -r .live.sessionId)"
[ "$live" = "$s2" ] || fail "conf live.sessionId $live want $s2"
ok "$s1 → $s2; old pid $pid1 gone, new pid $pid2 running"

echo "== human output =="
txt="$(k2 agent restart "$dir")" || fail "plain restart failed: $txt"
grep -Eq "^Desk restarted: session $s2 → [0-9a-f-]+\.$" <<<"$txt" || fail "plain output: $txt"
ok "plain output names old → new"

echo ""
echo "Results: $pass passed, 0 failed"
