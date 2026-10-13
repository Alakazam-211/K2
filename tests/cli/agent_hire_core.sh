#!/usr/bin/env bash
# 0.45.6 agent hire (prd-agent-hire-templates-v1 "build A"): `k2 agent hire`
# runs its create/persona/register/db steps on the daemon's shared hire core
# (POST /cli/agents/hire); the people's verbs for recipe hires, team links,
# the hire root and the `k2 msg who` sender check work end to end.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped, no member
# launches, an empty agent shim dir so nothing real can spawn. No vault
# confirm here (that needs a person; the Rust tests cover the mint).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-hire-shim-XXXXXX)"
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT

WORK="$SANDBOX_HOME/work"
mkdir -p "$WORK"

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" K2_PROJECT_PATH="$WORK" "$K2_CLI" "$@"
}

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
step_status() { jq -r --arg s "$1" '[.actions[] | select(.step == $s) | .status] | first // ""'; }

echo "== tool policy: agent team/instances carry the passport =="
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
[ "$(_cli_tool_id_for_verb agent team)" = "agent-teams" ] || fail "agent team tool id"
[ "$(_cli_tool_id_for_verb agent instances)" = "agent-teams" ] || fail "agent instances tool id"
[ "$(_cli_tool_default_mode agent-teams)" = "id" ] || fail "agent-teams is an id tool"
_cli_tool_is_locked agent-teams || fail "agent-teams is locked"
[ "$(_cli_tool_id_for_verb agent hire)" = "" ] || fail "agent hire stays open until noun tiers S6"
[ "$(_cli_tool_id_for_verb agent)" = "" ] || fail "bare agent unchanged"
ok "sub-verb aware mapping (HT14)"

echo "== k2 agent hire runs on the daemon hire core =="
dir="$WORK/core-$RANDOM$RANDOM"
out="$(k2 agent hire "$dir" --name "Core Agent" --template worker --db-access read --json)"
[ "$(jq -r .ok <<<"$out")" = "true" ] || fail "hire: $out"
[ "$(step_status register-workspace <<<"$out")" = "done" ] || fail "register via core: $out"
[ "$(step_status write-persona <<<"$out")" = "done" ] || fail "persona via core: $out"
[ "$(step_status set-db-access <<<"$out")" = "done" ] || fail "db access via core: $out"
grep -q "WORKER agent" "$dir/.k2/agent/ROLE.md" || fail "template persona written"
conf="$(k2 agent conf "$dir" --json)"
[ "$(jq -r .name <<<"$conf")" = "Core Agent" ] || fail "name: $conf"
again="$(k2 agent hire "$dir" --name "Core Agent" --template worker --db-access read --json)"
[ "$(jq -r .changed <<<"$again")" = "false" ] || fail "re-hire converges: $again"
ok "create + converge"

echo "== hire root =="
k2 agent hire-root set "$SANDBOX_HOME/agents" >/dev/null
root="$(k2 agent hire-root)"
case "$root" in *"/agents") ;; *) fail "hire root: $root" ;; esac
case "$root" in *"(default)"*) fail "root still default: $root" ;; esac
ok "owner sets the hire root"

echo "== recipe hire, team, links, retire =="
recipe="$WORK/desk.json"
cat >"$recipe" <<'JSON'
{
  "team": { "name": "CLI Desk" },
  "members": [
    { "role": "librarian", "lead": true, "persona": "Index plan sets." },
    { "role": "sme", "heartbeats": [{ "name": "digest", "frequency": "daily", "spec": { "time": "07:00" } }] }
  ],
  "onBehalfOf": "cli test"
}
JSON
plan="$(k2 agent instances hire "$recipe" --dry-run --json)"
[ "$(jq -r '.plan.members[0].handle' <<<"$plan")" = "cli-desk-librarian" ] || fail "plan: $plan"
hired="$(k2 agent instances hire "$recipe" --key cli-desk-1 --json)"
inst="$(jq -r .instance <<<"$hired")"
[ "${inst#inst_}" != "$inst" ] || fail "instance id: $hired"
again="$(k2 agent instances hire "$recipe" --key cli-desk-1 --json)"
[ "$(jq -r .changed <<<"$again")" = "false" ] || fail "idempotent: $again"
k2 agent instances | grep -q "$inst" || fail "instances list"
link="$(k2 agent team link "$inst" "$dir" --scope '{"planSets":["ps-1"]}' --json)"
lid="$(jq -r .link.id <<<"$link")"
view="$(k2 agent team --workspace "$dir" --json)"
[ "$(jq -r '.servedBy[0].id' <<<"$view")" = "$lid" ] || fail "served by: $view"
served="$(k2 agent team served --workspace cli-desk-librarian)"
grep -q "$lid" <<<"$served" || fail "desk serves: $served"
set +e
env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_PORT="$K2SO_PORT" K2_PROJECT_PATH="$dir" \
    "$K2_CLI" connections remove cli-desk-librarian >"$WORK/rm.out" 2>"$WORK/rm.err"
rc=$?
set -e
[ "$rc" -ne 0 ] || fail "manual remove of a link edge must refuse"
grep -q "belongs to team link" "$WORK/rm.err" "$WORK/rm.out" || fail "refusal names the link: $(cat "$WORK/rm.err" "$WORK/rm.out")"
k2 agent team unlink "$lid" | grep -q "Unlinked" || fail "unlink"
ret="$(k2 agent instances retire "$inst" --json)"
[ "$(jq -r .status <<<"$ret")" = "retired" ] || fail "retire: $ret"
[ ! -e "$SANDBOX_HOME/agents/cli-desk-librarian" ] || fail "retired folder archived"
ok "recipe hire → link → unlink → retire"

echo "== sender check =="
set +e
k2 msg who m_0000000000 >/dev/null 2>"$WORK/who.err"
rc=$?
set -e
[ "$rc" = "4" ] || fail "unknown delivery id exit 4, got $rc: $(cat "$WORK/who.err")"
grep -q "never sent it" "$WORK/who.err" || fail "who refusal text: $(cat "$WORK/who.err")"
ok "k2 msg who refuses an id K2 never sent"

echo "== api-key create: hire limits need --hire =="
set +e
k2 api-key create --label x --max-live 3 >/dev/null 2>"$WORK/ak.err"
rc=$?
set -e
[ "$rc" = "2" ] || fail "limits without --hire exit 2, got $rc"
ok "limits without --hire refused"

echo
echo "Results: $pass passed"
