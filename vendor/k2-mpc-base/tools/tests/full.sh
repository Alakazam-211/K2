# The workspace's tests, in parts (PART picks one), each a job of a few minutes. A part runs
# in a checkout of the repo shipped by the runner (tools/README.md, "Test jobs"):
#   k2 compute run <node> --repo <repo> [--sha REV] --needs rust@stable,cargo-nextest \
#     --cpus 4 --mem 8G --env PART=base-a1 -- sh tools/job.sh tools/tests/full.sh
# (the fallback, tools/test-bundle.sh, ships only what tools/tests/full-paths.sh lists for the
# part). Debug builds, as CI runs them (with the workspace's dev profile: tools/README.md,
# "Build settings"). fmt and clippy run as the `lint` part (a --repo job; the licence check
# stays in scripts/check.sh).
#   base-a1      the kit: unit tests and doctests
#   base-a2      the kit: every other integration test but tier 0's, base-c's, base-d's and errors' (client_vectors among them)
#   evidence     the evidence tools' self-test and check (tools/evidence/; Python, no cargo), and
#                tools/check-private.sh on what the part ships
#   base-c       the kit: soak (CI variant), edit_paths, edit_cost, history, presence, sessions, roster, app_view, overflow
#   base-d       the kit: disk, if_version, blame, blame_incremental (blame_cost is ignored: a
#                measurement), and position_index with its planted engine bugs (v0.9.7)
#   errors       the kit: error_ids (the registry snapshot, explain goldens, ERRORS.md, the walk), tools_session, pack_verbs, and the mpc example
#   base-b       the kit: tier 0's integration tests (tier0, tier0_large, promote), the parity harnesses (parity, parity_blob) and erasure
#   conformance  the conformance suite (with the vendored reference model)
#   spec-undo    the undo spec suite, its mutant tests included
#   wire-guard   the untrusted-wire build guard (tools/tests/wire-guard.sh)
#   lint         fmt --check and clippy -D warnings of the kit's crates (scripts/check.sh's)
# Tests run under cargo-nextest when the node has it (`--needs cargo-nextest`; its settings in
# .config/nextest.toml: no retries, every failure reported), else under cargo test; NEXTEST=0
# forces cargo test. nextest runs no doctests, so base-a1 runs them with cargo test. The build
# step is `cargo test --no-run` either way (nextest builds the same artifacts).
if [ "${NEXTEST:-1}" != 0 ] && cargo nextest --version >/dev/null 2>&1; then
  echo "tests: $(cargo nextest --version | head -1)"
  tests() { cargo nextest run "$@"; }
else
  echo "tests: cargo test"; NEXTEST=0
  tests() { cargo test "$@"; }
fi
case ${PART:?PART: base-a1, base-a2, base-b, base-c, base-d, errors, conformance, spec-undo, wire-guard, lint or evidence} in
  base-a1) timed build cargo test -p k2-mpc-base --lib --no-run
          timed base_a1 tests -p k2-mpc-base --lib
          timed doctests cargo test -p k2-mpc-base --doc ;;
  # every integration test the part shipped (full-paths.sh base-a2 picks them); the kit
  # travels without its unit-test modules, so never --lib here
  base-a2) A2=$(for f in crates/base/tests/*.rs; do printf ' --test %s' "$(basename "$f" .rs)"; done)
          timed build cargo test -p k2-mpc-base $A2 --no-run
          timed base_a2 tests -p k2-mpc-base $A2 ;;
  evidence) timed evidence_selftest sh tools/evidence/test.sh
          timed evidence_check python3 tools/evidence/check.py
          timed private sh tools/check-private.sh . ;;
  base-b) timed build cargo test -p k2-mpc-base --test tier0 --test tier0_large --test promote --test parity --test parity_blob --test erasure --test live_edit --no-run
          timed base_b tests -p k2-mpc-base --test tier0 --test tier0_large --test promote --test parity --test parity_blob --test erasure --test live_edit ;;
  base-c) timed build cargo test -p k2-mpc-base --test soak --test edit_paths --test edit_cost --test history --test presence --test sessions --test roster --test app_view --test overflow --no-run
          timed base_c tests -p k2-mpc-base --test soak --test edit_paths --test edit_cost --test history --test presence --test sessions --test roster --test app_view --test overflow ;;
  base-d) timed build cargo test -p k2-mpc-base --test disk --test if_version --test blame --test blame_incremental --test blame_cost --no-run
          timed base_d tests -p k2-mpc-base --test disk --test if_version --test blame --test blame_incremental --test blame_cost
          timed position_index tests -p k2-mpc-base --features planted-position-bugs --test position_index ;;
  errors) timed build cargo test -p k2-mpc-base --test error_ids --test tools_session --test pack_verbs --no-run
          timed errors tests -p k2-mpc-base --test error_ids --test tools_session --test pack_verbs
          timed example cargo build -p k2-mpc-base --example mpc ;;
  conformance) timed build cargo test -p k2-mpc-conformance --no-run
          timed conformance tests -p k2-mpc-conformance ;;
  spec-undo) timed build cargo test -p k2-mpc-spec-undo --no-run
          timed spec_undo tests -p k2-mpc-spec-undo
          if [ "$NEXTEST" = 0 ]; then timed mutants tests -p k2-mpc-spec-undo --features mutants --test mutants
          else timed mutants cargo nextest run --profile mutants -p k2-mpc-spec-undo --features mutants --test mutants; fi ;;
  wire-guard) . tools/tests/wire-guard.sh ;;
  # scripts/check.sh's fmt and clippy, on a node (--needs rust@stable+clippy+rustfmt on k2-node
  # 0.46.7, else tools/job.sh's job-local toolchain); a --repo job only (the whole workspace)
  lint) timed fmt lint cargo fmt -p k2-mpc-base -p k2-mpc-conformance -p k2-mpc-spec-undo -- --check
          timed clippy lint cargo clippy -p k2-mpc-base -p k2-mpc-conformance -p k2-mpc-spec-undo --all-targets -- -D warnings
          timed clippy_mutants lint cargo clippy -p k2-mpc-spec-undo --all-targets --features mutants -- -D warnings ;;
  *) echo "unknown PART $PART" >&2; exit 2 ;;
esac
