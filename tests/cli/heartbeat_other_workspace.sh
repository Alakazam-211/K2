#!/usr/bin/env bash
# 0.46.6: pause / resume ANOTHER workspace's heartbeat. A manager agent was
# told to turn off two other workspaces' heartbeats; `k2 heartbeat schedule
# disable` bound to its own workspace and every spelling (--workspace,
# --project, ws/name, running it from the other folder) answered
# no_such_heartbeat.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped. Agent CLIs
# are `exec cat` shims, so nothing real runs.
#
#   1. help names --workspace and the card; bad flags exit 2;
#   2. the owner pauses and resumes B's heartbeat from outside B (--workspace
#      and <ws>/<name>); an unknown workspace exits 4;
#   3. the agent (A's pinned chat) naming B's heartbeat: exit 3,
#      other_workspace, nothing changes; a heartbeat B doesn't have:
#      no_such_heartbeat naming B; its own heartbeat works as before;
#   4. with permission cards on: the refusal ends with "Ask with: k2
#      permission request heartbeat.pause-other …"; the agent asks, the
#      owner approves, B's heartbeat is paused; audited.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-hbother-shim-XXXXXX)"
cat >"$SHIM_DIR/claude" <<EOF
#!/bin/sh
case "\$1" in --version|-v|version) echo "0.0.0-shim"; exit 0 ;; esac
env >"$SHIM_DIR/claude.env"
exec cat
EOF
chmod +x "$SHIM_DIR/claude"
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" ${K2_PROJECT_PATH:+K2_PROJECT_PATH="$K2_PROJECT_PATH"} "$K2_CLI" "$@"
}
PASSPORT=""
# The agent (A's pinned chat), its CLI pointed at <dir> (K2_PROJECT_PATH →
# project=); as_agent is from its own workspace.
as_agent_at() {
    local dir="$1"; shift
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_HOST=127.0.0.1 \
        K2_PORT="$K2SO_PORT" K2_HOOK_TOKEN="$PASSPORT" \
        K2_HOOK_SOCK="$SHIM_DIR/not-a-socket.sock" K2_PROJECT_PATH="$dir" "$K2_CLI" "$@"
}
as_agent() { as_agent_at "$A" "$@"; }

pass=0
fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail" >&2; tail -40 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
run() { set +e; out="$("$@" 2>&1)"; rc=$?; set -e; }
has() { grep -Fq -- "$2" <<<"$1" || fail "missing '$2' in: $1"; }
hasnt() { if grep -Fq -- "$2" <<<"$1"; then fail "unexpected '$2' in: $1"; fi; }
enabled_in() { # <workspace path> <name> → yes|no
    k2 heartbeat show "$2" --workspace "$1" --json | jq -r 'if .enabled then "yes" else "no" end'
}

echo "== help and usage =="
help="$(k2 heartbeat --help)"
has "$help" "heartbeat schedule disable <n> [--workspace <ws>]"
has "$help" "k2 permission request heartbeat.pause-other target=<ws> heartbeat=<n>"
run k2 heartbeat schedule disable triage --bogus; [ "$rc" = 2 ] || fail "bad flag exit $rc: $out"
run k2 heartbeat schedule disable triage --workspace; [ "$rc" = 2 ] || fail "--workspace without a value exit $rc: $out"
ok "help names --workspace and the card; usage exits 2"

A="$SANDBOX_HOME/work/hbo-a-$RANDOM$RANDOM"
B="$SANDBOX_HOME/work/hbo-b-$RANDOM$RANDOM"
mkdir -p "$SANDBOX_HOME/work"
k2 agent hire "$A" --name "Manager" --json >"$SANDBOX_HOME/hire-a.json" || fail "hire A: $(cat "$SANDBOX_HOME/hire-a.json")"
k2 agent hire "$B" --name "Companion" --json >"$SANDBOX_HOME/hire-b.json" || fail "hire B: $(cat "$SANDBOX_HOME/hire-b.json")"
K2_PROJECT_PATH="$B" k2 heartbeat schedule add --name triage --daily --time 07:00 --instructions "B-TRIAGE" >/dev/null || fail "add B's triage"
K2_PROJECT_PATH="$A" k2 heartbeat schedule add --name roundup --daily --time 08:00 --instructions "A-ROUNDUP" >/dev/null || fail "add A's roundup"
k2 agent restart "$A" --json >/dev/null || fail "start A's pinned chat"
for _ in $(seq 1 100); do [ -s "$SHIM_DIR/claude.env" ] && break; sleep 0.1; done
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SHIM_DIR/claude.env" | head -1)"
[ -n "$PASSPORT" ] || fail "the pinned chat got no passport"

echo "== the owner pauses and resumes B's heartbeat from outside B =="
run k2 heartbeat schedule disable triage --workspace "$B"
[ "$rc" = 0 ] || fail "owner disable exit $rc: $out"
has "$out" "Disabled heartbeat: triage (workspace "
BH="$(sed -n 's/.*(workspace \(.*\))$/\1/p' <<<"$out")"
[ -n "$BH" ] || fail "no workspace handle in: $out"
[ "$(enabled_in "$B" triage)" = no ] || fail "B's triage paused"
run k2 heartbeat enable "$BH/triage"
[ "$rc" = 0 ] || fail "owner enable ws/name exit $rc: $out"
[ "$(enabled_in "$B" triage)" = yes ] || fail "B's triage resumed"
run k2 heartbeat schedule list --workspace "$BH"
[ "$rc" = 0 ] || fail "owner list --workspace exit $rc: $out"
has "$out" "triage"
run k2 heartbeat schedule disable triage --workspace no-such-ws-xyz
[ "$rc" = 4 ] || fail "unknown workspace exit $rc: $out"
has "$out" "workspace not found"
ok "owner: --workspace and <ws>/<name>; unknown workspace exit 4"

echo "== the agent naming B's heartbeat: refused, nothing changes =="
for spelling in "triage --workspace $B" "triage --workspace $BH" "triage --project $BH" "$BH/triage"; do
    # shellcheck disable=SC2086
    run as_agent heartbeat schedule disable $spelling
    [ "$rc" = 3 ] || fail "agent '$spelling' exit $rc: $out"
    has "$out" "other_workspace"
    has "$out" "k2 heartbeat disable triage --workspace $BH"
    hasnt "$out" "no_such_heartbeat"
done
[ "$(enabled_in "$B" triage)" = yes ] || fail "an agent's refusal changed B"
run as_agent heartbeat schedule disable nope --workspace "$BH"
[ "$rc" = 1 ] || fail "agent missing heartbeat exit $rc: $out"
has "$out" "no_such_heartbeat"
has "$out" "in workspace $BH"
# Its own workspace's heartbeat, as before; and naming B's from its own
# workspace (the "ran it from the other folder" case) says where it is.
run as_agent heartbeat schedule disable roundup
[ "$rc" = 0 ] || fail "agent own disable exit $rc: $out"
has "$out" "Disabled heartbeat: roundup"
run as_agent heartbeat schedule enable roundup
[ "$rc" = 0 ] || fail "agent own enable exit $rc: $out"
run as_agent heartbeat schedule disable triage
[ "$rc" = 1 ] || fail "agent own-workspace triage exit $rc: $out"
has "$out" "k2 heartbeat disable triage --workspace $BH"
# project= is a spelling too: the agent's CLI pointed at B's folder.
run as_agent_at "$B" heartbeat schedule disable triage
[ "$rc" = 3 ] || fail "agent project=B exit $rc: $out"
has "$out" "other_workspace"
hasnt "$out" "no_such_heartbeat"
[ "$(enabled_in "$B" triage)" = yes ] || fail "the project= refusal changed B"
ok "agent: exit 3 other_workspace on every spelling (project= too); missing names B; own works"

echo "== with permission cards on: ask, approve, paused =="
k2 permission beta on >/dev/null || fail "beta on"
k2 permission agent "$A" on >/dev/null || fail "agent switch on"
run as_agent heartbeat schedule disable triage --workspace "$BH"
[ "$rc" = 3 ] || fail "agent (cards on) exit $rc: $out"
has "$out" "Ask with: k2 permission request heartbeat.pause-other"
has "$out" "target=$BH"
has "$out" "heartbeat=triage"
ask_line="$(grep -F 'Ask with: ' <<<"$out" | sed 's/^Ask with: k2 //; s/ --why .*$//')"
# shellcheck disable=SC2086
run as_agent $ask_line --why "The owner asked me to turn off Companion's triage heartbeat" --json
[ "$rc" = 0 ] || fail "request exit $rc: $out"
rid="$(jq -r .id <<<"$out")"
[ -n "$rid" ] && [ "$rid" != null ] || fail "request id: $out"
[ "$(jq -r .status <<<"$out")" = pending ] || fail "pending: $out"
[ "$(enabled_in "$B" triage)" = yes ] || fail "asking changed B"
run k2 permission approve "$rid"
[ "$rc" = 0 ] || fail "approve exit $rc: $out"
[ "$(enabled_in "$B" triage)" = no ] || fail "approved: B's triage paused"
ok "the owner's Allow paused B's triage"

AUDIT="$(cat "$SANDBOX_HOME/.k2/auth-audit.jsonl" 2>/dev/null || cat "$SANDBOX_HOME/.k2so/auth-audit.jsonl")"
has "$AUDIT" '"heartbeat-other"'
has "$AUDIT" "refused:other_workspace:$BH/triage"
has "$AUDIT" "paused:$BH/triage"
has "$AUDIT" "resumed:$BH/triage"
ok "every cross-workspace pause, resume and refusal is audited"

echo ""
echo "Results: $pass passed, 0 failed"
