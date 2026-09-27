#!/usr/bin/env bash
# k2 workspace resources — help, usage, ungoverned tool id, abs path, emit.
#
# No daemon. HOME sandboxed. Never touches the real ~/.k2 or a live resource list.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq "$needle"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2
        fail=$((fail + 1))
    fi
}
assert_rc_nonzero() {
    local label="$1" rc="$2"
    if [ "$rc" -ne 0 ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (exit 0)" >&2
        fail=$((fail + 1))
    fi
}

WORK="$(mktemp -d -t k2-ws-resources-XXXXXX)"
ABS_DIR="$(mktemp -d /private/tmp/k2-wsres-abs-XXXXXX)"
cleanup() {
    rm -rf "$WORK" "$ABS_DIR"
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/ws"
echo "1" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
        -u K2SO_PORT -u K2SO_HOOK_TOKEN \
        -u K2SO_PROJECT_PATH \
        HOME="$HOME" \
        K2_HOST=127.0.0.1 \
        K2_PORT=1 \
        K2_HOOK_TOKEN="fake-token" \
        K2_PROJECT_PATH="$WORK/ws" \
        "$K2_CLI" "$@"
}

echo "== help =="
set +e
help_out="$(run_k2 workspace resources --help 2>&1)"
help_rc=$?
set -e
assert_eq "workspace resources --help exit" "$help_rc" "0"
assert_contains "group page mentions resources add" "$help_out" "resources add"

echo "== usage =="
set +e
bare_out="$(run_k2 workspace resources 2>&1)"
bare_rc=$?
set -e
assert_eq "workspace resources exit" "$bare_rc" "2"
assert_contains "bare resources usage" "$bare_out" "list|add|remove"

set +e
nosuch_out="$(run_k2 workspace resources nosuch 2>&1)"
nosuch_rc=$?
set -e
assert_eq "workspace resources nosuch exit" "$nosuch_rc" "2"
assert_contains "nosuch names the subcommand" "$nosuch_out" "nosuch"

set +e
bogus_out="$(run_k2 workspace bogus 2>&1)"
bogus_rc=$?
set -e
assert_rc_nonzero "workspace bogus fails" "$bogus_rc"
assert_contains "workspace bogus usage names resources" "$bogus_out" "resources"

set +e
two_out="$(run_k2 workspace resources add -- a b 2>&1)"
two_rc=$?
set -e
assert_eq "two files after -- exit" "$two_rc" "2"
assert_contains "two files usage" "$two_out" "Usage:"

echo "== catalog =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "workspace tool id empty" "$(_cli_tool_id_for_verb workspace)" ""
assert_eq "resources is not its own tool id" "$(_cli_tool_id_for_verb resources)" ""
if _cli_tool_is_locked "$(_cli_tool_id_for_verb workspace)"; then
    echo "  FAIL: workspace must stay ungoverned (not a locked tool)" >&2
    fail=$((fail + 1))
else
    echo "  PASS: workspace is not a locked tool"
    pass=$((pass + 1))
fi

# shellcheck disable=SC1090
eval "$(sed -n '/^_uds_eligible()/,/^}/p' "$K2_CLI")"
for endpoint in /cli/workspace/resources /cli/workspace/resources/add /cli/workspace/resources/remove; do
    if _uds_eligible "$endpoint"; then
        echo "  FAIL: $endpoint must not be UDS-eligible" >&2
        fail=$((fail + 1))
    else
        echo "  PASS: $endpoint not UDS-eligible"
        pass=$((pass + 1))
    fi
done

echo "== abs =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_WORKSPACE_RESOURCES/,/^# END_WORKSPACE_RESOURCES/p' "$K2_CLI")"
pushd "$ABS_DIR" >/dev/null
rel_got="$(_workspace_resources_abs "brief.md")"
assert_eq "relative name is PWD/name" "$rel_got" "$PWD/brief.md"
abs_in="/private/tmp/k2-wsres-already-abs"
abs_got="$(_workspace_resources_abs "$abs_in")"
assert_eq "absolute path unchanged" "$abs_got" "$abs_in"
link_in="/tmp/k2-wsres-no-symlink-resolve"
link_got="$(_workspace_resources_abs "$link_in")"
assert_eq "absolute path does not resolve symlinks" "$link_got" "$link_in"
popd >/dev/null

echo "== emit =="
hint_text="not_a_file: path is not an existing file: /no/such"
err_json="$(printf '%s\n' '{"ok":false,"error":{"code":"not_a_file","hint":"not_a_file: path is not an existing file: /no/such"}}')"
err_file="$WORK/err"
set +e
err_stdout="$(printf '%s' "$err_json" | MODE=add WANT_JSON=0 _workspace_resources_emit 2>"$err_file")"
err_rc=$?
set -e
err_stderr="$(cat "$err_file")"
assert_rc_nonzero "not_a_file exits non-zero" "$err_rc"
assert_eq "not_a_file stdout empty" "$err_stdout" ""
assert_contains "not_a_file prints hint" "$err_stderr" "$hint_text"

list_json="$(printf '%s\n' '{"ok":true,"docs":[{"fileName":"brief.md","filePath":"/ws/docs/brief.md","missing":false,"addedAt":1},{"fileName":"gone.md","filePath":"/ws/docs/gone.md","missing":true,"addedAt":2}]}')"
set +e
list_out="$(printf '%s' "$list_json" | MODE=list WANT_JSON=0 _workspace_resources_emit 2>"$err_file")"
list_rc=$?
set -e
assert_eq "list exit" "$list_rc" "0"
assert_eq "list rows include missing" "$list_out" "$(printf 'brief.md\t/ws/docs/brief.md\ngone.md\t/ws/docs/gone.md\n')"

set +e
json_out="$(printf '%s' "$list_json" | MODE=list WANT_JSON=1 _workspace_resources_emit 2>"$err_file")"
json_rc=$?
set -e
assert_eq "WANT_JSON exit" "$json_rc" "0"
assert_eq "WANT_JSON writes the body through" "$json_out" "$list_json"

add_json="$(printf '%s\n' '{"ok":true,"workspaceId":"ws","filePath":"/ws/docs/brief.md","fileName":"brief.md"}')"
set +e
add_out="$(printf '%s' "$add_json" | MODE=add WANT_JSON=0 _workspace_resources_emit 2>"$err_file")"
add_rc=$?
set -e
assert_eq "add exit" "$add_rc" "0"
assert_eq "add prints filePath only" "$add_out" "$(printf '/ws/docs/brief.md\n')"

echo "== $pass passed, $fail failed =="
[ "$fail" -eq 0 ]
