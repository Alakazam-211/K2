#!/bin/sh
# check-credit.sh's self-test: a tree with every credit passes, and each missing credit fails and
# names its file. Each case builds a small tree from the good one with one change.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
CHECK=$HERE/check-credit.sh
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
CREDIT='Copyright (c) 2026 Baden Ballard, MIT'
good() { # $1: the tree to fill
  for f in crates/board/src/schema.rs crates/board/src/verbs/templates.rs crates/board/src/verbs/edit.rs \
           crates/board/src/verbs/arrange.rs crates/board/src/parity.rs \
           crates/board/tests/support/cases.rs; do
    mkdir -p "$1/$(dirname "$f")"
    printf '//! Ported from Board, %s (see THIRD-PARTY-NOTICES.md).\n\nfn f() {}\n' "$CREDIT" > "$1/$f"
  done
  # A credit across two comment lines is still the header's.
  printf '//! The rules derive from Board, Copyright (c) 2026 Baden Ballard,\n//! MIT.\n#![allow(dead_code)]\nfn f() {}\n' \
    > "$1/crates/board/src/verbs/clone.rs"
  mkdir -p "$1/docs" "$1/schema" "$1/vectors/widget"
  for f in docs/FORMAT.md docs/PARITY.md vectors/widget/README.md; do printf '# x\n\nPortions derived from Board, %s.\n' "$CREDIT" > "$1/$f"; done
  printf '{"notice":"Portions derived from Board, %s."}\n' "$CREDIT" > "$1/schema/board-schema.json"
  printf 'Portions derived from Board, %s.\n\nCopyright (c) 2026 Baden Ballard\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nThe above copyright notice and this permission notice shall be included in all\nTHE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\n' "$CREDIT" > "$1/THIRD-PARTY-NOTICES.md"
  printf 'Parts come from Board: see `THIRD-PARTY-NOTICES.md`.\n' > "$1/NOTICE.md"
  # A source that mentions boards without naming Board as a source.
  printf '//! Reads a board from the bytes given.\nfn f() {}\n' > "$1/crates/board/src/model.rs"
}
fails=0; n=0
try() { # $1: case name; $2: wanted exit; $3: the file the finding must name, or -; then: a change to make
  name=$1; want=$2; names=$3; shift 3
  n=$((n + 1)); t="$TMP/$n"; good "$t"
  ( cd "$t" && eval "$*" )
  set +e; out=$("$CHECK" "$t" 2>&1); rc=$?; set -e
  if [ "$rc" != "$want" ]; then echo "FAILED $name: wanted exit $want, got $rc"; fails=$((fails + 1)); return; fi
  if [ "$names" != - ] && ! printf '%s\n' "$out" | grep -qF "FAILED $names: "; then
    echo "FAILED $name: the finding doesn't name $names"; fails=$((fails + 1)); return
  fi
  echo "ok   $name exit $rc"
}
try good-tree 0 - 'true'
try header-missing 1 crates/board/src/verbs/edit.rs \
  'printf "//! The verbs.\nfn f() {}\n// %s\n" "$CREDIT" > crates/board/src/verbs/edit.rs'
try header-partial 1 crates/board/src/verbs/arrange.rs \
  'printf "//! Ported from Board.\nfn f() {}\n" > crates/board/src/verbs/arrange.rs'
try ported-file-gone 1 crates/board/src/parity.rs 'rm crates/board/src/parity.rs'
try generated-credit-missing 1 docs/PARITY.md 'printf "# Agent parity\n" > docs/PARITY.md'
try schema-credit-missing 1 schema/board-schema.json 'printf "{}\n" > schema/board-schema.json'
try unlisted-port 1 crates/board/src/new.rs \
  'printf "//! Ported from Board, %s.\nfn f() {}\n" "$CREDIT" > crates/board/src/new.rs'
try unlisted-partial-credit 1 crates/board/src/other.rs \
  'printf "// Portions derived from Board.\nfn f() {}\n" > crates/board/src/other.rs'
try unlisted-across-lines 1 fuzz/src/lib.rs \
  'mkdir -p fuzz/src && printf "//! The shapes come from\n//! Board, as its widget keeps them.\n" > fuzz/src/lib.rs'
try notice-pointer-missing 1 NOTICE.md 'printf "Parts come from elsewhere.\n" > NOTICE.md'
try mit-notice-missing 1 THIRD-PARTY-NOTICES.md \
  'printf "Portions derived from Board, %s.\n" "$CREDIT" > THIRD-PARTY-NOTICES.md'
set +e; "$CHECK" "$TMP/none-here" >/dev/null 2>&1; rc=$?; set -e
n=$((n + 1)); if [ "$rc" = 2 ]; then echo "ok   no-such-directory exit 2"; else echo "FAILED no-such-directory: got $rc"; fails=$((fails + 1)); fi
if [ "$fails" -gt 0 ]; then echo "check-credit self-test: $fails of $n failed"; exit 1; fi
echo "check-credit self-test: ok ($n cases)"
