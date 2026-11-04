#!/bin/sh
# check-licences.sh's self-test: an off-list licence fails, in any lock; NCSA passes only for
# libfuzzer-sys in the fuzz lock; our own crates aren't judged in the repo's and the fuzz lock, and
# in the bridge's every package is, path ones included.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
CHECK=$HERE/check-licences.sh
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fails=0; n=0
try() { # $1: wanted exit; $2: main, fuzz or bridge; $3: a package name; $4: its licence; $5: its source
  n=$((n + 1))
  src=${5:-'"registry"'}
  printf '{"packages":[{"name":"%s","version":"1.0.0","source":%s,"license":"%s"}]}' "$3" "$src" "$4" > "$TMP/m.json"
  set +e; sh "$CHECK" "$2" "$TMP/m.json" >/dev/null 2>&1; rc=$?; set -e
  if [ "$rc" = "$1" ]; then echo "ok   $2 $3 $4: exit $rc"
  else echo "FAILED $2 $3 $4: wanted exit $1, got $rc"; fails=$((fails + 1)); fi
}
try 0 main serde "MIT OR Apache-2.0"
try 0 main a "BSD-3-Clause"
try 1 main a "MPL-2.0"
try 1 fuzz a "MPL-2.0"
try 1 main a "GPL-3.0 OR MPL-2.0"
try 1 main libfuzzer-sys "(MIT OR Apache-2.0) AND NCSA"
try 1 fuzz other-crate "(MIT OR Apache-2.0) AND NCSA"
try 0 fuzz libfuzzer-sys "(MIT OR Apache-2.0) AND NCSA"
try 1 fuzz libfuzzer-sys "(MIT OR Apache-2.0) AND MPL-2.0"
try 0 main ours-by-path "MPL-2.0" null
try 0 bridge redb "MIT OR Apache-2.0"
try 1 bridge a "MPL-2.0"
try 1 bridge libfuzzer-sys "(MIT OR Apache-2.0) AND NCSA"
try 0 bridge kit-by-path "MIT OR Apache-2.0" null
try 1 bridge vendored-by-path "MPL-2.0" null
try 1 bridge vendored-by-path "NONE" null
if [ "$fails" -gt 0 ]; then echo "check-licences self-test: $fails of $n failed"; exit 1; fi
echo "check-licences self-test: ok ($n cases)"
