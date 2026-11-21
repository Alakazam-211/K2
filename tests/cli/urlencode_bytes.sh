#!/usr/bin/env bash
# urlencode puts a value into a query string. It encodes the bytes it is
# given, so a byte that is not valid UTF-8 becomes %XX, and when it cannot
# encode at all (no python3) it prints an error and ends the command, so
# nothing is sent.
# bash-only: reads cli/k2 as source; the parity gate skips it for the binary.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_BASH_SOURCE"
pass=0
die() { echo "FAIL: $*" >&2; exit 1; }

# _k2_end_command and the two helpers it calls when ps can't run.
fns="$(sed -n '/^urlencode()/,/^}/p; /^_k2_proc_table_pgrep()/,/^}/p; /^_k2_end_group()/,/^}/p; /^_k2_end_command()/,/^}/p' "$K2_CLI")"
[ "$(printf '%s\n' "$fns" | grep -c '^urlencode()')" = 1 ] || die "urlencode not found once in cli/k2"
[ "$(printf '%s\n' "$fns" | grep -c '^_k2_end_command()')" = 1 ] || die "_k2_end_command not found once in cli/k2"
for f in _k2_proc_table_pgrep _k2_end_group; do
    [ "$(printf '%s\n' "$fns" | grep -c "^$f()")" = 1 ] || die "$f not found once in cli/k2"
done
eval "$fns"

want() { # want <case> <value> <encoded>
    local got
    got="$(urlencode "$2")" || die "$1: urlencode failed"
    [ "$got" = "$3" ] || die "$1: got '$got', wanted '$3'"
    pass=$((pass + 1))
}
want "plain" "abc" "abc"
want "the safe set" "AZaz09_.-~" "AZaz09_.-~"
want "query specials" "a&b=c?d#e%f+g h/i" "a%26b%3Dc%3Fd%23e%25f%2Bg%20h%2Fi"
want "UTF-8" "é☃" "%C3%A9%E2%98%83"
want "emoji" "😀" "%F0%9F%98%80"
want "empty" "" ""
# Invalid bytes are percent-encoded, and so is everything after them.
want "invalid UTF-8 then &" "$(printf 'a\xffb&x')" "a%FFb%26x"
want "invalid UTF-8 then a parameter" "$(printf '\xc3\x28&project=other')" "%C3%28%26project%3Dother"
want "lone continuation bytes" "$(printf '\x80\xbf')" "%80%BF"

# No python3: an error line, and the WHOLE command ends (as in the CLI, where
# urlencode runs inside $(...) and the main shell traps USR1); the raw value
# never goes out and nothing after it runs. Run in a child shell, since
# ending the command means ending that shell.
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
set +e
out="$(FNS="$fns" PATH="$T:/usr/bin:/bin" BASH_BIN="$(command -v bash)" "$(command -v bash)" -c '
    trap "exit 1" USR1
    eval "$FNS"
    python3() { return 127; }
    v="$(urlencode "a&b")"
    echo "after:$v"
' 2>"$T/err")"; rc=$?
set -e
[ "$rc" != 0 ] || die "no python3: exit 0"
pass=$((pass + 1))
case "$out" in *after*) die "no python3: the command went on and printed '$out'" ;; esac
pass=$((pass + 1))
grep -q "^Error: k2 could not encode a value for its request" "$T/err" || die "no python3: stderr was: $(cat "$T/err")"
pass=$((pass + 1))

echo "urlencode_bytes: $pass passed"
