#!/usr/bin/env bash
# tests/cli/_cli_under_test.sh picks the program a CLI test runs
# (CLI_UNDER_TEST) and names the bash source (CLI_BASH_SOURCE). Checks:
# the default is this checkout's cli/k2; K5_CLI_UNDER_TEST overrides the
# program but never the source; an empty value counts as unset; a relative
# value is made absolute; a missing or non-executable program fails loudly.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
HELPER="$SCRIPT_DIR/_cli_under_test.sh"
BASH_CLI="$PROJECT_ROOT/cli/k2"

T="$(mktemp -d -t k2-cli-under-test-XXXXXX)"
T="$(cd "$T" && pwd)"
trap 'rm -rf "$T"' EXIT
pass=0
die() { echo "FAIL: $*" >&2; exit 1; }

# picked [VAR=value...]: source the helper in a clean bash and print
# "<CLI_UNDER_TEST>|<CLI_BASH_SOURCE>". Runs in $T.
picked() {
    (cd "$T" && env -u K5_CLI_UNDER_TEST "$@" bash -c '
        set -euo pipefail
        . "$1"
        printf "%s|%s\n" "$CLI_UNDER_TEST" "$CLI_BASH_SOURCE"
    ' _ "$HELPER")
}
want() {
    local name="$1" got="$2" wanted="$3"
    [ "$got" = "$wanted" ] || die "$name: got '$got', wanted '$wanted'"
    pass=$((pass + 1))
}

printf '#!/bin/sh\necho stand-in\n' > "$T/stand-in"
chmod +x "$T/stand-in"
printf '#!/bin/sh\n' > "$T/not-executable"

want "default" "$(picked)" "$BASH_CLI|$BASH_CLI"
want "override" "$(picked K5_CLI_UNDER_TEST="$T/stand-in")" "$T/stand-in|$BASH_CLI"
want "empty counts as unset" "$(picked K5_CLI_UNDER_TEST=)" "$BASH_CLI|$BASH_CLI"
want "relative is made absolute" "$(picked K5_CLI_UNDER_TEST=stand-in)" "$T/stand-in|$BASH_CLI"

# fails <name> <message part> VAR=value: the helper exits 1 and says why,
# and the script stops there.
fails() {
    local name="$1" part="$2" rc=0 out
    shift 2
    out="$(picked "$@" 2>&1)" || rc=$?
    [ "$rc" = 1 ] || die "$name: exit $rc, wanted 1 (output: $out)"
    case "$out" in
        *"$part"*) ;;
        *) die "$name: the message does not say '$part': $out" ;;
    esac
    case "$out" in
        *"|"*) die "$name: the script went on after the failure: $out" ;;
    esac
    pass=$((pass + 1))
}
fails "missing program" "not an executable file: $T/no-such-k2" K5_CLI_UNDER_TEST="$T/no-such-k2"
fails "not executable" "not an executable file: $T/not-executable" K5_CLI_UNDER_TEST="$T/not-executable"
fails "a directory" "not an executable file: $T" K5_CLI_UNDER_TEST="$T"

echo "cli_under_test_helper: $pass checks passed"
