#!/usr/bin/env bash
# `k2 workspace triage` from an agent's session lists only what K2 lets
# that agent see: its own workspace's inbox (or a connected peer's), never
# another workspace's, whichever folder the CLI is pointed at.
#
# End to end against a foreground sandbox daemon built from THIS worktree
# (temp HOME, no client attached). Two workspaces, A and B, are registered,
# each with one inbox item. A's pinned chat is a shim that records the
# passport and cell socket K2 hands it; the CLI then runs with those, as
# the agent does:
#   1. the owner (no session) lists each workspace's own item;
#   2. the agent of A with its cwd in B: A's item only, never B's;
#   3. the agent of A with K2_PROJECT_PATH set to B: the same;
#   4. the agent of A in its own workspace: A's item;
#   5. the same agent over TCP only (no cell socket): pointed at B it
#      never sees B's item; its own still lists.
#
#   cargo build -p k2-daemon && bash tests/cli/workspace_triage_headless.sh
#
# Every check fails loudly with the command output.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

# Never inherit a session's identity from the caller.
for v in $(env | grep -E '^(K2|K2SO|K5)_' | cut -d= -f1); do unset "$v"; done

# Short root: the cell socket (~/.k2/run/cells/<uuid>.sock) must fit SUN_LEN.
WORK="$(mktemp -d /tmp/k2tri.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
mkdir -p "$SB/.k2" "$SB/shim" "$SB/tmp" "$SB/run"
DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.3
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    pkill -9 -f "frpc -c $SB/" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

pass=0
fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log tail ---" >&2
    tail -30 "$SB/daemon.log" >&2 2>/dev/null || true
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
# run <who> <cwd> [VAR=value...] -- <args…> → OUT, ERR, RC. <who>: owner,
# agent (passport + cell socket) or agent-tcp (passport, no cell socket).
run() {
    local who="$1" cwd="$2"; shift 2
    local -a envs=(HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" K2_AIRGAP=1)
    case "$who" in
        agent) envs+=(K2_HOOK_TOKEN="$PASSPORT" K2_HOOK_SOCK="$SOCK") ;;
        agent-tcp) envs+=(K2_HOOK_TOKEN="$PASSPORT" K2_HOOK_SOCK="$WORK/no-cell.sock") ;;
        owner) ;;
        *) fail "run: unknown caller $who" ;;
    esac
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    set +e
    OUT="$(cd "$cwd" && env -i PATH="$PATH" "${envs[@]}" "$K2_CLI" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}
want_rc() { [ "$RC" = "$1" ] || fail "$2: exit $RC (want $1)
stdout: $OUT
stderr: $ERR"; }
has() { [[ "$1" == *"$2"* ]] || fail "$3: missing $(printf %q "$2") in:
$1
stderr: $ERR"; }
lacks() { [[ "$1" != *"$2"* ]] || fail "$3: unexpected $(printf %q "$2") in:
$1
stderr: $ERR"; }

echo "== daemon =="
DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || fail "k2-daemon not built (cargo build -p k2-daemon)"
# The pinned chat's harness is a shim that records its env (the passport).
cat >"$SB/shim/claude" <<EOF
#!/bin/sh
case "\$1" in --version|-v|version) echo "0.0.0-shim"; exit 0 ;; esac
env > "$SB/claude.env"
exec /bin/cat > /dev/null
EOF
chmod +x "$SB/shim/claude"
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
    HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny K2_AIRGAP=1 \
    "$DAEMON_BIN" >"$SB/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 200); do
    [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ] && break
    sleep 0.1
done
[ -s "$SB/.k2/daemon.port" ] || fail "daemon never wrote daemon.port"
DPORT="$(cat "$SB/.k2/daemon.port")"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
BASE="http://127.0.0.1:$DPORT"
phase=""
for _ in $(seq 1 300); do
    phase="$(curl -s "$BASE/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || fail "daemon never reached ready"
ok "sandbox daemon ready ($DAEMON_BIN)"

echo "== two workspaces, an inbox item each, A's agent passport =="
A="$WORK/ws-a"
B="$WORK/ws-b"
mkdir -p "$A" "$B"
for w in "$A" "$B"; do
    reg="$(curl -s -X POST -H 'Content-Type: application/json' \
        -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$w")" \
        "$BASE/cli/projects/add-without-git?token=$DTOKEN")"
    python3 -c 'import json, sys; j = json.loads(sys.argv[1]); sys.exit(0 if isinstance(j, dict) and "error" not in j and j.get("path") == sys.argv[2] else 1)' "$reg" "$w" ||
        fail "register $w: $reg"
    # The cwd walk finds a workspace by its .k2 folder.
    mkdir -p "$w/.k2"
done
ok "workspaces A and B registered"
A_ITEM="Alpha-own-item-$RANDOM$RANDOM"
B_ITEM="Bravo-private-item-$RANDOM$RANDOM"
run owner "$A" K2_PROJECT_PATH="$A" -- inbox compose --title "$A_ITEM" --body "for A"
want_rc 0 "compose A's item"
run owner "$B" K2_PROJECT_PATH="$B" -- inbox compose --title "$B_ITEM" --body "for B"
want_rc 0 "compose B's item"
# A new server's first pinned chat waits for a harness pick (409
# harness_choice_needed): pick claude, the shim, as the Chat tab would.
resp="$(curl -s -X POST "$BASE/cli/workspace/ensure-pinned-chat?token=$DTOKEN" \
    -H 'Content-Type: application/json' --data-raw "$(python3 -c 'import json,sys; print(json.dumps({"project": sys.argv[1], "harness": "claude"}))' "$A")")"
case "$resp" in *'"sessionId"'*) ;; *) fail "pinned chat: $resp" ;; esac
for _ in $(seq 1 100); do grep -q '^K2_HOOK_SOCK=.' "$SB/claude.env" 2>/dev/null && break; sleep 0.05; done
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
SOCK="$(sed -n 's/^K2_HOOK_SOCK=//p' "$SB/claude.env" | head -1)"
[ -n "$PASSPORT" ] || fail "the pinned chat got no passport"
[ -S "$SOCK" ] || fail "the pinned chat's cell socket is missing: '$SOCK'"
ok "A's agent passport + cell socket"

echo "== the owner: each workspace's own inbox =="
run owner "$A" K2_PROJECT_PATH="$A" -- workspace triage
want_rc 0 "owner triage A"
has "$OUT" "$A_ITEM" "owner triage A"
lacks "$OUT" "$B_ITEM" "owner triage A"
run owner "$B" K2_PROJECT_PATH="$B" -- workspace triage
want_rc 0 "owner triage B"
has "$OUT" "$B_ITEM" "owner triage B"
lacks "$OUT" "$A_ITEM" "owner triage B"
ok "the owner lists A's item in A and B's item in B"

echo "== A's agent, its cwd in B: never B's item =="
run agent "$B" -- workspace triage
want_rc 0 "agent cwd=B"
lacks "$OUT" "$B_ITEM" "agent cwd=B"
has "$OUT" "$A_ITEM" "agent cwd=B"
ok "agent, cwd in B: A's item only"

echo "== A's agent, K2_PROJECT_PATH set to B: never B's item =="
run agent "$A" K2_PROJECT_PATH="$B" -- workspace triage
want_rc 0 "agent K2_PROJECT_PATH=B"
lacks "$OUT" "$B_ITEM" "agent K2_PROJECT_PATH=B"
has "$OUT" "$A_ITEM" "agent K2_PROJECT_PATH=B"
ok "agent, K2_PROJECT_PATH=B: A's item only"

echo "== A's agent in its own workspace =="
run agent "$A" -- workspace triage
want_rc 0 "agent own"
has "$OUT" "$A_ITEM" "agent own"
lacks "$OUT" "$B_ITEM" "agent own"
ok "agent, own workspace: A's item"

echo "== A's agent over TCP only (no cell socket) =="
run agent-tcp "$A" K2_PROJECT_PATH="$B" -- workspace triage
lacks "$OUT" "$B_ITEM" "agent tcp K2_PROJECT_PATH=B"
lacks "$ERR" "$B_ITEM" "agent tcp K2_PROJECT_PATH=B (stderr)"
echo "    (tcp, pointed at B: exit $RC, stdout $(printf %q "$OUT"))"
ok "agent over TCP, pointed at B: never B's item"
run agent-tcp "$B" -- workspace triage
lacks "$OUT" "$B_ITEM" "agent tcp cwd=B"
ok "agent over TCP, cwd in B: never B's item"
run agent-tcp "$A" K2_PROJECT_PATH="$A" -- workspace triage
want_rc 0 "agent tcp own"
has "$OUT" "$A_ITEM" "agent tcp own"
ok "agent over TCP, own workspace: A's item"

echo
echo "workspace_triage_headless: $pass passed, 0 failed"
