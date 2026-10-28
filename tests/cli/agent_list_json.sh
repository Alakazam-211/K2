#!/usr/bin/env bash
# 0.46.6 fleet-roll papercuts: scripts must never parse the agent or
# workspace tables (a path with a space split the columns and an agent was
# skipped), `k2 workspace list --json` printed the table, and
# `k2 agent restart` must take a full workspace path.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped; agent CLIs
# are `exec cat` shims. Names and paths carry spaces ('Skin QA',
# '<home>/AI Projects/Apop').
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-listjson-shim-XXXXXX)"
for cli in claude codex grok gemini cursor-agent pi; do
    printf '#!/bin/sh\ncase "$1" in --version|-v|version) echo "0.0.0-shim"; exit 0 ;; esac\nexec cat\n' >"$SHIM_DIR/$cli"
    chmod +x "$SHIM_DIR/$cli"
done
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_PORT="$K2SO_PORT" "$K2_CLI" "$@"
}
pass=0
fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail" >&2; tail -30 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

SPACED="$SANDBOX_HOME/AI Projects/Apop"
PLAIN="$SANDBOX_HOME/work/plain-$RANDOM$RANDOM"
mkdir -p "$SANDBOX_HOME/AI Projects" "$SANDBOX_HOME/work"
k2 agent hire "$SPACED" --name "Skin QA" --json >"$SANDBOX_HOME/hire1.json" || fail "hire spaced: $(cat "$SANDBOX_HOME/hire1.json")"
k2 agent hire "$PLAIN" --name "Plain" --json >"$SANDBOX_HOME/hire2.json" || fail "hire plain: $(cat "$SANDBOX_HOME/hire2.json")"

echo "== agent list --json =="
out="$(k2 agent list --json)" || fail "agent list --json: $out"
row="$(jq -c --arg p "$SPACED" '.agents[] | select(.path == $p)' <<<"$out")"
[ -n "$row" ] || fail "no row for the spaced path: $out"
[ "$(jq -r .name <<<"$row")" = "Skin QA" ] || fail "name: $row"
for k in id handle mode enabled live sessionId backend ptydAdopted path; do
    jq -e --arg k "$k" 'has($k)' <<<"$row" >/dev/null || fail "$k missing: $row"
done
[ "$(jq -r .id <<<"$row")" != null ] || fail "id: $row"
[ "$(jq -r .live <<<"$row")" = false ] || fail "not live yet: $row"
ok "agent list --json: spaced name and path intact, all fields"

echo "== workspace list --json =="
wout="$(k2 workspace list --json)" || fail "workspace list --json: $wout"
jq -e '.ok == true and (.workspaces | type == "array")' <<<"$wout" >/dev/null || fail "not JSON: $wout"
[ "$(jq -r --arg p "$SPACED" '[.workspaces[] | select(.path == $p)] | length' <<<"$wout")" = 1 ] || fail "spaced row: $wout"
[ "$(jq -r --arg p "$PLAIN" '[.workspaces[] | select(.path == $p)] | length' <<<"$wout")" = 1 ] || fail "plain row: $wout"
table="$(k2 workspace list)" || fail "table: $table"
grep -Fq "$SPACED" <<<"$table" || fail "the table still lists it: $table"
if jq . <<<"$table" >/dev/null 2>&1; then fail "the table is not JSON"; fi
ok "workspace list --json is JSON; the table still prints"

echo "== agent restart by full path =="
r1="$(k2 agent restart "$SPACED" --json)" || fail "restart by spaced path: $r1"
sid="$(jq -r .sessionId <<<"$r1")"
[ -n "$sid" ] && [ "$sid" != null ] || fail "sessionId: $r1"
r2="$(k2 agent restart "$SPACED/" --json)" || fail "restart with a trailing slash: $r2"
sid2="$(jq -r .sessionId <<<"$r2")"
row="$(k2 agent list --json | jq -c --arg p "$SPACED" '.agents[] | select(.path == $p)')"
[ "$(jq -r .live <<<"$row")" = true ] || fail "live after restart: $row"
[ "$(jq -r .sessionId <<<"$row")" = "$sid2" ] || fail "list sessionId $(jq -r .sessionId <<<"$row") want $sid2"
[ "$(jq -r .backend <<<"$row")" != null ] || fail "backend: $row"
[ "$(jq -r .ptydAdopted <<<"$row")" = false ] || fail "spawned here, not adopted: $row"
ok "restart takes a full path (with spaces, with a trailing slash); list shows its session"

echo "== a miss never suggests the same name =="
set +e; miss="$(k2 agent restart "Skin QA Nope" 2>&1)"; rc=$?; set -e
[ "$rc" = 4 ] || fail "unknown name exit $rc: $miss"
if grep -Fq "did you mean 'Skin QA Nope'" <<<"$miss"; then fail "self-suggestion: $miss"; fi
ok "unknown name exit 4, no self-suggestion"

echo ""
echo "Results: $pass passed, 0 failed"
