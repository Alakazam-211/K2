#!/usr/bin/env bash
# GH #66: removing a workspace stops every session the daemon runs for it
# (pinned chat + sidecar tabs). Before the fix, removal only deleted DB
# rows, so agent processes kept running under k2-daemon with no workspace.
#
# Covers both removal doors: `k2 workspace remove` (/cli/workspace/remove)
# and the app's POST /cli/projects/delete. A session in a NESTED registered
# workspace must survive its parent's removal.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped. Agent CLIs
# are shims (`exec cat`) that log "<pid> <cwd> <args>", so nothing real runs.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-rm-shim-XXXXXX)"
LOG="$SHIM_DIR/starts.log"
: >"$LOG"
for cli in claude codex grok gemini cursor-agent pi; do
    printf '#!/bin/sh\ncase "$1" in --version|-v|version) echo "0.0.0-shim"; exit 0 ;; esac\necho "$$ $(pwd) $*" >>"%s"\nexec cat\n' "$LOG" >"$SHIM_DIR/$cli"
    chmod +x "$SHIM_DIR/$cli"
done
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
ALL_PIDS=""
trap 'for p in $ALL_PIDS; do kill "$p" 2>/dev/null || true; done; _sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" "$K2_CLI" "$@"
}
post() { # post <route> <json>
    curl -s -X POST -H 'Content-Type: application/json' --data "$2" \
        "http://127.0.0.1:${K2SO_PORT}$1?token=${K2SO_TOKEN}"
}

pass=0
fail() { echo "FAIL: $*" >&2; cat "$LOG" >&2; echo "--- daemon log tail" >&2; tail -30 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

# pid of the shim whose log line contains <needle>; waits up to 5s.
pid_for() {
    local needle="$1" i p
    for i in $(seq 1 50); do
        p="$(grep -F -- "$needle" "$LOG" | tail -1 | cut -d' ' -f1 || true)"
        [ -n "$p" ] && { echo "$p"; return 0; }
        sleep 0.1
    done
    fail "no shim start matching '$needle'"
}
alive() { kill -0 "$1" 2>/dev/null; }
wait_dead() {
    local i
    for i in $(seq 1 50); do alive "$1" || return 0; sleep 0.1; done
    return 1
}

# Registers <dir> as an agent workspace, starts its pinned chat and one
# sidecar tab. Prints "<project-id> <pinned-pid> <tab-pid>".
start_workspace() {
    local dir="$1" tag="$2" id resp pinned tab
    k2 agent hire "$dir" --name "$tag" --json >"$SANDBOX_HOME/hire-$tag.json" \
        || fail "hire $tag: $(cat "$SANDBOX_HOME/hire-$tag.json")"
    id="$(k2 agent conf "$dir" --json | jq -r .id)"
    [ -n "$id" ] && [ "$id" != null ] || fail "no project id for $dir"
    resp="$(post /cli/workspace/ensure-pinned-chat "$(jq -nc --arg p "$dir" '{project:$p}')")"
    [ "$(jq -r .sessionId <<<"$resp")" != null ] || fail "ensure-pinned-chat $tag: $resp"
    pinned="$(pid_for "$dir --")"
    resp="$(post /cli/sessions/v2/spawn "$(jq -nc --arg c "$dir" --arg t "tab-rm-$tag" \
        '{agent_name:$t, cwd:$c, command:"grok", args:["--k2-test-tab", $t]}')")"
    [ "$(jq -r .sessionId <<<"$resp")" != null ] || fail "v2 spawn tab $tag: $resp"
    tab="$(pid_for "--k2-test-tab tab-rm-$tag")"
    alive "$pinned" || fail "$tag pinned shim $pinned not running"
    alive "$tab" || fail "$tag tab shim $tab not running"
    ALL_PIDS="$ALL_PIDS $pinned $tab"
    echo "$id $pinned $tab"
}

WORK="$SANDBOX_HOME/work"
mkdir -p "$WORK"

echo "== k2 workspace remove stops the pinned chat and sidecar tabs =="
outer="$WORK/outer-$RANDOM"
read -r _ o_pin o_tab < <(start_workspace "$outer" outer)
inner="$outer/inner"
read -r _ i_pin i_tab < <(start_workspace "$inner" inner)
k2 workspace remove "$outer" >"$SANDBOX_HOME/rm.out" 2>&1 || fail "workspace remove: $(cat "$SANDBOX_HOME/rm.out")"
wait_dead "$o_pin" || fail "pinned chat $o_pin still running after workspace remove"
wait_dead "$o_tab" || fail "sidecar tab $o_tab still running after workspace remove"
ok "outer pinned $o_pin and tab $o_tab stopped"
alive "$i_pin" || fail "nested workspace pinned chat $i_pin was stopped"
alive "$i_tab" || fail "nested workspace tab $i_tab was stopped"
ok "nested workspace sessions $i_pin / $i_tab kept running"

echo "== POST /cli/projects/delete (the app's Remove) stops them too =="
read -r i_id _ _ < <(echo "$(k2 agent conf "$inner" --json | jq -r .id) x x")
resp="$(post /cli/projects/delete "$(jq -nc --arg id "$i_id" '{id:$id}')")"
jq -e '.error == null' <<<"$resp" >/dev/null || fail "projects/delete: $resp"
wait_dead "$i_pin" || fail "pinned chat $i_pin still running after projects/delete"
wait_dead "$i_tab" || fail "sidecar tab $i_tab still running after projects/delete"
ok "projects/delete stopped $i_pin and $i_tab"

echo "== k2 agent retire stops sidecar tabs, not only the pinned chat =="
gone="$WORK/retired-$RANDOM"
read -r _ r_pin r_tab < <(start_workspace "$gone" retired)
k2 agent retire "$gone" --force --json >"$SANDBOX_HOME/retire.json" 2>&1 \
    || fail "retire: $(cat "$SANDBOX_HOME/retire.json")"
wait_dead "$r_pin" || fail "pinned chat $r_pin still running after retire"
wait_dead "$r_tab" || fail "sidecar tab $r_tab still running after retire"
ok "retire stopped $r_pin and $r_tab"

echo ""
echo "Results: $pass passed, 0 failed"
