#!/usr/bin/env bash
# Headless smoke (0.46.3): failed agent starts never move the pinned Chat.
# Builds this checkout's k2-daemon, runs it from `env -i` in a temp HOME
# (every K2_*/K2SO_* dropped, never the real ~/.k2, keychain or running
# daemon) with an empty agent shim dir: no `claude` anywhere, like a box
# without the harness. Curl only. From anywhere in the repo:
#   scripts/test/spawn-fail-keeps-pin-smoke.sh      (KEEP=1 keeps the temp dir)
# Fails loudly at the first broken check; exit 0 only on PASS.
set -euo pipefail
for t in curl jq cargo; do command -v "$t" >/dev/null || { echo "[smoke] FAIL: needs $t" >&2; exit 1; }; done
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
(cd "$REPO" && cargo build -q -p k2-daemon --bin k2-daemon) || { echo "[smoke] FAIL: build" >&2; exit 1; }
# The build's own target dir (CARGO_TARGET_DIR on a compute node).
BIN="$(cd "$REPO" && cargo metadata -q --format-version 1 --no-deps | jq -r .target_directory)/debug/k2-daemon"
[[ -x "$BIN" ]] || { echo "[smoke] FAIL: no daemon binary at $BIN" >&2; exit 1; }
CLI="$REPO/cli/k2"
ROOT="$(mktemp -d /tmp/k2sf-XXXX)"
H="$ROOT/h"
PATH_CLEAN="/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin"
mkdir -p "$H/.k2" "$H/tmp" "$H/run" "$H/agent-shim-empty" "$H/beta"
chmod 700 "$H/tmp" "$H/run"
say() { printf '[smoke] %s\n' "$*" >&2; }
die() { say "FAIL: $*"; say "daemon.log (last 40 lines):"; tail -40 "$H/daemon.log" >&2 || true; exit 1; }
PID=""
cleanup() { [[ -n "$PID" ]] && kill "$PID" 2>/dev/null || true; sleep 0.3; [[ -n "$PID" ]] && kill -9 "$PID" 2>/dev/null || true; [[ "${KEEP:-0}" == 1 ]] || rm -rf "$ROOT"; }
trap cleanup EXIT

env -i HOME="$H" PATH="$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 SHELL=/bin/sh TMPDIR="$H/tmp" XDG_RUNTIME_DIR="$H/run" \
  K2_TEST_AGENT_SHIM_DIR="$H/agent-shim-empty" K2SO_WATCHDOG_DISABLED=1 K2_HEARTBEAT_NO_SELF_HEAL=1 \
  K2_SUBSCRIPTION_PROBE=deny K2_AIRGAP=1 K2_TUNNEL_LEASE=off K2_HOOK_INSTALL=0 \
  K2_LLM_ACCOUNTS_KEYCHAIN=0 K2_LLM_ACCOUNTS_LOOP=0 K2_USAGE_PRICES_FETCH=0 \
  "$BIN" >>"$H/daemon.log" 2>&1 &
PID=$!
for _ in $(seq 1 300); do
  if [[ -s "$H/.k2/daemon.token" && -s "$H/.k2/daemon.port" ]] &&
     curl -fsS "http://127.0.0.1:$(cat "$H/.k2/daemon.port")/boot-status" 2>/dev/null | jq -e '.phase=="ready"' >/dev/null; then break; fi
  kill -0 "$PID" 2>/dev/null || die "daemon exited"
  sleep 0.2
done
PORT="$(cat "$H/.k2/daemon.port")"; TOK="$(cat "$H/.k2/daemon.token")"
U="http://127.0.0.1:$PORT"
say "daemon up on :$PORT"

api() { curl -sS -X "$1" "$U$2?token=$TOK${4:+&$4}" -H 'content-type: application/json' ${3:+--data "$3"}; }

WS="$(cd "$H/beta" && pwd -P)"
api POST /cli/projects/add-without-git "{\"path\":\"$WS\",\"seedWiki\":false,\"seedAgentsMd\":false}" >/dev/null
say "workspace $WS"
# An added folder is not an agent yet: make it one (`k2 settings --mode`).
r="$(api POST /cli/mode '' "project=$(jq -rn --arg v "$WS" '$v|@uri')&set=custom")"
[[ "$(jq -r '.success // .ok // empty' <<<"$r")" == true ]] || die "set agent mode: $r"
HANDLE="beta"

msg() { api POST /cli/workspace/msg '' "workspace=$HANDLE&text=$(jq -rn --arg v "$1" '$v|@uri')&from=smoke&wake=true&wake_timeout=2"; }
thread() { api GET /cli/thread '' "addr=$HANDLE&limit=0"; }

# First wake of a never-started agent: fails (no claude); it may pin once.
r="$(msg 'ping 1')"; say "msg 1: $r"
[[ "$(jq -r .reason <<<"$r")" == spawn_failed ]] || die "msg 1 should be spawn_failed: $r"
t="$(thread)"; CONV="$(jq -r .conversation_id <<<"$t")"
[[ -n "$CONV" && "$CONV" != null ]] || die "no pinned Chat after the first wake: $t"
say "pinned Chat: $CONV"

# Thread content: three lines and a card.
for i in 1 2 3; do api POST /cli/thread/post "{\"addr\":\"$HANDLE\",\"text\":\"line $i\"}" >/dev/null; done
api POST /cli/thread/ask "{\"addr\":\"$HANDLE\",\"prompt\":\"Ship it?\",\"options\":\"Ship,Wait\"}" >/dev/null
api POST /cli/thread/read "{\"addr\":\"$HANDLE\",\"seq\":2}" >/dev/null
BEFORE="$(thread | jq -c '[.conversation_id, [.items[] | [.id, .seq]]]')"
UNREAD_BEFORE="$(api GET /cli/thread/unread | jq -c --arg c "$CONV" '.byConversation[] | select(.id==$c)')"
say "before: $BEFORE"
[[ "$(jq '.[1]|length' <<<"$BEFORE")" == 4 ]] || die "expected 4 items: $BEFORE"

for n in 2 3 4; do
  r="$(msg "ping $n")"; say "msg $n: $(jq -c '{success,reason}' <<<"$r")"
  [[ "$(jq -r .reason <<<"$r")" == spawn_failed ]] || die "msg $n should be spawn_failed: $r"
  NOW="$(thread | jq -c '[.conversation_id, [.items[] | [.id, .seq]]]')"
  [[ "$NOW" == "$BEFORE" ]] || die "Thread changed after failed msg $n: $NOW (was $BEFORE)"
done
UNREAD_AFTER="$(api GET /cli/thread/unread | jq -c --arg c "$CONV" '.byConversation[] | select(.id==$c)')"
[[ "$UNREAD_AFTER" == "$UNREAD_BEFORE" ]] || die "read state changed: $UNREAD_AFTER vs $UNREAD_BEFORE"
say "unread row: $UNREAD_AFTER"
# The CLI view too (`k2 thread beta --json`).
CLI_CONV="$(env -i HOME="$H" PATH="$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 bash "$CLI" thread "$HANDLE" --json | jq -r '.conversation_id')"
[[ "$CLI_CONV" == "$CONV" ]] || die "k2 thread shows $CLI_CONV, pin is $CONV"
say "PASS: 3 failed wakes, same pinned Chat $CONV, 4 items, read state unchanged"
