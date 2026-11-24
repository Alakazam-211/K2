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
#   cli_page FILE    a cli/text page as both CLIs print it in the default
#                    brand: its {company}, {login}, {contact_email} and
#                    {repo} slots filled with ours, as scripts/build-cli.sh
#                    and the k5 build fill them
#                    (scripts/brand/page_slots.py). A test that compares
#                    what a CLI prints with a page reads the page this way.
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
CLI_PAGE_SLOTS="$_cut_root/scripts/brand/page_slots.py"
# The python3 on PATH now, before a test puts its stubs first.
CLI_PAGE_PYTHON="$(command -v python3 || true)"
unset _cut_root _cut_prog

cli_page() {
    if grep -qF -e '{company}' -e '{login}' -e '{contact_email}' -e '{repo}' "$1"; then
        [ -n "$CLI_PAGE_PYTHON" ] || { echo "FAIL: cli_page needs python3 for $1" >&2; return 1; }
        "$CLI_PAGE_PYTHON" "$CLI_PAGE_SLOTS" < "$1"
    else
        cat "$1"
    fi
}
