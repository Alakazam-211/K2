#!/usr/bin/env bash
# Every CLI test takes the program it runs from _cli_under_test.sh, so a
# stand-in binary (K5_CLI_UNDER_TEST) can run the same suite. A test that
# can only work on the bash says so with a `# bash-only: <reason>` line.
# This gate fails, naming each file, when a tests/cli script:
#   1. names cli/k2 as a path on a non-comment line, does not source
#      _cli_under_test.sh and is not marked bash-only;
#   2. is not bash-only and builds the path itself ("$ROOT/cli/k2"),
#      even when it sources the helper (it should run CLI_UNDER_TEST or
#      read CLI_BASH_SOURCE);
#   3. is not bash-only and runs the program under test with bash
#      (bash "$K2", bash -x "$K2_CLI", bash "$CLI_UNDER_TEST", for any
#      name given "$CLI_UNDER_TEST"). A binary is not a bash script.
# cli/k2so and cli/k2-something are other files and do not count.
# Files whose name starts with `_` are sourced helpers, not tests.
#
# Before the real scan it runs itself on a scratch dir of broken and good
# scripts and fails unless it catches exactly the broken ones.
#
# Run with: bash tests/cli/cli_under_test_gate.sh [dir]   (default tests/cli)
set -euo pipefail
export LC_ALL=C   # one file order everywhere

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SELF="$(basename "$0")"

# Not CLI tests: this gate (its fixtures name cli/k2) and the helper's own
# test (it checks that the helper's default is cli/k2).
EXEMPT=" $SELF cli_under_test_helper.sh "

noncomment() { grep -vE '^[[:space:]]*#' "$1" || true; }

# problems <file>: one line per rule the file breaks, empty when clean.
problems() {
    local f="$1" body bash_only=0 sourced=0 vars v
    body="$(noncomment "$f")"
    grep -qE '^# bash-only:' "$f" && bash_only=1
    grep -qE '^[[:space:]]*(\.|source)[[:space:]]+["'\'']?[^[:space:]]*/_cli_under_test\.sh' <<<"$body" && sourced=1
    if [ "$bash_only" = 0 ] && [ "$sourced" = 0 ] &&
        grep -qE '(^|[^A-Za-z0-9_.-])cli/k2([^A-Za-z0-9_-]|$)' <<<"$body"; then
        echo "names cli/k2 but neither sources _cli_under_test.sh nor carries '# bash-only:'"
    fi
    [ "$bash_only" = 1 ] && return 0
    if grep -qE '\$\{?[A-Za-z_][A-Za-z0-9_]*\}?(/[^[:space:]"'\''|;)]*)?/cli/k2([^A-Za-z0-9_-]|$)' <<<"$body"; then
        echo "builds the cli/k2 path itself; run \$CLI_UNDER_TEST or read \$CLI_BASH_SOURCE"
    fi
    vars="CLI_UNDER_TEST $(grep -oE '[A-Za-z_][A-Za-z0-9_]*="\$\{?CLI_UNDER_TEST\}?"' <<<"$body" | cut -d= -f1 | sort -u | tr '\n' ' ' || true)"
    for v in $vars; do
        if grep -qE "(^|[^A-Za-z0-9_-])bash([[:space:]]+-[A-Za-z]+)*[[:space:]]+\"?\\\$\\{?${v}\\}?([^A-Za-z0-9_]|\$)" <<<"$body"; then
            echo "runs the program under test with bash (\$$v); run it directly"
        fi
    done
}

# scan <dir>: print "<file>: <problem>" for each problem; set CHECKED.
CHECKED=0
scan() {
    local dir="$1" f name
    CHECKED=0
    for f in "$dir"/*.sh; do
        [ -e "$f" ] || continue
        name="$(basename "$f")"
        case "$name" in _*) continue ;; esac
        case "$EXEMPT" in *" $name "*) continue ;; esac
        CHECKED=$((CHECKED + 1))
        problems "$f" | sed "s|^|$name: |"
    done
}

# ── Self-check: the gate catches exactly the broken fixtures. ──
T="$(mktemp -d -t k2-cli-under-test-gate-XXXXXX)"
OUT="$(mktemp -t k2-cli-under-test-gate-out-XXXXXX)"
trap 'rm -rf "$T" "$OUT"' EXIT
fx() { cat >"$T/$1"; }
fx good_helper.sh <<'EOF'
#!/usr/bin/env bash
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
bash -n "$CLI_BASH_SOURCE" || fail "bash -n cli/k2"
"$K2" --version
EOF
fx good_bash_only.sh <<'EOF'
#!/usr/bin/env bash
# bash-only: traces the bash.
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
bash -x "$ROOT/cli/k2" --version
EOF
fx good_other_files.sh <<'EOF'
#!/usr/bin/env bash
# Only cli/k2 in a comment; the rest are other files.
cat "$ROOT/cli/k2so" "$ROOT/cli/k2-helper" "$ROOT/tests/cli/k2_case.sh"
EOF
fx _sourced_helper.sh <<'EOF'
K2="$ROOT/cli/k2"
EOF
fx bad_naked.sh <<'EOF'
#!/usr/bin/env bash
K2="$PROJECT_ROOT/cli/k2"
"$K2" --version
EOF
fx bad_naked_quote.sh <<'EOF'
#!/usr/bin/env bash
cd "$ROOT"; ./cli/k2 --version
EOF
fx bad_builds_path.sh <<'EOF'
#!/usr/bin/env bash
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
"$ROOT/cli/k2" --version
EOF
fx bad_builds_path_up.sh <<'EOF'
#!/usr/bin/env bash
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
"$SCRIPT_DIR/../../cli/k2" --version
EOF
fx bad_bash_run.sh <<'EOF'
#!/usr/bin/env bash
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
out="$(bash "$K2" --version)"
EOF
fx bad_bash_x.sh <<'EOF'
#!/usr/bin/env bash
. "$ROOT/tests/cli/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
bash -x "${K2_CLI}" --version
EOF
fx bad_bash_direct.sh <<'EOF'
#!/usr/bin/env bash
. "$here/_cli_under_test.sh"
bash "$CLI_UNDER_TEST" --version
EOF
scan "$T" >"$OUT"; self="$(cat "$OUT")"
want="bad_bash_direct.sh: runs the program under test with bash (\$CLI_UNDER_TEST); run it directly
bad_bash_run.sh: runs the program under test with bash (\$K2); run it directly
bad_bash_x.sh: runs the program under test with bash (\$K2_CLI); run it directly
bad_builds_path.sh: builds the cli/k2 path itself; run \$CLI_UNDER_TEST or read \$CLI_BASH_SOURCE
bad_builds_path_up.sh: builds the cli/k2 path itself; run \$CLI_UNDER_TEST or read \$CLI_BASH_SOURCE
bad_naked.sh: names cli/k2 but neither sources _cli_under_test.sh nor carries '# bash-only:'
bad_naked.sh: builds the cli/k2 path itself; run \$CLI_UNDER_TEST or read \$CLI_BASH_SOURCE
bad_naked_quote.sh: names cli/k2 but neither sources _cli_under_test.sh nor carries '# bash-only:'"
if [ "$self" != "$want" ]; then
    echo "FAIL: the gate's self-check did not catch exactly the broken fixtures" >&2
    echo "--- got:" >&2; printf '%s\n' "$self" >&2
    echo "--- wanted:" >&2; printf '%s\n' "$want" >&2
    exit 1
fi
[ "$CHECKED" = 10 ] || { echo "FAIL: self-check scanned $CHECKED fixtures, wanted 10 (the _ helper is skipped)" >&2; exit 1; }
selfpass="$(printf '%s\n' "$self" | wc -l | tr -d ' ')"

# ── The real scan. ──
DIR="${1:-$SCRIPT_DIR}"
[ -d "$DIR" ] || { echo "FAIL: no such dir: $DIR" >&2; exit 1; }
scan "$DIR" >"$OUT"; found="$(cat "$OUT")"
[ "$CHECKED" -gt 0 ] || { echo "FAIL: no test scripts in $DIR" >&2; exit 1; }
if [ -n "$found" ]; then
    echo "FAIL: CLI tests that skip _cli_under_test.sh:" >&2
    printf '%s\n' "$found" | sed 's/^/  /' >&2
    echo "Run the CLI as \$CLI_UNDER_TEST (read the bash as \$CLI_BASH_SOURCE), or mark a test that needs the bash with '# bash-only: <reason>' under its header." >&2
    exit 1
fi
echo "cli_under_test_gate: $CHECKED scripts clean, self-check caught $selfpass of $selfpass problems in 10 fixtures"
