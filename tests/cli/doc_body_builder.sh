#!/usr/bin/env bash
# The doc body builder in cli/k2 (_DOC_BODY_PY) reads a marker only under a
# key the CLI marks with "@", and refuses a marker it can't read with one
# usage error, exit 2 and no body (so _doc_call sends nothing; the CLI side
# of that is tests/cli/doc_body_markers.sh).
#
# Every place in cli/k2 checks its numbers before it builds "@int:N", so no
# command line reaches these refusals today; this pins them for the next
# caller. A plain key's value is sent as typed, marker or not.
#
# How: the builder's Python is read out of cli/k2 and run directly. No
# daemon, no network, no HOME.
# Run with: bash tests/cli/doc_body_builder.sh
# bash-only: reads cli/k2 as source; the parity gate skips it for the binary.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-doc-builder-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

PY_DEF="$(sed -n "/^_DOC_BODY_PY='/,/^print(json.dumps(d))'\$/p" "$CLI_BASH_SOURCE")"
[ -n "$PY_DEF" ] || { echo "FAIL: no _DOC_BODY_PY block in $CLI_BASH_SOURCE" >&2; exit 1; }
_DOC_BODY_PY=""
eval "$PY_DEF"
[ -n "$_DOC_BODY_PY" ] || { echo "FAIL: _DOC_BODY_PY is empty after reading it" >&2; exit 1; }

# build <as_json 0|1> <key value>...: the builder with no workspace, no
# on-behalf-of, no --text or --file, path notes.md. Sets OUT, ERR, RC.
build() {
    local as_json="$1"; shift
    set +e
    OUT="$(python3 -c "$_DOC_BODY_PY" "$as_json" "" "" 0 "" "" notes.md "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# refused <want-stderr> <as_json> <key value>...
refused() {
    local want="$1"; shift
    local label="builder $*"
    build "$@"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ -z "$OUT" ] && ok "$label: no body" \
        || { bad "$label: printed a body"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    if [ "$ERR" = "$want" ]; then
        ok "$label: the usage error"
    else
        bad "$label: stderr is not the usage error"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
    fi
}

# built <want-body> <key value>...
built() {
    local want="$1"; shift
    local label="builder $*"
    build 0 "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ "$OUT" = "$want" ] && ok "$label: $want" \
        || { bad "$label: body is not $want"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
}

echo "== a marker under an @ key that can't be read is refused =="
refused $'Error: version takes a whole number, not \'x\'\n{"error": {"code": "usage", "hint": "version takes a whole number, not \'x\'"}}' \
    0 @version @int:x
refused $'Error: version takes a whole number, not \'\'\n{"error": {"code": "usage", "hint": "version takes a whole number, not \'\'"}}' \
    0 @version @int:
refused $'Error: version takes a whole number, not \'-3\'\n{"error": {"code": "usage", "hint": "version takes a whole number, not \'-3\'"}}' \
    0 @version @int:-3
refused $'Error: partial: \'yes\' is not a value marker\n{"error": {"code": "usage", "hint": "partial: \'yes\' is not a value marker"}}' \
    0 @partial yes
refused $'{"error": {"code": "usage", "hint": "version takes a whole number, not \'x\'"}}' \
    1 @version @int:x

echo "== markers under @ keys, plain text under plain keys =="
built '{"path": "notes.md", "version": 7, "partial": true, "all": false}' @version @int:7 @partial @true @all @false
built '{"path": "notes.md", "name": "@int:x", "with": "@true", "to": "@text", "b": "@b64"}' name @int:x with @true to @text b @b64
built '{"path": "notes.md", "argv": ["cat", "-n"]}' argv '["cat", "-n"]'

echo ""
echo "doc_body_builder: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
