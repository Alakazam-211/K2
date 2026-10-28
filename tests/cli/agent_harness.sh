#!/usr/bin/env bash
# 0.46.6 (Rosson): `k2 agent harness <workspace> <harness>` moves a pinned
# chat to another harness; plus two CLI bugs from Appa on baden:
# `k2 sessions live <path>` crashed on a refusal body, and
# `k2 heartbeat show <name> --workspace <ws>` ignored --workspace.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped. Agent CLIs
# are `exec cat` shims that record argv + env, so nothing real runs.
#
#   1. help + usage (exit 2), unknown harness (2), unknown workspace (4);
#   2. sessions live: the owner gets the table; an agent's passport gets a
#      clean refusal (exit 3), never a python traceback;
#   3. heartbeat show --workspace reads THAT workspace (owner); an agent
#      naming another workspace is refused (exit 3);
#   4. an agent can't move its own pinned chat (exit 3, harness_needs_person,
#      nothing spawned); with permission cards on, the refusal ends with
#      "Ask with: k2 permission request agent.harness-switch harness=grok";
#      the agent asks, the owner approves, and the pinned chat moves;
#   5. the owner moves it back to claude: a fresh chat (no --resume), the
#      old one named; a second run is "already on" and spawns nothing.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-harness-shim-XXXXXX)"
for cli in claude grok codex; do
    cat >"$SHIM_DIR/$cli" <<EOF
#!/bin/sh
case "\$1" in --version|-v|version) echo "0.0.0-shim"; exit 0 ;; esac
printf '%s\n' "\$@" >>"$SHIM_DIR/$cli.argv"
echo "--" >>"$SHIM_DIR/$cli.argv"
env >"$SHIM_DIR/$cli.env"
exec cat
EOF
    chmod +x "$SHIM_DIR/$cli"
done
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" ${K2_PROJECT_PATH:+K2_PROJECT_PATH="$K2_PROJECT_PATH"} "$K2_CLI" "$@"
}
PASSPORT=""
as_agent() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_HOST=127.0.0.1 \
        K2_PORT="$K2SO_PORT" K2_HOOK_TOKEN="$PASSPORT" \
        K2_HOOK_SOCK="$SHIM_DIR/not-a-socket.sock" K2_PROJECT_PATH="$A" "$K2_CLI" "$@"
}

pass=0
fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail" >&2; tail -40 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
run() { set +e; out="$("$@" 2>&1)"; rc=$?; set -e; }
has() { grep -Fq -- "$2" <<<"$1" || fail "missing '$2' in: $1"; }
hasnt() { if grep -Fq -- "$2" <<<"$1"; then fail "unexpected '$2' in: $1"; fi; }
starts() {
    if [ -f "$SHIM_DIR/$1.argv" ]; then awk '$0 == "--" { n++ } END { print n + 0 }' "$SHIM_DIR/$1.argv"; else echo 0; fi
}
wait_starts() {
    local cli="$1" want="$2" i
    for i in $(seq 1 100); do
        [ "$(starts "$cli")" -ge "$want" ] && return 0
        sleep 0.1
    done
    fail "want $want $cli start(s), got $(starts "$cli")"
}
pinned_provider() {
    curl -s "http://127.0.0.1:$K2SO_PORT/cli/workspace/resume-chat-args?token=$K2SO_TOKEN&project=$(jq -rn --arg p "$1" '$p|@uri')" | jq -r .provider
}

echo "== help and usage =="
help="$(k2 agent --help)"
has "$help" "harness <name> <harness>"
hhelp="$(k2 agent harness --help)"
has "$hhelp" "Default Agent only"
has "$hhelp" "k2 permission request agent.harness-switch harness=<harness>"
run k2 agent harness; [ "$rc" = 2 ] || fail "no args exit $rc: $out"
run k2 agent harness ws-only; [ "$rc" = 2 ] || fail "no harness exit $rc: $out"
run k2 agent harness ws claude --bogus; [ "$rc" = 2 ] || fail "bad flag exit $rc: $out"
ok "help names harness; usage exits 2"

A="$SANDBOX_HOME/work/harness-a-$RANDOM$RANDOM"
B="$SANDBOX_HOME/work/harness-b-$RANDOM$RANDOM"
mkdir -p "$SANDBOX_HOME/work"
k2 agent hire "$A" --name "Desk" --json >"$SANDBOX_HOME/hire-a.json" || fail "hire A: $(cat "$SANDBOX_HOME/hire-a.json")"
k2 agent hire "$B" --name "Ops" --json >"$SANDBOX_HOME/hire-b.json" || fail "hire B: $(cat "$SANDBOX_HOME/hire-b.json")"
k2 agent restart "$A" --json >/dev/null || fail "start A's pinned chat"
wait_starts claude 1
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SHIM_DIR/claude.env" | head -1)"
[ -n "$PASSPORT" ] || fail "the pinned chat got no passport"
[ "$(pinned_provider "$A")" = claude ] || fail "A starts on claude"

run k2 agent harness "$A" aider; [ "$rc" = 2 ] || fail "unknown harness exit $rc: $out"
has "$out" "unknown harness"
run k2 agent harness no-such-workspace-xyz claude; [ "$rc" = 4 ] || fail "unknown workspace exit $rc: $out"
ok "unknown harness exit 2; unknown workspace exit 4"

echo "== sessions live =="
run k2 sessions live "$A"; [ "$rc" = 0 ] || fail "owner sessions live exit $rc: $out"
has "$out" "live session(s)"
hasnt "$out" "Traceback"
[ "$(k2 sessions live "$A" --count)" -ge 1 ] || fail "count"
run as_agent sessions live "$A"
[ "$rc" = 3 ] || fail "agent sessions live exit $rc: $out"
has "$out" "Error:"
hasnt "$out" "Traceback"
hasnt "$out" "AttributeError"
hasnt "$out" "live session(s)"
ok "owner table; agent refused cleanly (exit 3)"

echo "== heartbeat show --workspace =="
K2_PROJECT_PATH="$B" k2 heartbeat schedule add --name nightly --daily --time 07:00 --instructions "B-NIGHTLY-$$" >/dev/null \
    || fail "add B's heartbeat"
K2_PROJECT_PATH="$A"
run k2 heartbeat show nightly
[ "$rc" = 1 ] || fail "A has no nightly: exit $rc: $out"
has "$out" "No heartbeat named"
run k2 heartbeat show nightly --workspace "$B"
unset K2_PROJECT_PATH
[ "$rc" = 0 ] || fail "--workspace B exit $rc: $out"
has "$out" "B-NIGHTLY-$$"
has "$out" "Workspace:"
run as_agent heartbeat show nightly --workspace "$B"
[ "$rc" = 3 ] || fail "agent reading B exit $rc: $out"
has "$out" "other_workspace"
hasnt "$out" "B-NIGHTLY-$$"
ok "--workspace reads B; an agent can't"

echo "== an agent can't move its own pinned chat =="
run as_agent agent harness "$A" grok
[ "$rc" = 3 ] || fail "agent harness exit $rc: $out"
has "$out" "harness_needs_person"
has "$out" "k2 agent harness"
[ ! -e "$SHIM_DIR/grok.argv" ] || fail "grok spawned for a refused switch"
[ "$(pinned_provider "$A")" = claude ] || fail "still claude"
# The older door refuses the same way.
run as_agent workspace swap-canonical "$A" --agent grok
[ "$rc" = 3 ] || fail "agent swap-canonical exit $rc: $out"
has "$out" "restarts your own session"
[ ! -e "$SHIM_DIR/grok.argv" ] || fail "grok spawned for a refused swap"
ok "held, not switched (both doors)"

echo "== with permission cards on: ask, approve, moved =="
k2 permission beta on >/dev/null || fail "beta on"
k2 permission agent "$A" on >/dev/null || fail "agent switch on"
run as_agent agent harness "$A" grok
[ "$rc" = 3 ] || fail "agent harness (cards on) exit $rc: $out"
has "$out" "Ask with: k2 permission request agent.harness-switch harness=grok"
run as_agent permission request agent.harness-switch harness=grok --why "Baden wants this agent on grok" --json
[ "$rc" = 0 ] || fail "request exit $rc: $out"
rid="$(jq -r .id <<<"$out")"
[ -n "$rid" ] && [ "$rid" != null ] || fail "request id: $out"
[ "$(jq -r .status <<<"$out")" = pending ] || fail "pending: $out"
[ ! -e "$SHIM_DIR/grok.argv" ] || fail "asking spawned grok"
run k2 permission approve "$rid"
[ "$rc" = 0 ] || fail "approve exit $rc: $out"
wait_starts grok 1
grok_argv="$(cat "$SHIM_DIR/grok.argv")"
has "$grok_argv" "--session-id"
hasnt "$grok_argv" "--resume"
[ "$(pinned_provider "$A")" = grok ] || fail "approved: A on grok"
ok "the owner's Allow moved A to grok"

echo "== the owner moves it back; idempotent =="
claude_before="$(starts claude)"
run k2 agent harness "$A" grok
[ "$rc" = 0 ] || fail "already on grok exit $rc: $out"
has "$out" "already on grok"
[ "$(starts grok)" = 1 ] || fail "already-on spawned grok again"
run k2 agent harness "$A" claude --dry-run --json
[ "$rc" = 0 ] || fail "dry run exit $rc: $out"
[ "$(jq -r .fromHarness <<<"$out")" = grok ] && [ "$(jq -r .switched <<<"$out")" = false ] || fail "dry run: $out"
[ "$(starts claude)" = "$claude_before" ] || fail "dry run spawned"
run k2 agent harness "$A" claude
[ "$rc" = 0 ] || fail "move back exit $rc: $out"
has "$out" "pinned chat: grok -> claude"
has "$out" "Old chat:"
wait_starts claude $((claude_before + 1))
last_claude="$(awk 'BEGIN{RS="--\n"} {last=$0} END{print last}' "$SHIM_DIR/claude.argv")"
has "$last_claude" "--session-id"
hasnt "$last_claude" "--resume"
[ "$(pinned_provider "$A")" = claude ] || fail "back on claude"
run k2 agent harness "$A" claude
[ "$rc" = 0 ] || fail "second run exit $rc: $out"
has "$out" "already on claude"
[ "$(starts claude)" = $((claude_before + 1)) ] || fail "already-on spawned claude"
ok "grok -> claude; second run changes nothing"

AUDIT="$(cat "$SANDBOX_HOME/.k2/auth-audit.jsonl" 2>/dev/null || cat "$SANDBOX_HOME/.k2so/auth-audit.jsonl")"
has "$AUDIT" '"harness-switch"'
has "$AUDIT" "refused:needs_person"
has "$AUDIT" "switched:claude"

echo ""
echo "Results: $pass passed, 0 failed"
