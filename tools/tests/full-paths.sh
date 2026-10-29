#!/bin/sh
# The paths each part of tools/tests/full.sh ships (run from the repo root):
#   tools/test-bundle.sh tools/tests/full.sh <out> $(tools/tests/full-paths.sh base-a)
set -eu
KIT="Cargo.toml Cargo.lock crates/base/Cargo.toml crates/base/src vendor/yrs"
# the kit for a part that doesn't run its unit tests: without the test-only module files (each
# `#[cfg(test)] mod x;`, never compiled when the kit is a dependency); v0.9.3, to make room
# for vendor/yrs
KIT_LIB="Cargo.toml Cargo.lock crates/base/Cargo.toml vendor/yrs $(git ls-files crates/base/src | grep -vE '(^|/)([a-z0-9_]+_tests|tests)\.rs$' | tr '\n' ' ')"
# base-c's integration tests (v0.9.3: base-a alone was over a job's variable limit)
C="soak edit_paths edit_cost history presence sessions"
case ${1:?part: base-a, base-b, base-c, conformance, spec-undo or evidence} in
  base-a)
    T=$(git ls-files crates/base/tests crates/base/proptest-regressions | grep -v -e '^crates/base/tests/tier0' -e '^crates/base/tests/promote' -e '^crates/base/tests/parity' -e '^crates/base/tests/erasure' -e '^crates/base/tests/fixtures/')
    for t in $C; do T=$(echo "$T" | grep -v -x "crates/base/tests/$t.rs"); done
    echo "$KIT" $T ;;
  # the evidence notes and their run records, checked (Python only; no cargo)
  evidence) echo "tools/evidence docs/evidence" ;;
  base-c) echo "$KIT_LIB" $(for t in $C; do printf ' crates/base/tests/%s.rs' "$t"; done) ;;
  base-b) echo "$KIT_LIB crates/base/tests/tier0.rs crates/base/tests/tier0_large.rs crates/base/tests/promote.rs crates/base/tests/parity.rs crates/base/tests/parity_blob.rs crates/base/tests/erasure.rs crates/base/tests/fixtures" ;;
  # without the kit's `differential` module (feature-gated; the suite never turns it on) and
  # the reference model's binaries (never built as a dependency): v0.9.5, the part was one
  # chunk over a job's limit (65), twice
  conformance) echo "$(echo "$KIT_LIB" | tr ' ' '\n' | grep -vx crates/base/src/differential.rs | tr '\n' ' ')crates/conformance vendor/mpce-model/Cargo.toml $(git ls-files vendor/mpce-model/src | grep -v '^vendor/mpce-model/src/bin/' | tr '\n' ' ')docs/CONFORMANCE.md" ;;
  # spec/quint: only the file its tests read (v0.9.4: the whole folder put the part over a
  # job's limit, 65 chunks)
  spec-undo) echo "$KIT_LIB crates/spec-undo spec/quint/undo_diff.qnt" ;;
  # not suite parts: what tools/tests/soak.sh and the edit-path measurements need
  soak) echo "$KIT_LIB crates/base/tests/soak.rs" ;;
  measure) echo "$KIT_LIB crates/base/tests/edit_cost.rs crates/base/tests/edit_paths.rs crates/base/tests/soak.rs" ;;
  *) echo "unknown part $1" >&2; exit 2 ;;
esac
