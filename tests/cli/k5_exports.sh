#!/usr/bin/env bash
# What the CLI exports for its children carries the K5_ name too.
#
# A child reads a setting as K5_<NAME>, then K2_<NAME>, then K2SO_<NAME>,
# so a value the CLI exports under K2_ alone would lose to a stale K5_ one.
#   1. The healed passport (BEGIN_CLI_HEALED_PASSPORT): K2_HOOK_TOKEN and
#      K5_HOOK_TOKEN always, K2SO_HOOK_TOKEN only when the session had it.
#   2. --workspace on `k2 db` and `k2 agent context`: K2_PROJECT_PATH and
#      K5_PROJECT_PATH, both spellings of the flag. Seen by a python3 shim
#      first on PATH, which logs the two names (paths only) and runs the
#      real python3.
# No daemon (a dead port), HOME is a temp dir.
# bash-only: runs a block cut out of cli/k2 and watches the bash's python3 children.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_BASH_SOURCE"
REAL_PY="$(command -v python3)" || { echo "FAIL: python3 required" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', wanted '$3')"; fi; }

WORK="$(mktemp -d -t k2-k5-exports-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/home" "$WORK/secrets" "$WORK/ws" "$WORK/shim"
WS="$(cd "$WORK/ws" && pwd)"

echo "== 1. the healed passport =="
BLOCK="$(sed -n '/^# BEGIN_CLI_HEALED_PASSPORT/,/^# END_CLI_HEALED_PASSPORT/p' "$K2_CLI")"
[ -n "$BLOCK" ] || { echo "FAIL: no BEGIN_CLI_HEALED_PASSPORT block in $K2_CLI" >&2; exit 1; }
printf '%s\n' "sessA.new1" >"$WORK/secrets/.k2-passport"
chmod 600 "$WORK/secrets/.k2-passport"
# healed <env…>: the three names a child of the healed shell sees.
healed() {
    env -u K2_HOOK_TOKEN -u K5_HOOK_TOKEN -u K2SO_HOOK_TOKEN "$@" \
        K2_SECRETS_DIR="$WORK/secrets" BLOCK="$BLOCK" bash -c '
        set -euo pipefail
        SCOPED_TOKEN="sessA.old1"; TOKEN="$SCOPED_TOKEN"
        eval "$BLOCK"
        bash -c '\''printf "%s|%s|%s" "${K2_HOOK_TOKEN-unset}" "${K5_HOOK_TOKEN-unset}" "${K2SO_HOOK_TOKEN-unset}"'\'''
}
eq "K2_ only: the child sees K2_ and K5_, no K2SO_" \
    "$(healed K2_HOOK_TOKEN=sessA.old1)" "sessA.new1|sessA.new1|unset"
eq "K2_ and K2SO_: all three" \
    "$(healed K2_HOOK_TOKEN=sessA.old1 K2SO_HOOK_TOKEN=sessA.old1)" "sessA.new1|sessA.new1|sessA.new1"
eq "K5_ only: K5_ is replaced, K2_ added" \
    "$(healed K5_HOOK_TOKEN=sessA.old1)" "sessA.new1|sessA.new1|unset"
eq "all three: all replaced" \
    "$(healed K2_HOOK_TOKEN=sessA.old1 K5_HOOK_TOKEN=sessA.old1 K2SO_HOOK_TOKEN=sessA.old1)" "sessA.new1|sessA.new1|sessA.new1"
rm -f "$WORK/secrets/.k2-passport"
eq "no healed passport: nothing exported" \
    "$(healed K2_HOOK_TOKEN=sessA.old1)" "sessA.old1|unset|unset"

echo "== 2. --workspace exports the project path =="
cat >"$WORK/shim/python3" <<EOF
#!/bin/sh
printf 'K2=%s K5=%s\n' "\${K2_PROJECT_PATH-unset}" "\${K5_PROJECT_PATH-unset}" >>'$WORK/py.log'
exec '$REAL_PY' "\$@"
EOF
chmod +x "$WORK/shim/python3"
# project <args…>: run k2 with stale K2_/K5_ project paths; check what its
# python3 children saw once the flag was read.
project() {
    local label="${*//$WS/<ws>}" seen
    : >"$WORK/py.log"
    env -u K2SO_PROJECT_PATH -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K5_PORT -u K5_HOOK_TOKEN \
        -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K5_HOOK_SOCK -u K2_HOST -u K2_CELL -u K2_API_CELL \
        PATH="$WORK/shim:$PATH" HOME="$WORK/home" K2_PORT=1 K2_HOOK_TOKEN=test-token \
        K2_PROJECT_PATH=/stale/k2 K5_PROJECT_PATH=/stale/k5 \
        "$K2_CLI" "$@" >/dev/null 2>&1 || true
    seen="$(grep -F "K2=$WS " "$WORK/py.log" || true)"
    if [ -z "$seen" ]; then
        bad "$label: no python3 child saw K2_PROJECT_PATH=$WS ($(sort -u "$WORK/py.log" | tr '\n' ';'))"
        return
    fi
    if grep -vxF "K2=$WS K5=$WS" <<<"$seen" >/dev/null; then
        bad "$label: a child saw K5_PROJECT_PATH stale ($(sort -u <<<"$seen" | tr '\n' ';'))"
    else
        ok "$label: children see K2_ and K5_ as the workspace"
    fi
}
project db --workspace "$WS" list
project db --workspace="$WS" list
project agent context --workspace "$WS" list
project agent context --workspace="$WS" list

echo
echo "k5_exports: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
