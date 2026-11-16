# Which k2 a CLI test runs, and where the bash source lives.
#
# The bash CLI (cli/k2) is being replaced by a Rust binary, one family of
# verbs at a time. The tests in tests/cli are the parity contract: a test
# that runs the CLI should run the same way against either. So it takes the
# program from here instead of naming cli/k2 itself.
#
#   CLI_UNDER_TEST   the program to run: K5_CLI_UNDER_TEST when it is set
#                    and not empty (an empty value counts as unset), else
#                    this checkout's cli/k2.
#   CLI_BASH_SOURCE  this checkout's cli/k2, always. For a test that reads
#                    the bash itself (bash -n, grep or sed on the file). A
#                    binary can't stand in for that.
#
# Both are absolute paths. A relative K5_CLI_UNDER_TEST is taken from the
# current directory. Exits 1 with a message when the program is not an
# executable file or the bash source is missing.
#
# Usage (bash, after set -euo pipefail):
#   . "$SCRIPT_DIR/_cli_under_test.sh"
#   K2_CLI="$CLI_UNDER_TEST"

_cut_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CLI_BASH_SOURCE="$_cut_root/cli/k2"
if [ ! -f "$CLI_BASH_SOURCE" ]; then
    echo "FAIL: the bash CLI source is missing: $CLI_BASH_SOURCE" >&2
    exit 1
fi

if [ -n "${K5_CLI_UNDER_TEST:-}" ]; then
    case "$K5_CLI_UNDER_TEST" in
        /*) _cut_prog="$K5_CLI_UNDER_TEST" ;;
        *) _cut_prog="$PWD/$K5_CLI_UNDER_TEST" ;;
    esac
else
    _cut_prog="$CLI_BASH_SOURCE"
fi
if [ ! -f "$_cut_prog" ] || [ ! -x "$_cut_prog" ]; then
    echo "FAIL: the CLI under test is not an executable file: $_cut_prog" >&2
    exit 1
fi
CLI_UNDER_TEST="$_cut_prog"
unset _cut_root _cut_prog
