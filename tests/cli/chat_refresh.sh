#!/usr/bin/env bash
# k2 agent refresh (0.46.8), headless: a real k2-daemon (temp HOME) with
# `exec cat` CLI shims, so nothing real runs.
#   1. help, usage;
#   2. an agent with the switch off: exit 3, nothing spawned; cards off, no
#      "Ask with:" line;
#   3. cards on: the refusal ends with the chat.refresh line carrying the
#      note's hash; the agent asks, the owner allows: a new chat (no --resume),
#      the same launch options, the note in the handoff, the Thread at the
#      address, the sidecar still live;
#   4. "from now on" through the switch: the agent's own call just runs; a
#      sidecar's passport is refused either way;
#   5. a person's call, the older door, GET is 405.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-refresh-shim-XXXXXX)"
# Turns bracketed paste on, as a real TUI does when it's ready, and appends
# what it reads, so the test can see what reached the new chat.
for cli in claude codex; do
    cat >"$SHIM_DIR/$cli" <<EOF
#!/bin/sh
case "\$1" in --version|-v|version|--help|app-server) echo "0.0.0-shim"; exit 0 ;; esac
printf '%s\n' "\$@" >>"$SHIM_DIR/$cli.argv"
echo "--" >>"$SHIM_DIR/$cli.argv"
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
wait_passport() {
    local cli="$1" old="$2" i tok
    for i in $(seq 1 100); do
        tok="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SHIM_DIR/$cli.env" 2>/dev/null | head -1 || true)"
        if [ -n "$tok" ] && [ "$tok" != "$old" ]; then PASSPORT="$tok"; return 0; fi
        sleep 0.1
    done
    fail "no new passport for $cli"
}
last_argv() { awk 'BEGIN{RS="--\n"} {last=$0} END{print last}' "$SHIM_DIR/$1.argv"; }
pin() { curl -s "http://127.0.0.1:$K2SO_PORT/cli/workspace/resume-chat-args?token=$K2SO_TOKEN&project=$(jq -rn --arg p "$A" '$p|@uri')"; }

echo "== help and usage =="
help="$(k2 agent --help)"
has "$help" "refresh [<name>]"
rhelp="$(k2 agent refresh --help)"
has "$rhelp" "same harness"
has "$rhelp" "k2 permission request chat.refresh"
has "$rhelp" "k2 study chat-refresh"
run k2 agent refresh; [ "$rc" = 2 ] || fail "no workspace, no passport: exit $rc: $out"
run k2 agent refresh ws --bogus; [ "$rc" = 2 ] || fail "bad flag exit $rc: $out"
run k2 agent refresh ws --note a --notes-file b; [ "$rc" = 2 ] || fail "two notes exit $rc: $out"
run k2 agent refresh ws --notes-file /no/such/file; [ "$rc" = 1 ] || fail "missing notes file exit $rc: $out"
study="$(k2 study chat-refresh)"
has "$study" "k2 study chat-refresh — refreshing your own chat"
has "$study" "chat_refresh_needs_person"
ok "help, usage, study page"

A="$SANDBOX_HOME/work/refresh-a-$RANDOM$RANDOM"
mkdir -p "$SANDBOX_HOME/work"
k2 agent hire "$A" --name "Desk" --json >"$SANDBOX_HOME/hire-a.json" || fail "hire A: $(cat "$SANDBOX_HOME/hire-a.json")"
k2 agent restart "$A" --json >/dev/null || fail "start A's pinned chat"
wait_starts claude 1
wait_passport claude ""
old_id="$(pin | jq -r .resumeSession)"
[ -n "$old_id" ] && [ "$old_id" != null ] || fail "no pinned id: $(pin)"
run k2 thread "$A" "THREAD-BEFORE-REFRESH"
[ "$rc" = 0 ] || fail "thread post exit $rc: $out"
run k2 thread "$A"
has "$out" "THREAD-BEFORE-REFRESH"

echo "== an agent with the switch off =="
run as_agent agent refresh --note "the note"
[ "$rc" = 3 ] || fail "agent refresh exit $rc: $out"
has "$out" "chat_refresh_needs_person"
has "$out" "Can refresh its own pinned chat"
hasnt "$out" "Ask with:"
[ "$(starts claude)" = 1 ] || fail "a refused refresh spawned"
run as_agent workspace swap-canonical "$A" --agent claude
[ "$rc" = 3 ] || fail "agent swap-canonical (same harness) exit $rc: $out"
has "$out" "Refreshing ends this chat"
[ "$(starts claude)" = 1 ] || fail "a refused swap spawned"
ok "refused (exit 3) by both doors, nothing spawned, no card line while cards are off"

echo "== with permission cards on: ask, allow, refreshed =="
k2 permission beta on >/dev/null || fail "beta on"
k2 permission agent "$A" on >/dev/null || fail "agent switch on"
# A dry run says a person is needed and hands out no line to ask with.
run as_agent agent refresh --dry-run --note "ignored"
[ "$rc" = 0 ] || fail "agent dry run (switch off) exit $rc: $out"
has "$out" "a person has to allow this"
hasnt "$out" "Ask with:"
[ "$(starts claude)" = 1 ] || fail "the dry run spawned"
printf 'NOTE-LINE-ONE\nNOTE-LINE-TWO\n' >"$SANDBOX_HOME/note.md"
run as_agent agent refresh --notes-file "$SANDBOX_HOME/note.md"
[ "$rc" = 3 ] || fail "agent refresh (cards on) exit $rc: $out"
ask_line="$(grep -F 'Ask with: k2 permission request chat.refresh notes=' <<<"$out")" || fail "no Ask line: $out"
hash="$(sed -E 's/.*notes=([0-9a-f]{64}).*/\1/' <<<"$ask_line")"
[ "${#hash}" = 64 ] || fail "no hash in: $ask_line"
run as_agent permission request chat.refresh "notes=$hash" --why "My context is full" --json
[ "$rc" = 0 ] || fail "request exit $rc: $out"
rid="$(jq -r .id <<<"$out")"
[ -n "$rid" ] && [ "$rid" != null ] || fail "request id: $out"
[ "$(starts claude)" = 1 ] || fail "asking spawned"
run k2 permission approve "$rid"
[ "$rc" = 0 ] || fail "approve exit $rc: $out"
wait_starts claude 2
argv="$(last_argv claude)"
has "$argv" "--session-id"
hasnt "$argv" "--resume"
new_id="$(pin | jq -r .resumeSession)"
[ "$new_id" != "$old_id" ] || fail "the pin didn't move: $new_id"
hasnt "$argv" "$old_id"
handoff="$A/.k2/handoffs/CANONICAL-HANDOFF.md"
[ -f "$handoff" ] || fail "no handoff at $handoff"
has "$(cat "$handoff")" "NOTE-LINE-ONE"
has "$(cat "$handoff")" "NOTE-LINE-TWO"
has "$(cat "$handoff")" "$old_id"
run k2 thread "$A"
has "$out" "THREAD-BEFORE-REFRESH"
ok "the owner's Allow started a new chat: note handed over, Thread at the address"

echo "== the note reached the new chat, after its CLI was ready =="
for i in $(seq 1 100); do
    grep -Fq "NOTE-LINE-TWO" "$SHIM_DIR/claude.stdin" 2>/dev/null && break
    sleep 0.2
done
grep -Fq "NOTE-LINE-TWO" "$SHIM_DIR/claude.stdin" || fail "the note never reached the new chat's input"
ok "note typed into the new chat"

echo "== the switch =="
wait_passport claude "$PASSPORT"
run k2 agent access set "$A" chat_refresh on
[ "$rc" = 0 ] || fail "switch on exit $rc: $out"
has "$out" "Can refresh its own pinned chat is on"
# A passport can't flip it.
code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$K2SO_PORT/cli/agent-access/set?token=$PASSPORT" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg w "$A" '{workspace:$w, toggle:"chat_refresh", value:false}')")"
[ "$code" = 403 ] || fail "a passport flipped the switch: $code"
# A person's project list shows it for the Agent tab.
[ "$(curl -s "http://127.0.0.1:$K2SO_PORT/cli/projects/list?token=$K2SO_TOKEN" | jq -r --arg p "$A" '[.. | objects | select(.path? == $p)][0].agentsCanRefreshChat')" = 1 ] \
    || fail "the project list doesn't show the switch"
run as_agent agent refresh --dry-run --json
[ "$rc" = 0 ] || fail "agent dry run exit $rc: $out"
[ "$(jq -r .refreshed <<<"$out")" = false ] && [ "$(jq -r .dryRun <<<"$out")" = true ] || fail "dry run: $out"
[ "$(starts claude)" = 2 ] || fail "the dry run spawned"
run as_agent agent refresh --note "FROM-THE-AGENT" --json
[ "$rc" = 0 ] || fail "agent refresh (switch on) exit $rc: $out"
[ "$(jq -r .refreshed <<<"$out")" = true ] || fail "refresh answer: $out"
wait_starts claude 3
has "$(cat "$handoff")" "FROM-THE-AGENT"
run k2 thread "$A"
has "$out" "THREAD-BEFORE-REFRESH"
ok "with the switch on, the agent's own main session just runs"

echo "== a sidecar's passport =="
wait_passport claude "$PASSPORT"
main_passport="$PASSPORT"
curl -s -X POST "http://127.0.0.1:$K2SO_PORT/cli/agent-access/set?token=$K2SO_TOKEN" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg w "$A" '{workspace:$w, toggle:"agents_manage", value:true}')" >/dev/null
run as_agent sidecar new helper --harness codex --brief-text "wait"
[ "$rc" = 0 ] || fail "sidecar new exit $rc: $out"
wait_starts codex 1
wait_passport codex ""
sidecar_passport="$PASSPORT"
run as_agent agent refresh
[ "$rc" = 3 ] || fail "sidecar refresh exit $rc: $out"
has "$out" "chat_refresh_main_only"
run as_agent permission request chat.refresh --why "no" --json
[ "$rc" != 0 ] || fail "a sidecar asked: $out"
has "$out" "chat_refresh_main_only"
PASSPORT="$main_passport"
run as_agent agent refresh --json
[ "$rc" = 0 ] || fail "main refresh with a sidecar up exit $rc: $out"
wait_starts claude 4
list="$(curl -s "http://127.0.0.1:$K2SO_PORT/cli/sidecar/list?token=$K2SO_TOKEN&workspace=$(jq -rn --arg p "$A" '$p|@uri')")"
[ "$(jq -r '.sidecars[] | select(.harness == "codex") | .state' <<<"$list")" = live ] || fail "the sidecar stopped: $list"
[ "$(starts codex)" = 1 ] || fail "the sidecar restarted"
ok "a sidecar can't refresh or ask, and a main-session refresh leaves it running"

echo "== a person, and the route =="
wait_passport claude "$main_passport"
run k2 agent refresh "$A" --note "FROM-A-PERSON"
[ "$rc" = 0 ] || fail "owner refresh exit $rc: $out"
has "$out" "Refreshed"
has "$out" "Old chat:"
wait_starts claude 5
has "$(cat "$handoff")" "FROM-A-PERSON"
code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$K2SO_PORT/cli/agent/refresh?token=$K2SO_TOKEN")"
[ "$code" = 405 ] || fail "GET /cli/agent/refresh answered $code"
code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$K2SO_PORT/cli/agent/refresh?token=not-a-token" -d '{}')"
[ "$code" = 403 ] || [ "$code" = 401 ] || fail "a bad token answered $code"
AUDIT="$(cat "$SANDBOX_HOME/.k2/auth-audit.jsonl" 2>/dev/null || cat "$SANDBOX_HOME/.k2so/auth-audit.jsonl")"
has "$AUDIT" '"chat-refresh"'
has "$AUDIT" "refreshed:claude"
ok "a person refreshes any time; GET 405; a bad token refused; audited"

echo ""
echo "Results: $pass passed, 0 failed"
