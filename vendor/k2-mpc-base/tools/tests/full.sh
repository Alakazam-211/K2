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
#   evidence     the evidence tools' self-test and check (tools/evidence/; Python, no cargo; the
#                check not in a public cut, which has no run records),
#                tools/check-private.sh (with its self-test), tools/check-windows-names.sh (with its self-test), the
#                engine-features check (cargo tree, no build; not in a public cut) on what the
#                part ships, and tools/oss-check.sh with its
#                self-test (K-OSS-3: both licence files, "MIT OR Apache-2.0" on our crates and
#                package.json files)
#   base-c       the kit: soak (CI variant), edit_paths, edit_cost, history, presence, sessions, roster, app_view, overflow
#   base-d       the kit: disk, if_version, blame, blame_incremental (blame_cost is ignored: a
#                measurement), and position_index with its planted engine bugs (v0.9.7)
#   errors       the kit: error_ids (the registry snapshot, explain goldens, ERRORS.md, the walk), tools_session, pack_verbs, and the mpc example
#   base-b       the kit: tier 0's integration tests (tier0, tier0_large, promote), the parity harnesses (parity, parity_blob, parity_grid) and erasure
#   spec-undo    the undo spec suite, its mutant tests included
#   wire-guard   the untrusted-wire build guard (tools/tests/wire-guard.sh), and the native
#                engine's absence without its feature
#   lint         fmt --check and clippy -D warnings of the kit's crates (scripts/check.sh's)
# Tests run under cargo-nextest when the node has it (`--needs cargo-nextest`; its settings in
# .config/nextest.toml: no retries, every failure reported), else under cargo test; NEXTEST=0
# forces cargo test. nextest runs no doctests, so base-a1 runs them with cargo test. The build
# step is `cargo test --no-run` either way (nextest builds the same artifacts).
settings PART NEXTEST=1
if [ "${NEXTEST:-1}" != 0 ] && cargo nextest --version >/dev/null 2>&1; then
  echo "tests: $(cargo nextest --version | head -1)"
  tests() { cargo nextest run "$@"; }
else
  echo "tests: cargo test"; NEXTEST=0
  tests() { cargo test "$@"; }
fi
case ${PART:?PART: base-a1, base-a2, base-b, base-c, base-d, errors, spec-undo, wire-guard, lint or evidence} in
  base-a1) timed build cargo test -p k2-mpc-base --lib --no-run
          timed base_a1 tests -p k2-mpc-base --lib
          timed doctests cargo test -p k2-mpc-base --doc ;;
  # every integration test the part shipped (full-paths.sh base-a2 picks them); the kit
  # travels without its unit-test modules, so never --lib here
  base-a2) A2=$(for f in crates/base/tests/*.rs; do printf ' --test %s' "$(basename "$f" .rs)"; done)
          timed build cargo test -p k2-mpc-base $A2 --no-run
          timed base_a2 tests -p k2-mpc-base $A2 ;;
  evidence) timed evidence_selftest sh tools/evidence/test.sh
          timed private_selftest sh tools/test-check-private.sh
          timed private sh tools/check-private.sh .
          timed windows_names_selftest sh tools/test-windows-names.sh
          timed windows_names sh tools/check-windows-names.sh .
          timed oss_selftest sh tools/test-oss-check.sh
          timed oss sh tools/oss-check.sh .
          ;;
  base-b) timed build cargo test -p k2-mpc-base --test tier0 --test tier0_large --test promote --test parity --test parity_blob --test parity_grid --test erasure --test live_edit --no-run
          timed base_b tests -p k2-mpc-base --test tier0 --test tier0_large --test promote --test parity --test parity_blob --test parity_grid --test erasure --test live_edit ;;
  base-c) timed build cargo test -p k2-mpc-base --test soak --test edit_paths --test edit_cost --test history --test presence --test sessions --test roster --test app_view --test overflow --no-run
          timed base_c tests -p k2-mpc-base --test soak --test edit_paths --test edit_cost --test history --test presence --test sessions --test roster --test app_view --test overflow ;;
  base-d) timed build cargo test -p k2-mpc-base --test disk --test if_version --test blame --test blame_incremental --test blame_cost --no-run
          timed base_d tests -p k2-mpc-base --test disk --test if_version --test blame --test blame_incremental --test blame_cost
          timed position_index tests -p k2-mpc-base --features planted-position-bugs --test position_index ;;
  errors) timed build cargo test -p k2-mpc-base --test error_ids --test tools_session --test pack_verbs --no-run
          timed errors tests -p k2-mpc-base --test error_ids --test tools_session --test pack_verbs
          timed example cargo build -p k2-mpc-base --example mpc ;;
  # the mutant tests, each required by name (tools/job.sh `passed`): every mutant fails its
  # rule, the oracle passes at each mutant's bound, and the harness's own checks
  spec-undo) timed build cargo test -p k2-mpc-spec-undo --no-run
          timed spec_undo tests -p k2-mpc-spec-undo
          # the skipped set is exactly the four #[ignore]d large trace runs, by name
          timed spec_undo_ignored ignored_exactly "trace_equivalence_large d1_traces_large group_trace_equivalence_large
            end_break_trace_class_large" cargo test -p k2-mpc-spec-undo --tests -- --list --ignored
          M="the_oracle_passes_at_the_tiny_bound the_oracle_passes_at_the_restart_bound the_oracle_passes_at_the_provenance_bound
            the_oracle_passes_at_the_no_o4a_bound the_oracle_passes_at_the_revert_bound the_oracle_passes_at_the_restorations_bound
            no_taint_fails_u9 partial_hides_taint_fails_u15 redo_retries_skipped_fails_u17
            o5_every_window_end_fails_o5 o5_region_base_fails_o5 skip_u19_fails_u19 rediff_fails_own_only restart_single_taint_fails_rt
            provenance_ignored_fails_i1 no_o4a_fails_i1 refused_revert_applies_fails_u13 redo_rewrites_window_fails_own_only_through_redo
            revert_rewrites_window_fails_own_only_through_revert i1_per_request_fails_i1_at_an_intermediate_step
            o5_every_window_end_fails_o5doc o5_region_base_fails_o5doc redo_retries_skipped_fails_u17doc
            every_mutant_has_its_test no_o4a_fails_i1_in_mid_text
            an_oracle_panic_ends_every_enumerator_mode_as_a_violation d1_mutants_are_caught_by_the_traces"
          if [ "$NEXTEST" = 0 ]; then timed mutants passed "$M" tests -p k2-mpc-spec-undo --features mutants --test mutants
          else timed mutants passed "$M" cargo nextest run --profile mutants -p k2-mpc-spec-undo --features mutants --test mutants; fi ;;
  wire-guard) . tools/tests/wire-guard.sh ;;
  # scripts/check.sh's fmt and clippy, on a node (--needs rust@stable+clippy+rustfmt on k2-node
  # 0.46.7, else tools/job.sh's job-local toolchain); a --repo job only (the whole workspace)
  lint) timed fmt lint cargo fmt -p k2-mpc-base -p k2-mpc-spec-undo -- --check
          timed clippy lint cargo clippy -p k2-mpc-base -p k2-mpc-spec-undo --all-targets -- -D warnings
          timed clippy_mutants lint cargo clippy -p k2-mpc-spec-undo --all-targets --features mutants -- -D warnings ;;
  *) echo "unknown PART $PART" >&2; exit 2 ;;
esac
