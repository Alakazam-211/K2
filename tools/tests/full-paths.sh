#!/bin/sh
# The paths each part of tools/tests/full.sh ships (run from the repo root):
#   tools/test-bundle.sh tools/tests/full.sh <out> $(tools/tests/full-paths.sh base-a)
set -eu
KIT="Cargo.toml Cargo.lock crates/base/Cargo.toml crates/base/src"
case ${1:?part: base-a, base-b, conformance or spec-undo} in
  base-a)
    T=$(git ls-files crates/base/tests crates/base/proptest-regressions | grep -v -e '^crates/base/tests/tier0' -e '^crates/base/tests/fixtures/')
    echo "$KIT" $T ;;
  base-b) echo "$KIT crates/base/tests/tier0.rs crates/base/tests/tier0_large.rs crates/base/tests/fixtures" ;;
  conformance) echo "$KIT crates/conformance vendor/mpce-model/Cargo.toml vendor/mpce-model/src docs/CONFORMANCE.md" ;;
  spec-undo) echo "$KIT crates/spec-undo spec/quint" ;;
  *) echo "unknown part $1" >&2; exit 2 ;;
esac
