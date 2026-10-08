#!/usr/bin/env bash
# k2 workspace swap-canonical (PR #74, Sterling Long).
#
#   1. No daemon: usage errors exit 2; a missing notes file exits 1; inside
#      a K2 terminal with no passport the command refuses (exit 3) instead
#      of falling back to the owner token on disk.
#   2. A real headless daemon (temp HOME, claude + grok shims; never a real
#      CLI):
#      - an agent passport cannot swap another workspace's pinned chat
#        (other_workspace, nothing written there);
#      - an agent swaps its own pinned chat claude → grok: new --session-id,
#        no --resume, the handoff reaches the new chat stamped as that
#        agent (never "owner"), the handoff file is 0600 and gitignored
#        under .k2/handoffs/, the saved row moves to grok;
#      - the owner swaps it back grok → claude, stamped "owner";
#      - GET is 405; an app-style bad token is 403; both swaps are audited.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-swap-cli-XXXXXX)"
DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.3
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# Never inherit a session's identity from the caller.
unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
    K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL K2_PORT K2_HOOK_TOKEN \
    K2_SESSION_ID K2_PANE_ID K2_TAB_ID K2_PRIMARY K2SO_PANE_ID K2SO_TAB_ID || true

# ── 1. no daemon ─────────────────────────────────────────────────────
echo "== usage (no daemon) =="
HOME1="$WORK/home1"
mkdir -p "$HOME1/.k2"
echo "owner-disk-token-swap" > "$HOME1/.k2/heartbeat.token"
nod() { env HOME="$HOME1" K2_PORT=1 "$K2_CLI" "$@"; }
capture() { set +e; out="$("$@" 2>&1)"; rc=$?; set -e; }

capture nod workspace swap-canonical
assert_eq "no args exit 2" "$rc" "2"
assert_contains "usage text" "$out" "k2 workspace swap-canonical <workspace> --agent <harness>"
capture nod workspace swap-canonical ws
assert_eq "no --agent exit 2" "$rc" "2"
capture nod workspace swap-canonical ws --agent claude --mode everything
assert_eq "bad mode exit 2" "$rc" "2"
capture nod workspace swap-canonical ws --agent claude --bogus
assert_eq "unknown flag exit 2" "$rc" "2"
capture nod workspace swap-canonical ws --agent claude --notes-file "$WORK/nope.md"
assert_eq "missing notes file exit 1" "$rc" "1"
assert_contains "missing notes file says so" "$out" "file not found"
# Inside a K2 terminal (hook socket set) with no passport: refuse, never
# fall back to the owner token on disk.
capture env HOME="$HOME1" K2_PORT=1 K2_HOOK_SOCK="$WORK/not-a-socket.sock" \
    "$K2_CLI" workspace swap-canonical ws --agent claude
assert_eq "in-cell without passport exit 3" "$rc" "3"
assert_contains "in-cell without passport says why" "$out" "requires session ID (passport)"

# ── 2. real headless daemon ──────────────────────────────────────────
echo "== real daemon =="
DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || { echo "FAIL: k2-daemon not built (cargo build -p k2-daemon)" >&2; exit 1; }

SB="$WORK/sb"
mkdir -p "$SB/.k2" "$SB/shim"
# Each shim records argv + env and copies whatever is typed into it.
for h in claude grok; do
    cat > "$SB/shim/$h" <<EOF
#!/bin/sh
printf '%s\n' "\$@" > "$SB/$h.argv"
env > "$SB/$h.env"
exec /bin/cat > "$SB/$h.stdin"
EOF
    chmod +x "$SB/shim/$h"
done
HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    "$DAEMON_BIN" >"$SB/daemon.log" 2>&1 &
DAEMON_PID=$!
disown "$DAEMON_PID" 2>/dev/null || true
for _ in $(seq 1 200); do
    [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ] && break
    sleep 0.1
done
[ -s "$SB/.k2/daemon.port" ] || { echo "FAIL: daemon never wrote daemon.port" >&2; tail -30 "$SB/daemon.log" >&2; exit 1; }
DPORT="$(cat "$SB/.k2/daemon.port")"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
phase=""
for _ in $(seq 1 300); do
    phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || { echo "FAIL: daemon never reached ready" >&2; exit 1; }

enc() { python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
WSA="$SB/swapa"
WSB="$SB/swapb"
for ws in "$WSA" "$WSB"; do
    resp="$(curl -s -X POST "http://127.0.0.1:$DPORT/cli/workspace/create?token=$DTOKEN&path=$(enc "$ws")" --data-raw '')"
    assert_absent "workspace $(basename "$ws") registered" "$resp" '"error"'
done
# Default harness claude: the pinned chat comes up on the claude shim.
resp="$(curl -s -X POST "http://127.0.0.1:$DPORT/cli/workspace/ensure-pinned-chat?token=$DTOKEN" \
    -H 'Content-Type: application/json' --data-raw "{\"project\":\"$WSA\"}")"
assert_contains "pinned chat spawned" "$resp" '"sessionId"'
for _ in $(seq 1 100); do [ -s "$SB/claude.env" ] && break; sleep 0.05; done
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
[ -n "$PASSPORT" ] || { echo "FAIL: the pinned chat got no passport" >&2; exit 1; }
OLD_CLAUDE_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("claudeSessionId",""))' "$resp")"
[ -n "$OLD_CLAUDE_ID" ] || { echo "FAIL: no claude session id in $resp" >&2; exit 1; }

GET_CODE="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DPORT/cli/workspace/swap-canonical?token=$DTOKEN")"
assert_eq "GET is 405" "$GET_CODE" "405"
BAD_CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$DPORT/cli/workspace/swap-canonical?token=k2skn_not-a-real-pass" \
    -H 'Content-Type: application/json' --data-raw "{\"project\":\"$WSA\",\"provider\":\"grok\"}")"
assert_eq "app pass is 403" "$BAD_CODE" "403"

# As the swapa agent (its passport, inside a K2 terminal).
agent() {
    env HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" K2_HOOK_TOKEN="$PASSPORT" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WSA" "$K2_CLI" "$@"
}
owner() {
    env HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" K2_PROJECT_PATH="$WSA" "$K2_CLI" "$@"
}

capture agent workspace swap-canonical swapb --agent grok
assert_eq "agent cannot swap another workspace (exit 1)" "$rc" "1"
assert_contains "refusal names the rule" "$out" "only its own workspace"
[ ! -e "$WSB/.k2/handoffs" ] && ok "nothing written into the other workspace" || bad "handoff written into swapb"
[ ! -e "$SB/grok.argv" ] && ok "no grok spawned for the refused swap" || bad "grok spawned for a refused swap"

MARK1="AGENT-NOTES-$$-$RANDOM"
printf 'Keep going on the release.\n%s\n' "$MARK1" > "$WORK/notes1.md"
capture agent workspace swap-canonical swapa --agent grok --notes-file "$WORK/notes1.md"
assert_eq "agent swaps its own chat (exit 0)" "$rc" "0"
[ "$rc" = "0" ] || echo "$out" >&2
assert_contains "says swapped to grok" "$out" "Swapped the canonical chat to grok."
assert_contains "previous harness" "$out" "Previous harness: claude"
assert_contains "handoff delivered" "$out" "The handoff was sent into the new chat."
for _ in $(seq 1 100); do [ -s "$SB/grok.argv" ] && break; sleep 0.05; done
GROK_ARGV="$(cat "$SB/grok.argv" 2>/dev/null || true)"
assert_contains "grok premints a session id" "$GROK_ARGV" "--session-id"
assert_absent "grok does not resume" "$GROK_ARGV" "--resume"
assert_absent "grok argv never names the old chat" "$GROK_ARGV" "$OLD_CLAUDE_ID"
for _ in $(seq 1 100); do grep -Fq "$MARK1" "$SB/grok.stdin" 2>/dev/null && break; sleep 0.05; done
GROK_IN="$(cat "$SB/grok.stdin" 2>/dev/null || true)"
assert_contains "notes reached the new chat" "$GROK_IN" "$MARK1"
assert_contains "stamped as the agent" "$GROK_IN" "[from swapa]"
assert_absent "never stamped as the owner" "$GROK_IN" "[from owner]"
HANDOFF="$WSA/.k2/handoffs/CANONICAL-HANDOFF.md"
[ -f "$HANDOFF" ] && ok "handoff file under .k2/handoffs" || bad "no handoff at $HANDOFF"
mode="$(stat -f %Lp "$HANDOFF" 2>/dev/null || stat -c %a "$HANDOFF")"
assert_eq "handoff is 0600" "$mode" "600"
assert_eq "handoff dir is gitignored" "$(cat "$WSA/.k2/handoffs/.gitignore")" "*"
assert_contains "handoff holds the notes" "$(cat "$HANDOFF")" "$MARK1"
[ ! -e "$WSA/.k2/agent/CANONICAL-HANDOFF.md" ] && ok "nothing next to ROLE.md" || bad "handoff in .k2/agent"

# The daemon owns the new identity: the saved row is grok now.
args="$(curl -s "http://127.0.0.1:$DPORT/cli/workspace/resume-chat-args?token=$DTOKEN&project=$(enc "$WSA")")"
assert_contains "resume args now point at grok" "$args" '"provider":"grok"'

MARK2="OWNER-NOTES-$$-$RANDOM"
printf '%s\n' "$MARK2" > "$WORK/notes2.md"
rm -f "$SB/claude.argv" "$SB/claude.stdin"
capture owner workspace swap-canonical swapa --agent claude --full --notes-file "$WORK/notes2.md"
assert_eq "owner swaps back (exit 0)" "$rc" "0"
[ "$rc" = "0" ] || echo "$out" >&2
assert_contains "says swapped to claude" "$out" "Swapped the canonical chat to claude."
for _ in $(seq 1 100); do grep -Fq "$MARK2" "$SB/claude.stdin" 2>/dev/null && break; sleep 0.05; done
CLAUDE_ARGV="$(cat "$SB/claude.argv" 2>/dev/null || true)"
assert_contains "claude keeps its pinned flag" "$CLAUDE_ARGV" "--dangerously-skip-permissions"
assert_contains "claude premints a session id" "$CLAUDE_ARGV" "--session-id"
assert_absent "claude does not resume" "$CLAUDE_ARGV" "--resume"
assert_absent "claude argv never names the first chat" "$CLAUDE_ARGV" "$OLD_CLAUDE_ID"
CLAUDE_IN="$(cat "$SB/claude.stdin" 2>/dev/null || true)"
assert_contains "owner notes reached the chat" "$CLAUDE_IN" "$MARK2"
assert_contains "owner swap stamped as owner" "$CLAUDE_IN" "[from owner]"

AUDIT="$(cat "$SB/.k2/auth-audit.jsonl" 2>/dev/null || true)"
assert_contains "audit: refused agent swap" "$AUDIT" "refused:other_workspace"
assert_contains "audit: agent swap" "$AUDIT" '"swapped:grok"'
assert_contains "audit: owner swap" "$AUDIT" '"swapped:claude"'
assert_absent "notes never in the daemon log" "$(cat "$SB/daemon.log")" "$MARK1"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
