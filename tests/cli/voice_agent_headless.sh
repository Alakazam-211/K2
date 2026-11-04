#!/usr/bin/env bash
# `k2 voice` from a real agent terminal (T3 on voicetest-pub, 11-04): a
# foreground sandbox daemon built from THIS worktree with the voice rooms
# preview on (no Sew), and an agent's real passport and cell socket from its
# pinned chat. Before the fix every verb answered the cell socket's bare
# "Invalid or missing auth token". Now rooms and status read; join answers
# agents_not_in_voice, a change owner_or_admin_only and moderation
# agents_cannot_moderate, each exit 3 with the code on stderr.
#
#   cargo build -p k2-daemon && bash tests/cli/voice_agent_headless.sh
#
# Every check fails loudly with the output.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }
command -v git >/dev/null || { echo "FAIL: git is required" >&2; exit 1; }


SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }

# Never inherit a session's identity from the caller.
for v in $(env | grep -E '^(K2|K2SO)_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d /tmp/k2voiceag.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
mkdir -p "$SB/.k2" "$SB/shim" "$SB/tmp" "$SB/run"
DAEMON_PID=""
DLOG="$SB/daemon.log"
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        for _ in $(seq 1 100); do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.1; done
        kill -9 "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    chmod -R u+w "$WORK" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

pass=0
fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log tail ---" >&2
    tail -40 "$DLOG" >&2 2>/dev/null || true
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
jcheck() {
    python3 - "$1" "$2" <<'PY' || return 1
import json, sys
j = json.loads(sys.argv[1])
if not eval(sys.argv[2], {"j": j}):
    sys.exit(1)
PY
}
jok() { jcheck "$1" "$2" || fail "$3: $2 is false for: $1"; ok "$3"; }

# cli [env K=V ...] -- <k2 args>: OUT, ERR, RC.
cli() {
    local -a envs=(HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT")
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    set +e
    OUT="$(env "${envs[@]}" "$K2_CLI" "$@" 2>"$WORK/cli.err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/cli.err")"
}
want_rc() { [ "$RC" = "$1" ] || fail "$2: exit $RC (want $1)
stdout: $OUT
stderr: $ERR"; }
has() { [[ "$1" == *"$2"* ]] || fail "$3: missing $(printf %q "$2") in:
$1"; }

echo "== daemon =="
# The agent's pinned chat runs this shim, which records its env: the real
# passport (K2_HOOK_TOKEN) and cell socket (K2_HOOK_SOCK) K2 hands an agent.
cat >"$SB/shim/claude" <<EOF
#!/bin/sh
env > "$SB/claude.env"
exec /bin/cat > /dev/null
EOF
chmod +x "$SB/shim/claude"
DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || fail "k2-daemon not built (cargo build -p k2-daemon)"
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
    HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny K2_VOICE_ROOMS=1 \
    "$DAEMON_BIN" >"$DLOG" 2>&1 &
DAEMON_PID=$!
phase=""
for _ in $(seq 1 400); do
    if [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ]; then
        DPORT="$(cat "$SB/.k2/daemon.port")"
        phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
        [ "$phase" = "ready" ] && break
    fi
    sleep 0.1
done
[ "$phase" = "ready" ] || fail "daemon never reached ready"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
BASE="http://127.0.0.1:$DPORT"
ok "sandbox daemon ready ($DAEMON_BIN)"

echo "== an agent with a real passport and cell socket =="
AG="$WORK/agentws"
mkdir -p "$AG"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git -c init.defaultBranch=main -C "$AG" init -q || fail "git init $AG"
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$AG")" \
    "$BASE/cli/projects/add-from-path?token=$DTOKEN")"
case "$reg" in *'"error"'*) fail "register $AG: $reg" ;; *"$AG"*) ;; *) fail "register $AG: $reg" ;; esac
resp="$(curl -s -X POST "$BASE/cli/workspace/ensure-pinned-chat?token=$DTOKEN" -H 'Content-Type: application/json' \
    --data-raw "$(python3 -c 'import json,sys; print(json.dumps({"project": sys.argv[1]}))' "$AG")")"
case "$resp" in *'"sessionId"'*) ;; *) fail "pinned chat: $resp" ;; esac
for _ in $(seq 1 400); do [ -s "$SB/claude.env" ] && break; sleep 0.05; done
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
SOCK="$(sed -n 's/^K2_HOOK_SOCK=//p' "$SB/claude.env" | head -1)"
[ -n "$PASSPORT" ] || fail "the agent's pinned chat got no passport"
for _ in $(seq 1 200); do [ -S "$SOCK" ] && break; sleep 0.05; done
[ -S "$SOCK" ] || fail "the agent's cell socket is missing: '$SOCK'"
ok "passport and cell socket"
AGENT_ENV=(K2_HOOK_TOKEN="$PASSPORT" K2_HOOK_SOCK="$SOCK" K2_PROJECT_PATH="$AG")
no_bare_403() { [[ "$OUT$ERR" != *"Invalid or missing auth token"* ]] || fail "$1: the bare 403 again: $OUT $ERR"; }

echo "== reads =="
cli "${AGENT_ENV[@]}" -- voice rooms --json
no_bare_403 "voice rooms"
want_rc 0 "voice rooms"
jok "$OUT" 'j["ok"] is True and "rooms" in j and j["canManage"] is False' "an agent reads the rooms (canManage false)"
cli "${AGENT_ENV[@]}" -- voice status --json
no_bare_403 "voice status"
want_rc 0 "voice status"
jok "$OUT" 'j["ok"] is True and "hosting" in j' "an agent reads the status"
cli "${AGENT_ENV[@]}" -- voice rooms
want_rc 0 "voice rooms (summary)"
has "$OUT" "Voice rooms on" "the summary prints"

echo "== refusals name the reason =="
refused() {
    no_bare_403 "$2"
    want_rc 3 "$2"
    has "$ERR" "$1" "$2 names $1"
}
cli "${AGENT_ENV[@]}" -- voice join 1
refused agents_not_in_voice "voice join 1"
cli "${AGENT_ENV[@]}" -- voice set rooms 2
refused owner_or_admin_only "voice set rooms 2"
cli "${AGENT_ENV[@]}" -- voice rename 1 Standup
refused owner_or_admin_only "voice rename"
cli "${AGENT_ENV[@]}" -- voice mute 1 owner
refused agents_cannot_moderate "voice mute"
cli "${AGENT_ENV[@]}" -- voice leave --call vc_x
refused agents_not_in_voice "voice leave"

echo "== the owner's view is unchanged =="
code="$(curl -s -o "$WORK/r.out" -w '%{http_code}' "$BASE/cli/voice/rooms?token=$DTOKEN")"
[ "$code" = 200 ] || fail "owner rooms: $code $(cat "$WORK/r.out")"
jok "$(cat "$WORK/r.out")" 'j["canManage"] is True' "the owner token still manages"

echo "voice agent headless: $pass passed, 0 failed"
