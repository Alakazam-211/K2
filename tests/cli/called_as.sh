#!/usr/bin/env bash
# The CLI knows the name it was called by (k2 or k5), and that is all it
# does with it today: the answer is the same under either name.
# Read from bash's own trace, so the CLI needs no verb for it.
# K5_CLI_CALLED_AS (set by the Rust front, which runs this script from a
# path of its own) wins over $0 when it says k2 or k5; anything else, and
# an empty value, is ignored.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
unset K5_CLI_CALLED_AS

T="$(mktemp -d -t k2-called-as-XXXXXX)"
trap 'rm -rf "$T"; hermetic_cli_cleanup' EXIT
pass=0
die() { echo "FAIL: $*" >&2; exit 1; }

ln -s "$K2_CLI" "$T/k2"
ln -s "$K2_CLI" "$T/k5"
ln -s "$K2_CLI" "$T/k2so"
cp "$K2_CLI" "$T/some-copy"

# called_as <path> [K5_CLI_CALLED_AS value]: the value the script gave
# _K2_CALLED_AS, from the trace. With no second argument the variable is
# unset.
called_as() {
    local trace line
    if [ $# -ge 2 ]; then
        trace="$(K5_CLI_CALLED_AS="$2" bash -x "$1" --version 2>&1 >/dev/null)" || die "$1 --version failed: $(printf '%s' "$trace" | tail -3)"
    else
        trace="$(bash -x "$1" --version 2>&1 >/dev/null)" || die "$1 --version failed: $(printf '%s' "$trace" | tail -3)"
    fi
    line="$(grep -E '^\++ _K2_CALLED_AS=' <<<"$trace")" || die "$1: no _K2_CALLED_AS line in the trace"
    [ "$(wc -l <<<"$line" | tr -d ' ')" = 1 ] || die "$1: _K2_CALLED_AS is set more than once: $line"
    printf '%s\n' "${line#*=}"
}
want() { local got; got="$(called_as "$1")"; [ "$got" = "$2" ] || die "$1 was read as '$got', wanted '$2'"; pass=$((pass + 1)); }
# want_env <path> <K5_CLI_CALLED_AS value> <wanted>
want_env() {
    local got
    got="$(called_as "$1" "$2")"
    [ "$got" = "$3" ] || die "$1 with K5_CLI_CALLED_AS='$2' was read as '$got', wanted '$3'"
    pass=$((pass + 1))
}

want "$T/k2" k2
want "$T/k5" k5
want "$K2_CLI" k2
# Any other name counts as k2: the old shim's name, a copy.
want "$T/k2so" k2
want "$T/some-copy" k2

# K5_CLI_CALLED_AS names k2 or k5: it wins over $0, either way.
want_env "$T/k2" k5 k5
want_env "$T/k5" k2 k2
want_env "$T/some-copy" k5 k5
want_env "$T/k2so" k5 k5
want_env "$T/k2" k2 k2
want_env "$T/k5" k5 k5
# Empty, or anything but the two bare names: ignored, $0 decides.
want_env "$T/k5" "" k5
want_env "$T/k2" "" k2
want_env "$T/k5" k2so k5
want_env "$T/k2" K5 k2
want_env "$T/k5" "k2 " k5
want_env "$T/k2" /usr/local/bin/k5 k2
want_env "$T/some-copy" k5-cli k2

# Nothing reads the name yet: both names answer the same, on both streams.
for args in "--version" "study what" "nosuchverb"; do
    # shellcheck disable=SC2086
    a="$(bash "$T/k2" $args 2>&1)" && ca=0 || ca=$?
    # shellcheck disable=SC2086
    b="$(bash "$T/k5" $args 2>&1)" && cb=0 || cb=$?
    [ "$ca" = "$cb" ] || die "'$args': exit $ca as k2, $cb as k5"
    [ "$a" = "$b" ] || die "'$args': the output differs between k2 and k5"
    [ -n "$a" ] || die "'$args': no output at all"
    # shellcheck disable=SC2086
    c="$(K5_CLI_CALLED_AS=k5 bash "$T/some-copy" $args 2>&1)" && cc=0 || cc=$?
    [ "$ca" = "$cc" ] || die "'$args': exit $ca as k2, $cc when K5_CLI_CALLED_AS=k5"
    [ "$a" = "$c" ] || die "'$args': the output differs when K5_CLI_CALLED_AS=k5"
    pass=$((pass + 1))
done
# The variable is assigned once and read nowhere else in the script.
[ "$(grep -c '_K2_CALLED_AS' "$K2_CLI")" = 2 ] || die "_K2_CALLED_AS should appear on exactly its 2 assignment lines; found $(grep -c '_K2_CALLED_AS' "$K2_CLI")"
pass=$((pass + 1))

echo "called_as: $pass passed, 0 failed"
