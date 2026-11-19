#!/usr/bin/env bash
# `k2 connect status` reads ~/.k2/tunnel.json as data: whatever its fields
# hold, nothing in them runs, and each value prints as the text it is.
#
# Covers:
#   1. A subdomain (and a token) holding $(...), backticks, $HOME and $var,
#      a single quote, a double quote, both, a newline, a backslash, a NUL:
#      no file appears, the command exits 0, the subdomain prints as text
#      and the token counts as set.
#   2. Values that are not strings, a file that is not JSON, a JSON list:
#      no subdomain, no token, exit 0.
#
# No daemon: HOME is a sandbox with no port or token files, and the env
# carries no session, so the last line is the "daemon not running" one.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}

WORK="$(mktemp -d -t k2-connect-status-XXXXXX)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2"
MARK="$WORK/MARK"
NOT_RUNNING="  Tunnel status:   (daemon not running — start with: k2 daemon start)"

# The CLI with nothing from this shell's session: no port, token, socket.
status() {
    env -i PATH="$PATH" HOME="$HOME" TMPDIR="${TMPDIR:-/tmp}" "$K2_CLI" connect status
}
run() {
    set +e
    "$@" >"$WORK/out" 2>"$WORK/err"
    rc=$?
    set -e
}
# tunnel.json from a Python literal for its JSON value.
tunnel() {
    python3 -c 'import ast, json, sys; json.dump(ast.literal_eval(sys.argv[1]), open(sys.argv[2], "w"))' "$1" "$HOME/.k2/tunnel.json"
}
# The text between the account line and the token line: the subdomain line,
# however many lines its value spans.
paired() {
    local out rest
    out="$(cat "$WORK/out")"
    rest="${out#*$'\n'*$'\n'}"
    printf '%s' "${rest%%$'\n  Tunnel token:'*}"
}

echo "== tunnel.json fields are data =="
# label|value (a Python string literal)|what the subdomain prints as
while IFS='|' read -r label value want; do
    rm -f "$MARK" "$MARK"2
    tunnel "{'subdomain': $value, 'token': $value}"
    run status
    want_text="$(python3 -c 'import ast, sys; sys.stdout.write(ast.literal_eval(sys.argv[1]))' "$want")"
    assert_eq "$label: exit 0" "$rc" "0"
    assert_eq "$label: nothing ran" "$( [ -e "$MARK" ] || [ -e "$MARK"2 ] && echo ran || echo none)" "none"
    assert_eq "$label: the subdomain prints as text" "$(paired)" \
        "  Paired subdomain: $want_text → https://$want_text.k2.dev"
    assert_eq "$label: the token counts as set" "$(grep '^  Tunnel token:' "$WORK/out")" "  Tunnel token:     yes"
    assert_eq "$label: stderr is empty" "$(cat "$WORK/err")" ""
done <<EOF
dollar-paren|'acme\$(touch $MARK)'|'acme\$(touch $MARK)'
quote then dollar-paren|"it's\$(touch $MARK)"|"it's\$(touch $MARK)"
backticks|"it's\`touch $MARK\`"|"it's\`touch $MARK\`"
home and var|"it's \$HOME \${HOME} \$var"|"it's \$HOME \${HOME} \$var"
single quote|"it's"|"it's"
double quote|'say "hi"'|'say "hi"'
both quotes|'it'"'"'s "x"\$(touch $MARK)'|'it'"'"'s "x"\$(touch $MARK)'
newline|'a\\nb\$(touch $MARK)'|'a\\nb\$(touch $MARK)'
backslash|'a\\\\b'|'a\\\\b'
backslash before a quote|"a\\\\'\$(touch ${MARK}2)"|"a\\\\'\$(touch ${MARK}2)"
nul|'ac\\x00me'|'acme'
EOF

echo "== a file that can't be read gives no pairing =="
for case_name in number list notjson; do
    case "$case_name" in
        number) tunnel "{'subdomain': 7, 'token': 'tok'}" ;;
        list) tunnel "['acme']" ;;
        notjson) printf 'sub=$(touch %s)\n' "$MARK" >"$HOME/.k2/tunnel.json" ;;
    esac
    rm -f "$MARK"
    run status
    assert_eq "$case_name: exit 0" "$rc" "0"
    assert_eq "$case_name: nothing ran" "$([ -e "$MARK" ] && echo ran || echo none)" "none"
    assert_eq "$case_name: the whole output" "$(cat "$WORK/out")" "K2 Connect account
  Account session: no
  Paired subdomain: '(none)'
  Tunnel token:     no
$NOT_RUNNING"
done

echo
echo "connect_status_files: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
