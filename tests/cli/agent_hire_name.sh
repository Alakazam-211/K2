#!/usr/bin/env bash
# A8 (0.45.1): `k2 agent hire --name` names the agent. From 0.40.100 to
# 0.45.0 a loop variable in the hire planner shadowed --name, so every
# hire of a new folder named the agent "AGENT.md" (or "ROLE.md").
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped, no
# --launch, an empty agent shim dir so nothing real can spawn. Loud: every
# check must pass; nothing is skipped.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

# Never let a caller's K2 session leak into the sandbox.
for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-a8-shim-XXXXXX)"
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT

WORK="$SANDBOX_HOME/work"
mkdir -p "$WORK"

# The CLI as an external terminal on the sandbox box.
k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" "$K2_CLI" "$@"
}

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

set_name_of() { jq -r '[.actions[] | select(.step == "set-name") | .name] | first // ""'; }
has_set_name() { jq -r '[.actions[] | select(.step == "set-name")] | length'; }

echo "== dry-run plans name the agent from --name =="
persona_file="$SANDBOX_HOME/press-persona.md"
printf -- '---\nname: press\n---\n# Press\n' >"$persona_file"
for extra in "" "--template worker" "--persona $persona_file"; do
    dir="$WORK/press-$RANDOM$RANDOM"
    # shellcheck disable=SC2086
    out="$(k2 agent hire "$dir" --name "Press Agent" $extra --dry-run --json)"
    [ "$(jq -r .name <<<"$out")" = "Press Agent" ] \
        || fail "plan name with [${extra:-no extra}] want 'Press Agent': $out"
    [ "$(set_name_of <<<"$out")" = "Press Agent" ] \
        || fail "set-name action with [${extra:-no extra}] want 'Press Agent': $out"
    ok "dry-run --name 'Press Agent' ${extra:+($extra)}"
done

echo "== no --name: no set-name step, name is the folder =="
dir="$WORK/quiet-$RANDOM$RANDOM"
out="$(k2 agent hire "$dir" --dry-run --json)"
[ "$(has_set_name <<<"$out")" = "0" ] || fail "no --name must plan no set-name: $out"
[ "$(jq -r .name <<<"$out")" = "$(basename "$dir")" ] || fail "name want basename: $out"
ok "no --name → basename, no set-name"

echo "== unregistered folder that already has .k2/agent/ROLE.md =="
dir="$WORK/has-role-$RANDOM$RANDOM"
mkdir -p "$dir/.k2/agent"
printf -- '---\nname: has-role\n---\n# Role\n' >"$dir/.k2/agent/ROLE.md"
out="$(k2 agent hire "$dir" --dry-run --json)"
[ "$(jq -r .name <<<"$out")" = "$(basename "$dir")" ] || fail "ROLE.md folder name: $out"
out="$(k2 agent hire "$dir" --name "Role Keeper" --dry-run --json)"
[ "$(jq -r .name <<<"$out")" = "Role Keeper" ] || fail "ROLE.md folder --name: $out"
[ "$(set_name_of <<<"$out")" = "Role Keeper" ] || fail "ROLE.md folder set-name: $out"
ok "existing ROLE.md never becomes the name"

echo "== real hire --name --template worker =="
dir="$WORK/press-real-$RANDOM$RANDOM"
k2 agent hire "$dir" --name "Press Agent" --template worker --json >"$SANDBOX_HOME/hire.json" \
    || fail "real hire failed: $(cat "$SANDBOX_HOME/hire.json")"
conf="$(k2 agent conf "$dir" --json)"
[ "$(jq -r .name <<<"$conf")" = "Press Agent" ] || fail "conf name after hire: $conf"
persona_path="$(jq -r .personaPath <<<"$conf")"
[ -f "$persona_path" ] || fail "persona file missing: $persona_path ($conf)"
grep -Fq "# Persona — Press Agent" "$persona_path" \
    || fail "persona heading must say Press Agent: $(cat "$persona_path")"
grep -Fq "display_name: Press Agent" "$persona_path" \
    || fail "persona display_name must say Press Agent: $(cat "$persona_path")"
ok "real hire names the agent and the persona heading"

echo "== re-hire a registered folder with no persona on disk (CA20a) =="
rm -f "$persona_path"
out="$(k2 agent hire "$dir" --name "Press Desk" --json)" || fail "re-hire failed: $out"
conf="$(k2 agent conf "$dir" --json)"
[ "$(jq -r .name <<<"$conf")" = "Press Desk" ] || fail "re-hire name: $conf"
ok "re-hire with no persona keeps --name"

echo "== daemon refuses the persona file name (CA18) =="
status="$(curl -s -o "$SANDBOX_HOME/setname.json" -w '%{http_code}' -X POST \
    --data-urlencode "project=$dir" --data-urlencode "name=AGENT.md" \
    "http://127.0.0.1:${K2SO_PORT}/cli/workspace/set-agent-display-name?token=${K2SO_TOKEN}")"
body="$(cat "$SANDBOX_HOME/setname.json")"
[ "$status" = "400" ] || fail "set-agent-display-name AGENT.md want 400 got $status: $body"
[ "$(jq -r .code <<<"$body")" = "bad_usage" ] || fail "want code bad_usage: $body"
conf="$(k2 agent conf "$dir" --json)"
[ "$(jq -r .name <<<"$conf")" = "Press Desk" ] || fail "name must be unchanged: $conf"
ok "set-agent-display-name name=AGENT.md → 400 bad_usage"

echo ""
echo "Results: $pass passed, 0 failed"
