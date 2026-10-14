#!/usr/bin/env bash
# `k2 sources` (Garden v3 data sources, contract-sources-v1 §7): the noun
# is wired, help needs no daemon, the schema lists the
# verbs, and the CLI tool policy treats it as a locked `id` tool (inside a
# K2 terminal it sends the session passport, never the owner token).
#
# No daemon needed. HOME is sandboxed. A missing python3 is a FAIL.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

WORK="$(mktemp -d -t k2-sources-cli-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
export HOME="$WORK/home"
mkdir -p "$HOME"
unset K2_HOOK_SOCK K2_HOOK_TOKEN K2SO_HOOK_SOCK K2SO_HOOK_TOKEN || true

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

run() {
    set +e
    "$K2_CLI" "$@" >"$WORK/out" 2>"$WORK/err"
    rc=$?
    set -e
}

echo "== help without a daemon =="
for form in "sources" "sources --help" "sources help" "source -h" "sources list --help"; do
    # shellcheck disable=SC2086 # split the form into args on purpose
    run $form
    if [ "$rc" -eq 0 ] && grep -q "k2 sources list" "$WORK/out"; then
        ok "k2 $form prints the usage"
    else
        bad "k2 $form: exit $rc, out: $(head -c 200 "$WORK/out") err: $(head -c 200 "$WORK/err")"
    fi
done

echo "== schema lists the verbs =="
run --schema
if python3 - "$WORK/out" <<'PY'
import json, sys
doc = json.load(open(sys.argv[1], encoding="utf-8"))
names = {c["name"] for c in doc["commands"]}
need = {"sources list", "sources show", "sources read", "sources propose", "sources approve", "sources trigger"}
missing = sorted(need - names)
if missing:
    print("missing:", missing)
    sys.exit(1)
PY
then ok "schema has the sources verbs"; else bad "schema is missing sources verbs"; fi

echo "== tool policy: sources is a locked id tool =="
policy="$WORK/policy.sh"
sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI" >"$policy"
if [ -s "$policy" ]; then
    # shellcheck disable=SC1090
    . "$policy"
    if [ "$(_cli_tool_id_for_verb sources)" = "sources" ] \
        && [ "$(_cli_tool_id_for_verb source)" = "sources" ] \
        && [ "$(_cli_tool_default_mode sources)" = "id" ] \
        && _cli_tool_is_locked sources; then
        ok "sources → id, locked"
    else
        bad "sources tool policy: id=$(_cli_tool_id_for_verb sources) mode=$(_cli_tool_default_mode sources)"
    fi
else
    bad "tool policy markers not found"
fi

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
