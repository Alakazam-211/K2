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
#   evidence     the evidence tools' self-test and check (tools/evidence/, mpc-tools evidence; the
#                check not in a public cut, which has no run records),
#                tools/check-private.sh (with its self-test), tools/check-windows-names.sh (with its self-test), the
#                engine-features check (cargo tree, no build; not in a public cut) on what the
#                part ships, tools/check-targets.sh (every test target, test-only feature and
#                #[ignore] accounted for in tools/targets.txt, each part named there checked to
#                run it; text only) with its self-test, and tools/oss-check.sh with its
#                self-test (both licence files, "MIT OR Apache-2.0" on our crates and
#                package.json files), and the self-test of the pack template's carrier-row
#                check (scripts/pack-checks/carrier-row.sh)
#   base-c       the kit: soak (CI variant), edit_paths, edit_cost, history, presence, sessions, roster, app_view, overflow
#   base-d       the kit: disk, if_version, blame, blame_incremental (blame_cost is ignored: a
#                measurement), and position_index with its planted engine bugs (v0.9.7)
#   errors       the kit: error_ids (the registry snapshot, explain goldens, ERRORS.md, the walk), tools_session, pack_verbs, and the mpc example
#   base-b       the kit: tier 0's integration tests (tier0, tier0_large, promote, demote), the parity harnesses (parity, parity_blob, parity_grid) and erasure
#   spec-undo    the undo spec suite, its mutant tests included
#   wire-guard   the untrusted-wire build guard (tools/tests/wire-guard.sh), and the native
#                engine's absence without its feature
#   lint         fmt --check and clippy -D warnings of the kit's crates (scripts/check.sh's),
#                and of the tools binary (tools/mpc-tools, its own workspace)
#   tools        the tools binary's tests (tools/mpc-tools): its unit tests, and each port's
#                pin table against the old tool's outputs (tools/README.md, "mpc-tools")
#   plants       each test-only plant alone over the suite whose rows must catch it, after a
#                control of that suite with no plant (tools/tests/plants.sh): CAUGHT only when
#                exactly the named rows fail, each at its own assertion
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
case ${PART:?PART: base-a1, base-a2, base-b, base-c, base-d, errors, spec-undo, wire-guard, plants, lint, tools or evidence} in
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
          # every test target, test-only feature and #[ignore] accounted for (tools/targets.txt)
          timed targets sh tools/check-targets.sh .
          timed targets_selftest sh tools/test-check-targets.sh
          timed oss_selftest sh tools/test-oss-check.sh
          timed oss sh tools/oss-check.sh .
          # the pack template's carrier-row check (scripts/pack-checks/carrier-row.sh), self-test
          timed carrier_row_selftest sh tools/test-carrier-row.sh
          ;;
  base-b) timed build cargo test -p k2-mpc-base --test tier0 --test tier0_large --test promote --test demote --test parity --test parity_blob --test parity_grid --test erasure --test live_edit --no-run
          timed base_b tests -p k2-mpc-base --test tier0 --test tier0_large --test promote --test demote --test parity --test parity_blob --test parity_grid --test erasure --test live_edit ;;
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
          # the skipped set is exactly the four #[ignore]d large trace runs and the two one-trace
          # replays run by name (tools/README.md, "One trace, by name"), by name
          timed spec_undo_ignored ignored_exactly "trace_equivalence_large d1_traces_large group_trace_equivalence_large
            end_break_trace_class_large replay_one grid_trace_one" cargo test -p k2-mpc-spec-undo --tests -- --list --ignored
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
  # each plant alone, after a control of its suite (tools/tests/plants.sh): ROWS are the rows
  # the plant must fail, AT and MSG where and how each fails (its own assertion)
  plants) . tools/tests/plants.sh
          # planted-device-bugs: every connection of a person shows the most active one's caret
          # ("first connection only"), so the rows of tests/roster.rs that read each device's
          # caret fail (a consumer ignoring `client` reads the person's caret, which the plant
          # leaves as it is)
          timed control_roster control roster -- tests -p k2-mpc-base --test roster
          timed plant_device_first_connection plant device-first-connection \
            'k2-mpc-base::roster/two_connections_of_one_pid_each_get_their_own_device_caret
             k2-mpc-base::roster/a_caret_move_on_connection_2_emits_its_client' \
            crates/base/tests/roster.rs '' -- tests -p k2-mpc-base --features planted-device-bugs --test roster
          # the agent-result plants (differential::ErgoMutant, MPC_ERGO_MUTANT): each fails the rows
          # of tests/agent_results.rs that read what it breaks, at their own assertions. The
          # result written though not asked for changes every row's text, so its run is the one
          # row that holds text the default
          timed control_agent_results control agent-results -- tests -p k2-mpc-base --test agent_results
          timed plant_refusal_dropped plant refusal-dropped \
            'k2-mpc-base::agent_results/pack_refusal_has_report
             k2-mpc-base::agent_results/pack_refusal_stopgap_parses
             k2-mpc-base::agent_results/result_of_a_refusal' \
            crates/base/tests/agent_results.rs "a pack's refusal is a report" MPC_ERGO_MUTANT=refusal-dropped \
            -- tests -p k2-mpc-base --test agent_results
          timed plant_units_drop_address plant units-drop-address \
            'k2-mpc-base::agent_results/result_of_an_edit
             k2-mpc-base::agent_results/changed_units_by_address' \
            crates/base/tests/agent_results.rs 'the changed units, by address' MPC_ERGO_MUTANT=units-drop-address \
            -- tests -p k2-mpc-base --test agent_results
          timed plant_envelope_drops_base plant envelope-drops-base \
            'k2-mpc-base::agent_results/result_of_an_edit
             k2-mpc-base::agent_results/result_version_base
             k2-mpc-base::agent_results/result_of_a_refusal' \
            crates/base/tests/agent_results.rs 'version.base: the version computed on' MPC_ERGO_MUTANT=envelope-drops-base \
            -- tests -p k2-mpc-base --test agent_results
          timed plant_data_spec_dropped plant data-spec-dropped \
            'k2-mpc-base::agent_results/result_of_a_typed_read' \
            crates/base/tests/agent_results.rs 'the typed data names its shape' MPC_ERGO_MUTANT=data-spec-dropped \
            -- tests -p k2-mpc-base --test agent_results
          timed plant_json_leaks_to_text plant json-leaks-to-text \
            'k2-mpc-base::agent_results/text_unchanged_with_json_off' \
            crates/base/tests/agent_results.rs 'text stays the default' MPC_ERGO_MUTANT=json-leaks-to-text \
            -- tests -p k2-mpc-base --test agent_results text_unchanged_with_json_off
          # the opened-pin plants (differential::OpenedMutant, MPC_OPENED_MUTANT): each fails the
          # rows of tests/opened.rs that read what it breaks, at their own assertions
          timed control_opened control opened -- tests -p k2-mpc-base --test opened
          timed plant_opened_in_memory plant opened-in-memory \
            'k2-mpc-base::opened/opened_pin_persists
             k2-mpc-base::opened/pin_survives_rewrite
             k2-mpc-base::opened/live_opened_pin_persists' \
            crates/base/tests/opened.rs 'the pin outlives a restart' MPC_OPENED_MUTANT=opened-in-memory \
            -- tests -p k2-mpc-base --test opened
          timed plant_pin_unchecked plant pin-unchecked \
            'k2-mpc-base::opened/pin_checked_at_use' \
            crates/base/tests/opened.rs 'pin is dropped' MPC_OPENED_MUTANT=pin-unchecked \
            -- tests -p k2-mpc-base --test opened
          timed plant_pin_never_moves plant pin-never-moves \
            'k2-mpc-base::opened/pin_moves_with_outside_save
             k2-mpc-base::opened/live_pin_moves_with_outside_save' \
            crates/base/tests/opened.rs 'the pin follows the app' MPC_OPENED_MUTANT=pin-never-moves \
            -- tests -p k2-mpc-base --test opened
          timed plant_file_now_skips_check plant file-now-skips-check \
            'k2-mpc-base::opened/file_now_takes_in_first
             k2-mpc-base::opened/live_file_now_takes_in_first' \
            crates/base/tests/opened.rs 'not a stale one' MPC_OPENED_MUTANT=file-now-skips-check \
            -- tests -p k2-mpc-base --test opened
          timed plant_hold_ignored plant hold-ignored \
            'k2-mpc-base::opened/file_now_holds_write_backs
             k2-mpc-base::opened/live_file_now_holds_write_backs' \
            crates/base/tests/opened.rs 'held: the file waits' MPC_OPENED_MUTANT=hold-ignored \
            -- tests -p k2-mpc-base --test opened
          timed plant_opened_ignored plant opened-ignored \
            'k2-mpc-base::opened/opened_save_merges
             k2-mpc-base::opened/text_save_merges_from_opened
             k2-mpc-base::opened/text_clash_from_opened_reports_kept_pairs
             k2-mpc-base::opened/grid_save_merges_from_opened
             k2-mpc-base::opened/keyed_save_merges_from_opened' \
            crates/base/tests/opened.rs 'merged from the opened version' MPC_OPENED_MUTANT=opened-ignored \
            -- tests -p k2-mpc-base --test opened
          timed plant_base_is_always_known plant base-is-always-known \
            'k2-mpc-base::opened/no_pin_keeps_today' \
            crates/base/tests/opened.rs 'no pin keeps both' MPC_OPENED_MUTANT=base-is-always-known \
            -- tests -p k2-mpc-base --test opened
          timed plant_clash_merged plant clash-merged \
            'k2-mpc-base::opened/keyed_clash_keeps_both' \
            crates/base/tests/opened.rs 'a clash keeps both' MPC_OPENED_MUTANT=clash-merged \
            -- tests -p k2-mpc-base --test opened
          timed plant_view_check_skipped plant view-check-skipped \
            'k2-mpc-base::opened/keyed_merge_checks_pack_rules' \
            crates/base/tests/opened.rs 'the pack refused the merged doc' MPC_OPENED_MUTANT=view-check-skipped \
            -- tests -p k2-mpc-base --test opened
          timed plant_stamp_ignored plant stamp-ignored \
            'k2-mpc-base::opened/stamped_base_first
             k2-mpc-base::opened/unknown_stamp_keeps_both' \
            crates/base/tests/opened.rs 'stamp decides' MPC_OPENED_MUTANT=stamp-ignored \
            -- tests -p k2-mpc-base --test opened
          timed plant_text_base_synced plant text-base-synced \
            'k2-mpc-base::opened/text_save_merges_from_opened
             k2-mpc-base::opened/text_clash_from_opened_reports_kept_pairs' \
            crates/base/tests/opened.rs 'merged from the opened version' MPC_OPENED_MUTANT=text-base-synced \
            -- tests -p k2-mpc-base --test opened
          timed plant_kept_pairs_dropped plant kept-pairs-dropped \
            'k2-mpc-base::opened/text_clash_from_opened_reports_kept_pairs' \
            crates/base/tests/opened.rs 'kept pairs: the live line' MPC_OPENED_MUTANT=kept-pairs-dropped \
            -- tests -p k2-mpc-base --test opened
          plants_summary ;;
  # scripts/check.sh's fmt and clippy, on a node (--needs rust@stable+clippy+rustfmt on k2-node
  # 0.46.7, else tools/job.sh's job-local toolchain); a --repo job only (the whole workspace)
  # first: the tree is the commit (tools/lint-guard.sh; its plants in tools/test-lint-guard.sh),
  # so every lint step below checks the committed files, never ones an earlier step changed
  lint) timed lint_guard_selftest sh tools/test-lint-guard.sh
          timed lint_guard sh tools/lint-guard.sh .
          timed fmt lint cargo fmt -p k2-mpc-base -p k2-mpc-spec-undo -- --check
          timed clippy lint cargo clippy -p k2-mpc-base -p k2-mpc-spec-undo --all-targets -- -D warnings
          timed clippy_mutants lint cargo clippy -p k2-mpc-spec-undo --all-targets --features mutants -- -D warnings
          # the kit's lib alone, as a host builds it: no test cfg and no feature a workspace member
          # turns on, so a value read only by a test-build counter shows as dead code here
          timed clippy_lib lint cargo clippy -p k2-mpc-base --lib -- -D warnings
          # the tools binary (tools/mpc-tools): its own workspace and lock
          timed tools_fmt lint cargo fmt --manifest-path tools/mpc-tools/Cargo.toml -- --check
          timed tools_clippy lint cargo clippy --locked --manifest-path tools/mpc-tools/Cargo.toml --all-targets -- -D warnings
          # and still the commit after every step (none may format or edit what it checks)
          timed lint_guard_after sh tools/lint-guard.sh .
          ;;
  # --show-output: each pin test prints how many rows it checked (a pass that checked none fails)
  tools) timed tools_build cargo test --locked --manifest-path tools/mpc-tools/Cargo.toml --no-run
          timed tools cargo test --locked --manifest-path tools/mpc-tools/Cargo.toml -- --show-output
          ;;
  *) echo "unknown PART $PART" >&2; exit 2 ;;
esac
