#!/bin/sh
# FALLBACK ONLY: the paths each part of tools/tests/full.sh ships in an env-chunk bundle
# (tools/test-bundle.sh). A --repo job (the default, tools/README.md "Test jobs") ships the
# whole commit and needs none of this. Run from the repo root:
#   tools/test-bundle.sh --env PART=base-a1 tools/tests/full.sh <out> $(tools/tests/full-paths.sh base-a1)
set -eu
# .cargo/config.toml: the untrusted-wire acknowledgement the kit's own tests need (K10)
KIT="Cargo.toml Cargo.lock .cargo/config.toml .config/nextest.toml crates/base/Cargo.toml crates/base/src vendor/yrs"
# the kit for a part that doesn't run its unit tests: without the test-only module files (each
# `#[cfg(test)] mod x;`, never compiled when the kit is a dependency); v0.9.3, to make room
# for vendor/yrs
KIT_LIB="Cargo.toml Cargo.lock .cargo/config.toml .config/nextest.toml crates/base/Cargo.toml vendor/yrs $(git ls-files crates/base/src | grep -vE '(^|/)([a-z0-9_]+_tests|tests)\.rs$' | tr '\n' ' ')"
# base-c's integration tests (v0.9.3: base-a alone was over a job's variable limit)
C="soak edit_paths edit_cost history presence sessions roster app_view overflow"
# the errors part's integration tests (v0.10: the error surface put base-a over the limit, so
# the run_tool refusal tests moved there with it)
E="error_ids tools_session pack_verbs"
# base-d's (v0.9.7: the vendored engine's patch grew with k2.3): blame's incremental map and its
# cost, position_index with the planted engine bugs, and disk, if_version and blame (in base-a1
# on main from exports P0; with k2.6.3's patch and the merged kit, base-a1 with them needed
# 130,240 inline characters, over a job's 120,000; without them 96,728)
D="disk if_version blame blame_incremental blame_cost position_index"
case ${1:?part: base-a1, base-a2, base-b, base-c, base-d, errors, conformance, spec-undo, wire-guard or evidence} in
  # base-a was one part until the v0.9.5 merge put it over a job's limit (INLINE 142,396):
  # base-a1 is the kit's unit tests (its proptest regressions) and doctests, base-a2 its
  # integration tests on the kit without its unit-test modules
  base-a1) echo "$KIT crates/base/proptest-regressions" ;;
  base-a2)
    T=$(git ls-files crates/base/tests | grep -v -e '^crates/base/tests/tier0' -e '^crates/base/tests/promote' -e '^crates/base/tests/parity' -e '^crates/base/tests/erasure' -e '^crates/base/tests/fixtures/' -e '^crates/base/tests/golden/errors/' -e '^crates/base/tests/vectors/k2-engine/' -e '^crates/base/tests/live_edit.rs')
    for t in $C $D $E; do T=$(echo "$T" | grep -v -x "crates/base/tests/$t.rs"); done
    # K2 Engine's model cases (tests/vectors/k2-engine, xz) are left out: only a LIVE_TRACES
    # run reads them (docs/evidence/v0.9.7-live.md), and they put the part over a job's limit
    # the JS client's shared vectors, which client_vectors.rs checks (v0.9.7)
    echo "$KIT_LIB" $T client/test/vectors ;;
  # the evidence notes and their run records, checked (Python only; no cargo)
  evidence) echo "tools/evidence docs/evidence tools/check-private.sh tools/private-allow.txt" ;;
  # bundled with STRIP_DOCS=1 since v0.9.7 (the K5 run_tool test put it 600 characters
  # past a job's 120,000 inline; it runs no doctests; tools/README.md)
  base-c) echo "$KIT_LIB" $(for t in $C; do printf ' crates/base/tests/%s.rs' "$t"; done) ;;
  base-d) echo "$KIT_LIB" $(for t in $D; do printf ' crates/base/tests/%s.rs' "$t"; done) ;;
  # the error surface (v0.10): its tests, the error goldens, the generated docs/ERRORS.md, the
  # mpc example
  errors) echo "$KIT_LIB" $(for t in $E; do printf ' crates/base/tests/%s.rs' "$t"; done) crates/base/tests/golden/errors docs/ERRORS.md crates/base/examples ;;
  # live_edit and its shared vectors since v0.9.8 (base-a2 was at a job's limit)
  base-b) echo "$KIT_LIB crates/base/tests/tier0.rs crates/base/tests/tier0_large.rs crates/base/tests/promote.rs crates/base/tests/parity.rs crates/base/tests/parity_blob.rs crates/base/tests/erasure.rs crates/base/tests/fixtures crates/base/tests/live_edit.rs crates/base/tests/vectors/live_rebase.json" ;;
  # without the kit's `differential` module (feature-gated; the suite never turns it on) and
  # the reference model's binaries (never built as a dependency): v0.9.5, the part was one
  # chunk over a job's limit (65), twice
  conformance) echo "$(echo "$KIT_LIB" | tr ' ' '\n' | grep -vx crates/base/src/differential.rs | tr '\n' ' ')crates/conformance vendor/mpce-model/Cargo.toml $(git ls-files vendor/mpce-model/src | grep -v '^vendor/mpce-model/src/bin/' | tr '\n' ' ')docs/CONFORMANCE.md" ;;
  # spec/quint: only the file its tests read (v0.9.4: the whole folder put the part over a
  # job's limit, 65 chunks)
  # bundled with STRIP_DOCS=1 since v0.9.7 (D1 put it at 145,656 inline characters;
  # tools/README.md)
  spec-undo) echo "$KIT_LIB crates/spec-undo spec/quint/undo_diff.qnt" ;;
  # K10's build-time guard, checked from inside the repo and as a host would build (no cargo
  # check of tests: the lib alone)
  wire-guard) echo "$KIT_LIB tools/tests/wire-guard.sh" ;;
  # fmt and clippy need every target of every crate: a --repo job only
  lint) echo "full-paths.sh: lint runs only as a --repo job (tools/README.md, \"Test jobs\")" >&2; exit 2 ;;
  # not suite parts: what tools/tests/soak.sh and the edit-path measurements need
  soak) echo "$KIT_LIB crates/base/tests/soak.rs" ;;
  measure) echo "$KIT_LIB crates/base/tests/edit_cost.rs crates/base/tests/edit_paths.rs crates/base/tests/soak.rs" ;;
  # v0.9.5: blame's differential test at node scale and its cost at sizes
  blame) echo "$KIT_LIB crates/base/tests/blame_incremental.rs crates/base/tests/blame_cost.rs" ;;
  *) echo "unknown part $1" >&2; exit 2 ;;
esac
