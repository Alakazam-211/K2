#!/usr/bin/env bash
# Tests for scripts/brand/output-judges.py on small corpora: a planted
# `k2 msg` fails judges a and b, a name out of its place fails judge c, a
# keep pattern and a debt row let a line through, a stale debt row fails,
# an unfilled {cli} or {product} slot fails judge d with no way to allow it.
# Text files only: no CLI, no daemon, no cargo.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
J="$HERE/scripts/brand/output-judges.py"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export PYTHONDONTWRITEBYTECODE=1
pass=0
ok() { pass=$((pass + 1)); }
die() { echo "FAIL: $*" >&2; exit 1; }

keep="$T/keep.txt"
printf '# test keep file\nkeep\t(?<![A-Za-z0-9_])\\.k2(?![A-Za-z0-9_])\tthe home folder\nkeep\t\\|k2(?![A-Za-z0-9_])\ta value in a list\n' > "$keep"

# run <name> <corpus text...>: the judges on a one-file corpus; sets rc,
# out (stdout) and err (stderr).
run() {
    local c="$T/corpus-$1"
    shift
    rm -rf "$c"
    mkdir -p "$c"
    printf '%s\n' "$@" > "$c/form.out"
    rc=0
    out="$(python3 "$J" --brand "$HERE/brand/xy" --keep "$keep" "$c" 2> "$T/err")" || rc=$?
    err="$(cat "$T/err")"
}
hits() { printf '%s\n' "$err" | sed -n "s/^judge $1 ([^)]*): \([0-9]*\) hit(s)$/\1/p"; }

run clean 'Run xy msg <addr> to send.' 'Globex Desk keeps your agents.' 'xy 0.46.17' 'Type `xy` alone.' 'See ~/.k2/inbox.'
[ "$rc" = 0 ] || die "a clean corpus: rc $rc: $err"; ok
case "$out" in *"the five judges pass on 1 files"*) ok ;; *) die "clean: $out" ;; esac

run planted 'Run xy msg <addr>.' 'Run k2 msg k2 hi'
[ "$rc" = 1 ] || die "a planted k2 msg passed: $err"; ok
[ "$(hits a)" = 1 ] || die "judge a on a planted k2 msg: $err"; ok
[ "$(hits b)" = 1 ] || die "judge b on a planted k2 msg: $err"; ok
[ "$(hits c)" = 0 ] || die "judge c on a planted k2 msg: $err"; ok
case "$err" in *"FAILED (a, b)"*) ok ;; *) die "planted: $err" ;; esac

run planted-k5 'Use k5 agent list here.'
[ "$rc" = 1 ] && [ "$(hits a)" = 1 ] && [ "$(hits b)" = 1 ] || die "a planted k5 agent: $err"; ok

# Judge b alone: a bare k2 that is not the command by the rule.
run prose 'where k2 keeps it'
[ "$rc" = 1 ] && [ "$(hits a)" = 0 ] && [ "$(hits b)" = 1 ] || die "a bare k2 in prose: $err"; ok

# Keep patterns take out reviewed identifiers.
run kept 'Data lives in ~/.k2/db and .k2/inbox' '--mode <off|custom|k2>'
[ "$rc" = 0 ] || die "kept identifiers: $err"; ok

# Judge c: the names in their places.
for line in 'XY keeps your agents.' 'Xy is up.' 'Run Globex Desk msg <addr>.' 'Globex Desk --json' 'Type `Globex Desk` alone.' "xy's agents are up" 'Welcome to xy!'; do
    run c "$line"
    [ "$rc" = 1 ] && [ "$(hits c)" = 1 ] && [ "$(hits a)" = 0 ] || die "judge c on $line: $err"; ok
done

# Judge d: an unfilled slot fails, alone, wherever it stands, and no keep
# pattern or debt row lets it through. A word that only starts like a slot
# ({clients}) is not one.
for line in 'Run {cli} app list' 'from a terminal outside {product}' '"hint": "{cli} x"' 'a {cli}{product} b'; do
    run d "$line"
    [ "$rc" = 1 ] && [ "$(hits d)" = 1 ] && [ "$(hits a)" = 0 ] && [ "$(hits b)" = 0 ] && [ "$(hits c)" = 0 ] || die "judge d on $line: $err"; ok
    case "$err" in *"FAILED (d)"*"the slot {"*) ;; *"the slot {"*"FAILED (d)"*) ;; *) die "judge d's line on $line: $err" ;; esac; ok
done
printf 'keep\t\\{cli\\}\ta keep pattern is not read by judge d\ndebt\tb\tRun {cli} now\n' > "$T/keep-d"
c="$T/corpus-d2"; rm -rf "$c"; mkdir -p "$c"; printf 'Run {cli} now\n' > "$c/form.out"
rc=0; python3 "$J" --brand "$HERE/brand/xy" --keep "$T/keep-d" "$c" >/dev/null 2>"$T/err" || rc=$?
[ "$rc" = 1 ] && grep -q 'judge d (a slot not filled): 1 hit(s)' "$T/err" || die "judge d took a keep or debt row: $(cat "$T/err")"; ok
run d-text 'Run xy app list {id}, {clients} and {products} for {cli_path}.'
[ "$rc" = 0 ] || die "not a slot: $err"; ok

# Judge e: another brand's sign-in name (ours, K2 Connect) fails under xy;
# xy's own (Globex Account) passes, and a debt row for e lets a line through.
run e 'Sign in with K2 Connect first.'
[ "$rc" = 1 ] && [ "$(hits e)" = 1 ] && [ "$(hits a)" = 0 ] && [ "$(hits c)" = 0 ] || die "judge e on K2 Connect: $err"; ok
case "$err" in *"FAILED (e)"*) ok ;; *) die "judge e's line: $err" ;; esac
run e-own 'Sign in with Globex Account first.' 'K2 Connectors are not a name.'
[ "$rc" = 0 ] || die "xy's own sign-in name: $err"; ok
printf 'debt\te\tSign in with K2 Connect first.\n' > "$T/keep-e"
c="$T/corpus-e2"; rm -rf "$c"; mkdir -p "$c"; printf 'Sign in with K2 Connect first.\n' > "$c/form.out"
rc=0; python3 "$J" --brand "$HERE/brand/xy" --keep "$T/keep-e" "$c" >/dev/null 2>"$T/err" || rc=$?
[ "$rc" = 0 ] || die "a debt row for judge e: $(cat "$T/err")"; ok

# A debt row lets its line through; a debt row that matches nothing fails.
printf 'debt\tb\twhere k2 keeps it\n' >> "$keep"
run debt 'where k2 keeps it'
[ "$rc" = 0 ] || die "a debt row: $err"; ok
run stale 'Run xy msg.'
[ "$rc" = 1 ] || die "a stale debt row passed: $err"; ok
case "$err" in *"stale debt row"*"where k2 keeps it"*) ok ;; *) die "stale: $err" ;; esac
# Judge a takes no debt: a debt row for a command use is not read.
printf 'debt\tb\tRun k2 msg\n' >> "$keep"
run a-debt 'where k2 keeps it' 'Run k2 msg'
[ "$rc" = 1 ] && [ "$(hits a)" = 1 ] || die "judge a took a debt row: $err"; ok

# --write-debt prints a row for each hit of b and c, once.
: > "$T/none"; c="$T/corpus-write"; mkdir -p "$c"; printf 'the k5 way\nthe k5 way\nXY here\n' > "$c/a.out"
rc=0; rows="$(python3 "$J" --brand "$HERE/brand/xy" --keep "$T/none" --write-debt "$c" 2>/dev/null)" || rc=$?
[ "$rc" = 1 ] || die "--write-debt with hits: rc $rc"; ok
[ "$rows" = "$(printf 'debt\tb\tthe k5 way\ndebt\tc\tXY here')" ] || die "--write-debt rows: $rows"; ok

# A bad keep row, a missing corpus and bad arguments are refused.
printf 'keep\tx\n' > "$T/bad"
rc=0; python3 "$J" --brand "$HERE/brand/xy" --keep "$T/bad" "$c" >/dev/null 2>"$T/err" || rc=$?
[ "$rc" = 1 ] && grep -q 'not a keep or debt row' "$T/err" || die "a bad keep row: $(cat "$T/err")"; ok
rc=0; python3 "$J" --brand "$HERE/brand/xy" "$T/nope" >/dev/null 2>"$T/err" || rc=$?
[ "$rc" = 1 ] && grep -q 'usage:' "$T/err" || die "a missing corpus: $(cat "$T/err")"; ok
mkdir -p "$T/empty"
rc=0; python3 "$J" --brand "$HERE/brand/xy" "$T/empty" >/dev/null 2>"$T/err" || rc=$?
[ "$rc" = 1 ] && grep -q 'holds no files' "$T/err" || die "an empty corpus: $(cat "$T/err")"; ok

# The repo's keep file reads (every row is a keep or a debt row).
rc=0; python3 "$J" --brand "$HERE/brand/xy" "$T/corpus-clean" >/dev/null 2>"$T/err" || rc=$?
grep -q 'not a keep or debt row' "$T/err" && die "the repo's keep file: $(cat "$T/err")"; ok

echo "brand output judges tests: $pass passed"
