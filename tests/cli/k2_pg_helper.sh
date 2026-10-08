#!/usr/bin/env bash
# Hermetic test for the `psql` verb of scripts/k2-pg-helper (root helper the
# daemon runs through sudo). A PATH shim stands in for runuser and records
# the exact argv and stdin it would hand to psql. No root, no Postgres.
# Runs the helper under sh and, when present, dash and bash.
# Run with: bash tests/cli/k2_pg_helper.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER="$ROOT/scripts/k2-pg-helper"
[ -f "$HELPER" ] || { echo "FAIL: missing $HELPER" >&2; exit 1; }

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

if grep -Ev '^[[:space:]]*#' "$HELPER" | grep -Eq '(^|[^a-z])su[[:space:]]|-c "psql'; then
	fail "helper still runs psql through su -c / a shell string"
fi
ok "no su -c / shell string left"

WORK="$(mktemp -d -t k2-pg-helper-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
cat >"$SHIMS/runuser" <<SH
#!/bin/sh
# Record argv one word per line (NUL-free), then stdin.
: >"$WORK/argv"
for a in "\$@"; do printf '%s\n' "\$a" >>"$WORK/argv"; done
cat >"$WORK/stdin"
SH
chmod +x "$SHIMS/runuser"

SHELLS="sh"
for s in dash bash; do command -v "$s" >/dev/null 2>&1 && SHELLS="$SHELLS $s"; done

run() {
	# run <shell> <stdin> <args...>; sets out/rc
	local sh="$1" input="$2"
	shift 2
	rm -f "$WORK/argv" "$WORK/stdin"
	set +e
	out="$(printf '%s' "$input" | PATH="$SHIMS:$PATH" "$sh" "$HELPER" psql "$@" 2>&1)"
	rc=$?
	set -e
}

expect_argv() {
	# expect_argv <label> <word>...: exact runuser argv
	local label="$1"
	shift
	[ "$rc" -eq 0 ] || fail "$label: exit $rc: $out"
	local want
	want="$(printf '%s\n' "$@")"
	[ "$(cat "$WORK/argv")" = "$want" ] || fail "$label: argv was:
$(cat "$WORK/argv")
want:
$want"
}

expect_refused() {
	local label="$1"
	[ "$rc" -ne 0 ] || fail "$label: must be refused"
	[ ! -e "$WORK/argv" ] || fail "$label: runuser must not run (argv: $(cat "$WORK/argv"))"
}

for SH in $SHELLS; do
	echo "== $SH: every argv shape the daemon sends =="
	run "$SH" "SELECT 1;" -d postgres -tA
	expect_argv "ping" -u postgres -- psql -d postgres -tA
	[ "$(cat "$WORK/stdin")" = "SELECT 1;" ] || fail "stdin not passed through"
	run "$SH" "x" -d postgres -v ON_ERROR_STOP=1 -tA
	expect_argv "on_error_stop + tA" -u postgres -- psql -d postgres -v ON_ERROR_STOP=1 -tA
	run "$SH" "x" -d postgres -v ON_ERROR_STOP=1
	expect_argv "on_error_stop" -u postgres -- psql -d postgres -v ON_ERROR_STOP=1
	run "$SH" "x" -d ws_0f8e2c1a_7b3d_4e5f_9a0b_1c2d3e4f5a6b -v ON_ERROR_STOP=1
	expect_argv "workspace db" -u postgres -- psql -d ws_0f8e2c1a_7b3d_4e5f_9a0b_1c2d3e4f5a6b -v ON_ERROR_STOP=1
	run "$SH" "x" -d my_named_db_2 -v ON_ERROR_STOP=1
	expect_argv "k2 db create --name db" -u postgres -- psql -d my_named_db_2 -v ON_ERROR_STOP=1
	run "$SH" "x" -d 2024_reports
	expect_argv "digit-leading --name db" -u postgres -- psql -d 2024_reports
	ok "$SH: allowed shapes pass through as an exact argv, SQL on stdin"

	echo "== $SH: everything else is refused before psql runs =="
	for bad in \
		'x; touch /tmp/k2-pwn' \
		'x$(id)' \
		'x`id`' \
		'x y' \
		'Upper' \
		'has-dash' \
		'template0' \
		'template1' \
		$'x\ny' \
		$'x\n' \
		'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' \
		''; do
		run "$SH" "x" -d "$bad" -tA
		expect_refused "db name '$bad'"
	done
	run "$SH" "x" -d aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa -tA
	expect_argv "63-char name" -u postgres -- psql -d aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa -tA
	ok "$SH: bad database names refused; a 63-char name is fine"

	run "$SH" "x" -d postgres -c 'SELECT 1'
	expect_refused "-c"
	printf '%s' "$out" | grep -q 'psql -c is forbidden' || fail "-c message: $out"
	run "$SH" "x" -d postgres --command='SELECT 1'
	expect_refused "--command="
	run "$SH" "x" -d postgres -f /etc/shadow
	expect_refused "-f"
	run "$SH" "x" -d postgres -o /tmp/out
	expect_refused "-o"
	run "$SH" "x" -d postgres -v 'ON_ERROR_STOP=0'
	expect_refused "-v other value"
	run "$SH" "x" -d postgres -v
	expect_refused "-v without value"
	run "$SH" "x" -d postgres -d other
	expect_refused "-d twice"
	run "$SH" "x" -d
	expect_refused "-d without value"
	run "$SH" "x" -tA
	expect_refused "no -d"
	run "$SH" "x" --dbname=postgres
	expect_refused "--dbname="
	run "$SH" "x" "-d postgres"
	expect_refused "one word with a space"
	run "$SH" "x" -d postgres -tA --k2-end -c 'SELECT 1'
	expect_refused "sentinel-looking word"
	run "$SH" "x" -d postgres -t -A
	expect_refused "split -t -A (not a shape the daemon sends)"
	ok "$SH: -c/-f/-o, unknown flags, bad -v, duplicate or missing -d refused"
done

echo ""
echo "PASS: k2-pg-helper psql ($pass checks)"
