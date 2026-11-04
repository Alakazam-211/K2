#!/bin/sh
# check-host-forms.sh's self-test: each planted form fails, and each near miss passes.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
CHECK=$HERE/check-host-forms.sh
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fails=0; n=0
try() { # $1: wanted exit; $2: path; $3: the file's text
  n=$((n + 1))
  rm -rf "$TMP/t"; mkdir -p "$TMP/t/$(dirname "$2")"
  printf '%s\n' "$3" > "$TMP/t/$2"
  set +e; "$CHECK" "$TMP/t" >/dev/null 2>&1; rc=$?; set -e
  if [ "$rc" = "$1" ]; then echo "ok   $2 exit $rc: $3"
  else echo "FAILED $2: wanted exit $1, got $rc: $3"; fails=$((fails + 1)); fi
}
try 1 vectors/widget/a.json '{"src":"k2blob:sha256-00"}'
try 1 vectors/widget/a.json '{"votes":{"person:ann":1}}'
try 1 vectors/widget/a.json '{"votes":{"agent:bot":1}}'
try 1 vectors/index.json '{"a":"k2-board"}'
try 1 crates/board/tests/support/cases.rs 'let a = "person:ann";'
try 1 docs/FORMAT.md 'the k2blob scheme'
try 0 vectors/widget/a.json '{"people":{"p-1":{"kind":"person","name":"Ann"}}}'
try 0 vectors/widget/a.json '{"kind":"agent"}'
try 0 docs/FORMAT.md 'one person: two agents: and k2-mpc-board'
set +e; "$CHECK" "$TMP/none-here" >/dev/null 2>&1; rc=$?; set -e
n=$((n + 1)); if [ "$rc" = 2 ]; then echo "ok   no such directory is exit 2"; else echo "FAILED no directory: got $rc"; fails=$((fails + 1)); fi
mkdir -p "$TMP/empty"; set +e; "$CHECK" "$TMP/empty" >/dev/null 2>&1; rc=$?; set -e
n=$((n + 1)); if [ "$rc" = 2 ]; then echo "ok   a tree with no files is exit 2"; else echo "FAILED empty tree: got $rc"; fails=$((fails + 1)); fi
if [ "$fails" -gt 0 ]; then echo "check-host-forms self-test: $fails of $n failed"; exit 1; fi
echo "check-host-forms self-test: ok ($n cases)"
