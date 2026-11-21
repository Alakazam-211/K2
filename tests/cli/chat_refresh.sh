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
#      sidecar's passport is refused either way. The codex sidecar asks as
#      the k2 CLI does from a Codex tool call: its conversation in
#      X-K2-Conversation and X-K2-Thread, in curl's config, never in argv,
#      never the owner token; without the conversation K2 refuses its
#      passport (conversation_missing);
#   5. a person's call, the older door, GET is 405.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-refresh-shim-XXXXXX)"
# The Codex release K2 checked last (the daemon reads a Codex tab's version
# before it lets that tab's passport through).
CODEX_CHECKED="$(sed -n 's/^ *last: (\([0-9]*\), \([0-9]*\), \([0-9]*\)),$/\1.\2.\3/p' \
    "$ROOT/crates/k2-core/src/terminal/codex_version.rs" | head -1)"
[ -n "$CODEX_CHECKED" ] || { echo "FAIL: no checked Codex release in codex_version.rs" >&2; exit 1; }
# Turns bracketed paste on, as a real TUI does when it's ready, and appends
# what it reads, so the test can see what reached the new chat. The codex
# one answers as a Codex K2 checked: --help lists --no-daemon (so K2 starts
# it with its own app-server and marks the tab) and --version names that
# release.
for cli in claude codex; do
    if [ "$cli" = codex ]; then
        own="--help) printf '      --no-daemon\\n'; exit 0 ;; --version) echo 'codex-cli $CODEX_CHECKED'; exit 0 ;;"
    else
        own=""
    fi
    cat >"$SHIM_DIR/$cli" <<EOF
#!/bin/sh
case "\$1" in $own --version|-v|version|--help|app-server) echo "0.0.0-shim"; exit 0 ;; esac
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
# A curl that keeps each call's argv and the config it read (curl -K),
# then runs the real one. Only as_codex puts it first on PATH.
CURL_LOGGER_DIR="$SHIM_DIR/curl-logger"
CURL_LOG="$SHIM_DIR/curl-log"
mkdir -p "$CURL_LOGGER_DIR" "$CURL_LOG"
REAL_CURL="$(command -v curl)"
cat >"$CURL_LOGGER_DIR/curl" <<EOF
#!/usr/bin/env bash
n="\$(mktemp "$CURL_LOG/call.XXXXXX")"
printf '%s\n' "\$*" >"\$n.argv"
if [ "\${1:-}" = -K ] && [ "\${2:-}" = - ]; then
    cfg="\$(cat)"
    printf '%s\n' "\$cfg" >"\$n.cfg"
    printf '%s' "\$cfg" | "$REAL_CURL" "\$@"
    exit \$?
fi
if [ "\${1:-}" = -K ]; then cat "\$2" >"\$n.cfg"; else : >"\$n.cfg"; fi
exec "$REAL_CURL" "\$@"
EOF
chmod +x "$CURL_LOGGER_DIR/curl"
# A k2 call from a Codex tool call in the codex sidecar's tab: the tab's
# env (its passport and the marker K2 set) plus what Codex sets on every
# tool call, CODEX_SESSION_ID (the conversation's root) and CODEX_THREAD_ID
# (a top-level conversation's root is its own thread).
CODEX_CONV=""
CODEX_MARK=""
as_codex() {
    env -i HOME="$SANDBOX_HOME" PATH="$CURL_LOGGER_DIR:$PATH" K2_AIRGAP=1 K2_HOST=127.0.0.1 \
        K2_PORT="$K2SO_PORT" K2_HOOK_TOKEN="$PASSPORT" \
        K2_HOOK_SOCK="$SHIM_DIR/not-a-socket.sock" K2_PROJECT_PATH="$A" \
        K2_CODEX_TAB_ISOLATED="$CODEX_MARK" CODEX_SESSION_ID="$CODEX_CONV" CODEX_THREAD_ID="$CODEX_CONV" \
        "$K2_CLI" "$@"
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
k2 agent restart "$A" --harness claude --json >/dev/null || fail "start A's pinned chat"
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
has "$out" "Let this agent refresh its pinned chat"
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
# K2 started the sidecar's Codex with its own app-server and marked the
# tab with its passport's session, so the k2 CLI keeps the passport there.
has "$(last_argv codex)" "--no-daemon"
CODEX_MARK="$(sed -n 's/^K2_CODEX_TAB_ISOLATED=//p' "$SHIM_DIR/codex.env" | head -1)"
[ "$CODEX_MARK" = "${sidecar_passport%%.*}" ] \
    || fail "the sidecar's tab marker is '$CODEX_MARK', want ${sidecar_passport%%.*}"
# Codex starts its conversation: the rollout log it writes, whose header
# names the conversation and the folder. K2 learns the sidecar's
# conversation from it.
codex_home="$(sed -n 's/^CODEX_HOME=//p' "$SHIM_DIR/codex.env" | head -1)"
[ -n "$codex_home" ] || codex_home="$SANDBOX_HOME/.codex"
CODEX_CONV="$(python3 -c 'import uuid; print(uuid.uuid4())')"
codex_day="$codex_home/sessions/$(date -u +%Y/%m/%d)"
mkdir -p "$codex_day"
codex_ts="$(date -u +%Y-%m-%dT%H:%M:%S.000Z)"
jq -cn --arg id "$CODEX_CONV" --arg ts "$codex_ts" --arg cwd "$A" \
    '{timestamp: $ts, type: "session_meta", payload: {id: $id, timestamp: $ts, cwd: $cwd, originator: "codex_cli_rs"}}' \
    >"$codex_day/rollout-$(date -u +%Y-%m-%dT%H-%M-%S)-$CODEX_CONV.jsonl"
# Until K2 has adopted it, the call names a conversation K2 doesn't know.
for i in $(seq 1 150); do
    run as_codex agent refresh
    case "$out" in *unknown_conversation*|*conversation_unsettled*) sleep 0.2 ;; *) break ;; esac
done
# Without its conversation, the sidecar's passport is refused.
run as_agent agent refresh
[ "$rc" = 3 ] || fail "sidecar refresh with no conversation exit $rc: $out"
has "$out" "conversation_missing"
rm -f "$CURL_LOG"/*
run as_codex agent refresh
[ "$rc" = 3 ] || fail "sidecar refresh exit $rc: $out"
has "$out" "chat_refresh_main_only"
run as_codex permission request chat.refresh --why "no" --json
[ "$rc" != 0 ] || fail "a sidecar asked: $out"
has "$out" "chat_refresh_main_only"
# Every request carried the conversation in curl's config: never in argv,
# never the owner token.
calls=0
for f in "$CURL_LOG"/*.argv; do
    [ -f "$f" ] || continue
    calls=$((calls + 1))
    cfg="$(cat "${f%.argv}.cfg")"
    argv="$(cat "$f")"
    has "$cfg" "X-K2-Conversation: $CODEX_CONV"
    has "$cfg" "X-K2-Thread: $CODEX_CONV"
    hasnt "$cfg" "$K2SO_TOKEN"
    hasnt "$argv" "$CODEX_CONV"
    hasnt "$argv" "X-K2-"
    hasnt "$argv" "$sidecar_passport"
    hasnt "$argv" "$K2SO_TOKEN"
done
[ "$calls" -ge 2 ] || fail "the sidecar's calls ran curl $calls time(s)"
has "$(cat "$CURL_LOG"/*.cfg)" "/cli/agent/refresh"
has "$(cat "$CURL_LOG"/*.cfg)" "/cli/permission/request"
ok "the codex sidecar's calls name its conversation in curl's config, never argv or the owner token"
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
