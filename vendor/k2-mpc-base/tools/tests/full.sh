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
#   base-b       the kit: tier 0's integration tests (tier0, tier0_large, tier0_verbs, tier0_outside, tier0_base, tier0_batch, tier0_revert, tier0_holds_write, promote, demote), the parity harnesses (parity, parity_blob, parity_grid) and erasure
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
# the grouped-commit kill row (crates/base/src/session/grouped_commit_tests.rs): 50 kills at
# seeded points in a gate (the row's own default is 10)
export K2MB_GROUPED_KILLS=${K2MB_GROUPED_KILLS:-50}
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
  base-b) timed build cargo test -p k2-mpc-base --test tier0 --test tier0_large --test tier0_verbs --test tier0_outside --test tier0_base --test tier0_batch --test tier0_revert --test tier0_holds_write --test promote --test demote --test parity --test parity_blob --test parity_grid --test erasure --test live_edit --no-run
          timed base_b tests -p k2-mpc-base --test tier0 --test tier0_large --test tier0_verbs --test tier0_outside --test tier0_base --test tier0_batch --test tier0_revert --test tier0_holds_write --test promote --test demote --test parity --test parity_blob --test parity_grid --test erasure --test live_edit ;;
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
          # the text-only disk replica (v0.23, MPC_BETA_PLANT, the kit's lib): the synced state
          # an outside save the doc doesn't hold is stored on, made from the replica's texts on
          # fresh elements instead of decoded from the store (stale texts where the decoded state
          # was needed), fails both rows that compare it with the decoded replica's, at the
          # stored state's comparison
          timed control_text_replica control text-replica -- tests -p k2-mpc-base --lib writeback_diff_tests
          timed plant_text_replica_state plant yrs-replica-state-from-texts \
            'k2-mpc-base/session::writeback_diff_tests::the_text_replica_matches_the_decoded_replica
             k2-mpc-base/session::writeback_diff_tests::an_outside_save_the_doc_doesnt_hold_stores_what_the_decoded_replica_stored' \
            crates/base/src/session/writeback_diff_tests.rs "the stored synced states" MPC_BETA_PLANT=yrs-replica-state-from-texts \
            -- tests -p k2-mpc-base --lib text_replica_matches an_outside_save_the_doc_doesnt_hold
          # the release after a write-back tick (v0.24, MPC_BETA_PLANT, the kit's lib): a writing
          # tick that releases nothing fails the interval row at its first release assert and the
          # default row at its kit's-own assert
          timed control_write_release control write-release -- tests -p k2-mpc-base --lib release_tests
          timed plant_write_release_skipped plant write-release-skipped \
            'k2-mpc-base/session::release_tests::a_write_back_tick_releases_once_per_interval
             k2-mpc-base/session::release_tests::the_release_after_a_write_is_off_at_zero_and_the_kits_own_by_default' \
            crates/base/src/session/release_tests.rs "release" MPC_BETA_PLANT=write-release-skipped \
            -- tests -p k2-mpc-base --lib release_tests
          # the undo history at its real size (v0.24, MPC_BETA_PLANT, the kit's lib): a batch kept
          # as built (spare slots) fails the slot row at its slot assert
          timed control_undo_budget control undo-budget -- tests -p k2-mpc-base --lib undo_budget_tests
          timed plant_undo_batch_unshrunk plant undo-batch-unshrunk \
            'k2-mpc-base/session::undo_budget_tests::a_kept_batch_holds_only_what_it_needs' \
            crates/base/src/session/undo_budget_tests.rs "in one slot" MPC_BETA_PLANT=undo-batch-unshrunk \
            -- tests -p k2-mpc-base --lib undo_budget_tests
          # the write locks' bounded order (MPC_BETA_PLANT, the kit's lib): the session lock taken
          # as it comes free, whoever is first, fails the overtake row at its overtake assert;
          # a person's call in the agents' lane fails it at its person assert (the timed typist
          # rows and the store rows are left out of these runs)
          timed control_fair_lock control fair-lock -- tests -p k2-mpc-base --lib fair_lock_tests::overtakes_a_person_behind_31_agents_at_most
          timed plant_session_lock_barging plant session-lock-barging \
            'k2-mpc-base/session::fair_lock_tests::overtakes_a_person_behind_31_agents_at_most_once_each' \
            crates/base/src/session/fair_lock_tests.rs "overtaken" MPC_BETA_PLANT=session-lock-barging \
            -- tests -p k2-mpc-base --lib fair_lock_tests::overtakes_a_person_behind_31_agents_at_most
          timed plant_fair_lane_skipped plant fair-lane-skipped \
            'k2-mpc-base/session::fair_lock_tests::overtakes_a_person_behind_31_agents_at_most_once_each' \
            crates/base/src/session/fair_lock_tests.rs "overtaken" MPC_BETA_PLANT=fair-lane-skipped \
            -- tests -p k2-mpc-base --lib fair_lock_tests::overtakes_a_person_behind_31_agents_at_most
          # the store's writer turn (MPC_BETA_PLANT, the kit's lib): the turn skipped (the store
          # library's own writer lock lets in whoever is first) fails the 31-docs row at its
          # person assert
          timed control_store_turn control store-turn -- tests -p k2-mpc-base --lib fair_lock_tests::overtakes_a_person_behind_31_agents_on_31
          timed plant_store_gate_barging plant store-gate-barging \
            'k2-mpc-base/session::fair_lock_tests::overtakes_a_person_behind_31_agents_on_31_other_docs' \
            crates/base/src/session/fair_lock_tests.rs "overtaken" MPC_BETA_PLANT=store-gate-barging \
            -- tests -p k2-mpc-base --lib fair_lock_tests::overtakes_a_person_behind_31_agents_on_31
          # the session-owned texts (v0.24, MPC_BETA_PLANT, the kit's lib): an edit planned on a
          # stale copy of the session's text fails the lockstep against the engine's cache at
          # its first comparison
          timed control_owned_texts control owned-texts -- tests -p k2-mpc-base --lib owned_texts
          timed plant_session_text_stale plant session-text-stale \
            'k2-mpc-base/session::writeback_diff_tests::the_session_owned_texts_match_the_engines_cache' \
            crates/base/src/session/writeback_diff_tests.rs "differ" MPC_BETA_PLANT=session-text-stale \
            -- tests -p k2-mpc-base --lib owned_texts
          # the shared texts (v0.24, MPC_BETA_PLANT, the kit's lib): an edit of a shared live
          # text that reaches its other holders (the disk replica's, the read bases') fails the
          # rows that check those holders after sharing, each at its own assertion
          timed control_shared_texts control shared-texts -- tests -p k2-mpc-base --lib shared_texts
          timed plant_shared_text_written_in_place plant shared-text-written-in-place \
            'k2-mpc-base/session::writeback_diff_tests::shared_texts_are_one_allocation_until_an_edit
             k2-mpc-base/session::writeback_diff_tests::an_edit_after_shared_texts_leaves_the_replica_and_read_base_as_they_were
             k2-mpc-base/session::writeback_diff_tests::outside_saves_after_shared_texts_match_the_decoded_replica' \
            crates/base/src/session/writeback_diff_tests.rs "after sharing" MPC_BETA_PLANT=shared-text-written-in-place \
            -- tests -p k2-mpc-base --lib shared_texts
          # the store's plants (MPC_STORE_PLANT, test builds of the kit's lib): the cache size not
          # applied (the store library's default, 1 GiB) fails the cache rows that read the
          # library's evictions, each at its own assertion; the open with a wait letting the
          # store it found free go before opening it again fails the race row
          timed control_store_cache control store-cache -- tests -p k2-mpc-base --lib store::cache_tests
          timed plant_store_cache_ignored plant store-cache-ignored \
            'k2-mpc-base/store::cache_tests::the_cache_size_is_applied
             k2-mpc-base/store::cache_tests::a_rewrite_keeps_the_cache_size
             k2-mpc-base/store::cache_tests::an_open_with_a_wait_applies_the_cache_size' \
            crates/base/src/store/cache_tests.rs '(evictions 0)' MPC_STORE_PLANT=store-cache-ignored \
            -- tests -p k2-mpc-base --lib store::cache_tests
          # the person-writer row's retired bit (MPC_BETA_PLANT, the kit's lib): written without it,
          # the codec row fails at its retired round trip (the previous release's side of the same
          # plant is tools/n1/n1.sh node-retired's)
          timed control_store_rows control store-rows -- tests -p k2-mpc-base --lib store::tests
          timed plant_retired_bit_dropped plant retired-bit-dropped \
            'k2-mpc-base/store::tests::person_writer_rows' \
            crates/base/src/store/tests.rs 'a retired row keeps the rest of its row' MPC_BETA_PLANT=retired-bit-dropped \
            -- tests -p k2-mpc-base --lib -E 'test(=store::tests::person_writer_rows)'
          # a log entry of another engine than its doc's (MPC_BETA_PLANT, the kit's lib): the
          # store's check skipped (any entry logged, as before) fails the row of every log write
          # at its first refusal, the append's
          timed plant_append_engine_check_skipped plant append-engine-check-skipped \
            'k2-mpc-base/store::tests::an_entry_of_another_engine_is_refused_at_every_log_write' \
            crates/base/src/store/tests.rs 'appended to a Yrs doc is refused' MPC_BETA_PLANT=append-engine-check-skipped \
            -- tests -p k2-mpc-base --lib -E 'test(=store::tests::an_entry_of_another_engine_is_refused_at_every_log_write)'
          # an engine stamp of the other entry engine over a doc's rows (MPC_BETA_PLANT, the
          # kit's lib): the store's check skipped (any restamp written, as before) fails the
          # restamp row at its first refusal, put_meta's
          timed plant_restamp_engine_check_skipped plant restamp-engine-check-skipped \
            'k2-mpc-base/store::tests::an_engine_restamp_under_a_log_is_refused' \
            crates/base/src/store/tests.rs "a native stamp over a Yrs doc's log is refused" MPC_BETA_PLANT=restamp-engine-check-skipped \
            -- tests -p k2-mpc-base --lib -E 'test(=store::tests::an_engine_restamp_under_a_log_is_refused)'
          timed control_store_lock control store-lock -- tests -p k2-mpc-base --lib store::lock_tests
          timed plant_retry_probe_then_open plant retry-probe-then-open \
            'k2-mpc-base/store::lock_tests::an_open_with_a_wait_keeps_the_store_it_found_free' \
            crates/base/src/store/lock_tests.rs 'the open with a wait opens' MPC_STORE_PLANT=retry-probe-then-open \
            -- tests -p k2-mpc-base --lib store::lock_tests
          # a rewrite that leaves the first blob_verbs row out (MPC_STORE_PLANT, the kit's lib)
          # fails the row that holds every blob_verbs row to its bytes, at that row's assert
          timed control_store_verbs control store-verbs -- tests -p k2-mpc-base --lib store::verbs::tests
          timed plant_rewrite_drops_blob_verbs_row plant rewrite-drops-blob-verbs-row \
            'k2-mpc-base/store::verbs::tests::a_rewrite_keeps_every_blob_verbs_row_byte_for_byte' \
            crates/base/src/store/verbs.rs 'byte for byte' MPC_STORE_PLANT=rewrite-drops-blob-verbs-row \
            -- tests -p k2-mpc-base --lib store::verbs::tests
          # grouped commits (MPC_BETA_PLANT, the kit's lib): each plant alone fails its row of
          # session/grouped_commit_tests.rs at that row's own assertion
          timed control_grouped control grouped -- tests -p k2-mpc-base --lib session::grouped_commit_tests
          timed plant_g_durable_claimed_early plant durable-claimed-early \
            'k2-mpc-base/session::grouped_commit_tests::grouped_commits_survive_a_kill_as_a_prefix' \
            crates/base/src/session/grouped_commit_tests.rs 'every change known durable is kept' MPC_BETA_PLANT=durable-claimed-early \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::grouped_commits_survive_a_kill_as_a_prefix
          timed plant_g_write_back_skips_barrier plant write-back-skips-barrier \
            'k2-mpc-base/session::grouped_commit_tests::a_write_back_is_never_ahead_of_the_store' \
            crates/base/src/session/grouped_commit_tests.rs 'ahead of the store' MPC_BETA_PLANT=write-back-skips-barrier \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::a_write_back_is_never_ahead_of_the_store
          timed plant_g_group_changes_ignored plant group-changes-ignored \
            'k2-mpc-base/session::grouped_commit_tests::the_group_bound_holds' \
            crates/base/src/session/grouped_commit_tests.rs 'the count bound' MPC_BETA_PLANT=group-changes-ignored \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::the_group_bound_holds
          timed plant_g_group_ms_ignored plant group-ms-ignored \
            'k2-mpc-base/session::grouped_commit_tests::the_group_bound_holds' \
            crates/base/src/session/grouped_commit_tests.rs 'the time bound at the tick' MPC_BETA_PLANT=group-ms-ignored \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::the_group_bound_holds
          timed plant_g_rollback_keeps_live_docs plant rollback-keeps-live-docs \
            'k2-mpc-base/session::grouped_commit_tests::a_failed_durable_commit_reloads_every_live_doc' \
            crates/base/src/session/grouped_commit_tests.rs 'left RolledBack' MPC_BETA_PLANT=rollback-keeps-live-docs \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::a_failed_durable_commit_reloads_every_live_doc
          timed plant_g_rollback_skips_yrs plant rollback-skips-yrs \
            'k2-mpc-base/session::grouped_commit_tests::a_failed_durable_commit_reloads_every_live_doc' \
            crates/base/src/session/grouped_commit_tests.rs 'left RolledBack' MPC_BETA_PLANT=rollback-skips-yrs \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::a_failed_durable_commit_reloads_every_live_doc
          timed plant_g_no_rotation_after_recovery plant no-rotation-after-recovery \
            'k2-mpc-base/session::grouped_commit_tests::a_lost_version_is_refused_and_no_id_is_reissued' \
            crates/base/src/session/grouped_commit_tests.rs 'writer after the loss is fresh' MPC_BETA_PLANT=no-rotation-after-recovery \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::a_lost_version_is_refused_and_no_id_is_reissued
          timed plant_g_seq_floor_ignored plant seq-floor-ignored \
            'k2-mpc-base/session::grouped_commit_tests::a_lost_version_is_refused_and_no_id_is_reissued' \
            crates/base/src/session/grouped_commit_tests.rs 'is refused base_expired' MPC_BETA_PLANT=seq-floor-ignored \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::a_lost_version_is_refused_and_no_id_is_reissued
          timed plant_g_blob_floor_ignored plant blob-floor-ignored \
            'k2-mpc-base/session::grouped_commit_tests::a_lost_blob_version_is_refused_and_no_blob_version_is_reissued' \
            crates/base/src/session/grouped_commit_tests.rs 'is refused base_expired' MPC_BETA_PLANT=blob-floor-ignored \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::a_lost_blob_version_is_refused_and_no_blob_version_is_reissued
          timed plant_g_barrier_skipped_flush plant barrier-skipped-flush \
            'k2-mpc-base/session::grouped_commit_tests::barriers_make_the_group_durable' \
            crates/base/src/session/grouped_commit_tests.rs 'the barrier flush makes every commit before it durable' MPC_BETA_PLANT=barrier-skipped-flush \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::barriers_make_the_group_durable
          timed plant_g_barrier_skipped_close plant barrier-skipped-close \
            'k2-mpc-base/session::grouped_commit_tests::barriers_make_the_group_durable' \
            crates/base/src/session/grouped_commit_tests.rs 'the barrier close makes every commit before it durable' MPC_BETA_PLANT=barrier-skipped-close \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::barriers_make_the_group_durable
          timed plant_g_barrier_skipped_unload plant barrier-skipped-unload \
            'k2-mpc-base/session::grouped_commit_tests::barriers_make_the_group_durable' \
            crates/base/src/session/grouped_commit_tests.rs 'the barrier unload makes every commit before it durable' MPC_BETA_PLANT=barrier-skipped-unload \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::barriers_make_the_group_durable
          timed plant_g_barrier_skipped_release plant barrier-skipped-release \
            'k2-mpc-base/session::grouped_commit_tests::barriers_make_the_group_durable' \
            crates/base/src/session/grouped_commit_tests.rs 'the barrier release makes every commit before it durable' MPC_BETA_PLANT=barrier-skipped-release \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::barriers_make_the_group_durable
          timed plant_g_barrier_skipped_delete plant barrier-skipped-delete \
            'k2-mpc-base/session::grouped_commit_tests::barriers_make_the_group_durable' \
            crates/base/src/session/grouped_commit_tests.rs 'the barrier delete makes every commit before it durable' MPC_BETA_PLANT=barrier-skipped-delete \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::barriers_make_the_group_durable
          timed plant_g_barrier_skipped_rewrite plant barrier-skipped-rewrite \
            'k2-mpc-base/session::grouped_commit_tests::barriers_make_the_group_durable' \
            crates/base/src/session/grouped_commit_tests.rs 'the barrier rewrite makes every commit before it durable' MPC_BETA_PLANT=barrier-skipped-rewrite \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::barriers_make_the_group_durable
          timed plant_g_barrier_skipped_purge plant barrier-skipped-purge \
            'k2-mpc-base/session::grouped_commit_tests::barriers_make_the_group_durable' \
            crates/base/src/session/grouped_commit_tests.rs 'the barrier purge makes every commit before it durable' MPC_BETA_PLANT=barrier-skipped-purge \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::barriers_make_the_group_durable
          timed plant_g_barrier_skipped_demote plant barrier-skipped-demote \
            'k2-mpc-base/session::grouped_commit_tests::barriers_make_the_group_durable' \
            crates/base/src/session/grouped_commit_tests.rs 'the barrier demote makes every commit before it durable' MPC_BETA_PLANT=barrier-skipped-demote \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::barriers_make_the_group_durable
          timed plant_g_barrier_skipped_promote plant barrier-skipped-promote \
            'k2-mpc-base/session::grouped_commit_tests::barriers_make_the_group_durable' \
            crates/base/src/session/grouped_commit_tests.rs 'the barrier promote makes every commit before it durable' MPC_BETA_PLANT=barrier-skipped-promote \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::barriers_make_the_group_durable
          # the live switch of the durability (SessionManager::set_commit_durability): its commit
          # made an ordinary one (the group left waiting) fails the switch-to-Immediate row at its
          # durable-at-once assert; the setting changed at once, with no flush and no writer held,
          # fails the race row at its switch assert
          timed plant_g_durability_switch_skips_flush plant durability-switch-skips-flush \
            'k2-mpc-base/session::grouped_commit_tests::a_switch_to_immediate_makes_the_group_durable_at_once' \
            crates/base/src/session/grouped_commit_tests.rs 'makes every change before it durable at once' MPC_BETA_PLANT=durability-switch-skips-flush \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::a_switch_to_immediate_makes_the_group_durable_at_once
          timed plant_g_durability_switch_mid_group plant durability-switch-mid-group \
            'k2-mpc-base/session::grouped_commit_tests::a_switch_racing_edits_loses_and_tears_nothing' \
            crates/base/src/session/grouped_commit_tests.rs 'every switch leaves every commit before it durable' MPC_BETA_PLANT=durability-switch-mid-group \
            -- tests -p k2-mpc-base --lib session::grouped_commit_tests::a_switch_racing_edits_loses_and_tears_nothing
          # the native Beta's plants that run without the engine (MPC_BETA_PLANT, the kit's lib):
          # the cut cadence's settings and rules (session/native_beta_tests.rs), the cut queue
          # (session/cut_queue_tests.rs), a native refusal's exit (errors/tests.rs) and the memory
          # release (memory.rs); each alone fails the rows its docs name, at their own assertions
          timed control_beta_cadence control beta-cadence -- tests -p k2-mpc-base --lib session::native_beta_tests
          timed plant_bytes_before_t plant bytes-before-t \
            'k2-mpc-base/session::native_beta_tests::the_cadence_above_30k_waits_for_the_bytes' \
            crates/base/src/session/native_beta_tests.rs 'never due before T(n)' MPC_BETA_PLANT=bytes-before-t \
            -- tests -p k2-mpc-base --lib session::native_beta_tests
          timed plant_bytes_rule_below_30k plant bytes-rule-below-30k \
            'k2-mpc-base/session::native_beta_tests::the_cadence_below_30k_is_the_t_rule' \
            crates/base/src/session/native_beta_tests.rs 'whatever the tail'\''s bytes' MPC_BETA_PLANT=bytes-rule-below-30k \
            -- tests -p k2-mpc-base --lib session::native_beta_tests
          timed plant_bytes_rule_ignored plant bytes-rule-ignored \
            'k2-mpc-base/session::native_beta_tests::forced_stays_live_above_30k
             k2-mpc-base/session::native_beta_tests::the_cadence_above_30k_waits_for_the_bytes' \
            crates/base/src/session/native_beta_tests.rs 'row: n=' MPC_BETA_PLANT=bytes-rule-ignored \
            -- tests -p k2-mpc-base --lib session::native_beta_tests
          timed plant_cadence_unchecked plant cadence-unchecked \
            'k2-mpc-base/session::native_beta_tests::the_cadence_byte_settings_are_checked_and_listed
             k2-mpc-base/session::native_beta_tests::the_cut_cadence_is_checked_and_defaults_to_the_betas' \
            crates/base/src/session/native_beta_tests.rs 'is refused' MPC_BETA_PLANT=cadence-unchecked \
            -- tests -p k2-mpc-base --lib session::native_beta_tests
          timed plant_forced_dead_above_30k plant forced-dead-above-30k \
            'k2-mpc-base/session::native_beta_tests::forced_stays_live_above_30k' \
            crates/base/src/session/native_beta_tests.rs 'forced' MPC_BETA_PLANT=forced-dead-above-30k \
            -- tests -p k2-mpc-base --lib session::native_beta_tests
          timed control_cut_queue control cut-queue -- tests -p k2-mpc-base --lib session::cut_queue_tests
          timed plant_duty_ignored plant duty-ignored \
            'k2-mpc-base/session::cut_queue_tests::the_duty_gap_follows_a_measured_cut' \
            crates/base/src/session/cut_queue_tests.rs 'inside the gap' MPC_BETA_PLANT=duty-ignored \
            -- tests -p k2-mpc-base --lib session::cut_queue_tests
          timed plant_forced_waits_for_idle plant forced-waits-for-idle \
            'k2-mpc-base/session::cut_queue_tests::a_due_doc_is_queued_with_the_cadences_reason
             k2-mpc-base/session::cut_queue_tests::a_forced_cut_runs_while_editing_goes_on' \
            crates/base/src/session/cut_queue_tests.rs 'cut while not idle' MPC_BETA_PLANT=forced-waits-for-idle \
            -- tests -p k2-mpc-base --lib session::cut_queue_tests
          timed plant_idle_ignored plant idle-ignored \
            'k2-mpc-base/session::cut_queue_tests::a_due_doc_is_queued_with_the_cadences_reason
             k2-mpc-base/session::cut_queue_tests::the_queue_waits_for_idle' \
            crates/base/src/session/cut_queue_tests.rs 'row:' MPC_BETA_PLANT=idle-ignored \
            -- tests -p k2-mpc-base --lib session::cut_queue_tests
          timed plant_queue_runs_two plant queue-runs-two \
            'k2-mpc-base/session::cut_queue_tests::the_queue_hands_out_one_cut_at_a_time' \
            crates/base/src/session/cut_queue_tests.rs 'no second cut' MPC_BETA_PLANT=queue-runs-two \
            -- tests -p k2-mpc-base --lib session::cut_queue_tests
          timed plant_refused_dropped plant refused-dropped \
            'k2-mpc-base/session::cut_queue_tests::a_refused_cut_stays_queued_and_is_retried' \
            crates/base/src/session/cut_queue_tests.rs 'stays queued' MPC_BETA_PLANT=refused-dropped \
            -- tests -p k2-mpc-base --lib session::cut_queue_tests
          timed control_native_exits control native-exits -- tests -p k2-mpc-base --lib errors::tests
          timed plant_native_exit_drift plant native-exit-drift \
            'k2-mpc-base/errors::tests::every_variant_maps_to_a_live_id_with_its_exit
             k2-mpc-base/errors::tests::native_refusals_exit_as_their_standard_twins' \
            crates/base/src/errors/tests.rs 'MPC-E721' MPC_BETA_PLANT=native-exit-drift \
            -- tests -p k2-mpc-base --lib errors::tests
          timed control_memory_release control memory-release -- tests -p k2-mpc-base --lib memory::tests
          timed plant_release_skipped plant release-skipped \
            'k2-mpc-base/memory::tests::freed_memory_leaves_the_process' \
            crates/base/src/memory.rs 'the freed memory left the process' MPC_BETA_PLANT=release-skipped \
            -- tests -p k2-mpc-base --lib memory::tests
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
          # the view plants (differential::ErgoMutant, MPC_ERGO_MUTANT): each fails the rows of
          # tests/view_address.rs that read what it breaks, at their own assertions
          timed control_view_address control view-address -- tests -p k2-mpc-base --test view_address
          timed plant_view_dropped plant view-dropped \
            'k2-mpc-base::view_address/edit_result_has_view
             k2-mpc-base::view_address/view_is_the_presence
             k2-mpc-base::view_address/view_on_delete
             k2-mpc-base::view_address/view_3d_is_the_unit
             k2-mpc-base::view_address/view_never_moves_a_person
             k2-mpc-base::view_address/view_time
             k2-mpc-base::view_address/view_address_is_grammar
             k2-mpc-base::view_address/view_text_lines
             k2-mpc-base::view_address/view_markdown_section
             k2-mpc-base::view_address/view_host_run
             k2-mpc-base::view_address/known_limits_of_the_view
             k2-mpc-base::view_address/view_multi_step_undo' \
            crates/base/tests/view_address.rs '' MPC_ERGO_MUTANT=view-dropped \
            -- tests -p k2-mpc-base --test view_address
          timed plant_view_first_container plant view-first-container \
            'k2-mpc-base::view_address/view_is_the_presence' \
            crates/base/tests/view_address.rs 'the view is the presence' MPC_ERGO_MUTANT=view-first-container \
            -- tests -p k2-mpc-base --test view_address
          timed plant_view_deleted_live plant view-deleted-live \
            'k2-mpc-base::view_address/view_on_delete' \
            crates/base/tests/view_address.rs 'a deleted unit is not live' MPC_ERGO_MUTANT=view-deleted-live \
            -- tests -p k2-mpc-base --test view_address
          timed plant_view_moves_person plant view-moves-person \
            'k2-mpc-base::view_address/view_never_moves_a_person' \
            crates/base/tests/view_address.rs "a person's view never moves" MPC_ERGO_MUTANT=view-moves-person \
            -- tests -p k2-mpc-base --test view_address
          timed plant_view_section_outer plant view-section-outer \
            'k2-mpc-base::view_address/view_markdown_section
             k2-mpc-base::view_address/view_host_run' \
            crates/base/tests/view_address.rs '' MPC_ERGO_MUTANT=view-section-outer \
            -- tests -p k2-mpc-base --test view_address
          timed plant_view_address_unchecked plant view-address-unchecked \
            'k2-mpc-base::view_address/view_address_is_grammar' \
            crates/base/tests/view_address.rs 'a view address the grammar refuses is left out' MPC_ERGO_MUTANT=view-address-unchecked \
            -- tests -p k2-mpc-base --test view_address
          # the tier-0 verb path's plants (differential::Tier0VerbMutant, MPC_T0V_MUTANT): each
          # fails the rows of tests/tier0_verbs.rs that read what it breaks, at their own
          # assertions
          timed control_tier0_verbs control tier0-verbs -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_applier_off_by_one plant t0v-applier-off-by-one \
            'k2-mpc-base::tier0_verbs/copy_exact
             k2-mpc-base::tier0_verbs/dry_run_matches_real' \
            crates/base/tests/tier0_verbs.rs "the bytes outside the spans are the input's" MPC_T0V_MUTANT=applier-off-by-one \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_plan_unchecked plant t0v-plan-unchecked \
            'k2-mpc-base::tier0_verbs/plan_checked' \
            crates/base/tests/tier0_verbs.rs 'is refused as a bad plan (E725)' MPC_T0V_MUTANT=plan-unchecked \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_stray_unchecked plant t0v-stray-unchecked \
            'k2-mpc-base::tier0_verbs/strayed_refused' \
            crates/base/tests/tier0_verbs.rs "a neighbour's change refuses E726" MPC_T0V_MUTANT=stray-unchecked \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_output_index_skipped plant t0v-output-index-skipped \
            'k2-mpc-base::tier0_verbs/output_checked' \
            crates/base/tests/tier0_verbs.rs 'an output the index refuses gives E727' MPC_T0V_MUTANT=output-index-skipped \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_units_from_plan plant t0v-units-from-plan \
            'k2-mpc-base::tier0_verbs/units_from_index
             k2-mpc-base::tier0_verbs/dry_run_matches_real' \
            crates/base/tests/tier0_verbs.rs "the changed units are the index diff's" MPC_T0V_MUTANT=units-from-plan \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_lease_only_ignored plant t0v-lease-only-ignored \
            'k2-mpc-base::tier0_verbs/lease_only' \
            crates/base/tests/tier0_verbs.rs "without the caller's own lease, E718" MPC_T0V_MUTANT=lease-only-ignored \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_verb_not_recorded_blob plant t0v-verb-not-recorded-blob \
            'k2-mpc-base::tier0_verbs/verb_recorded' \
            crates/base/tests/tier0_verbs.rs "the verb's record holds" MPC_T0V_MUTANT=verb-not-recorded-blob \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_commit_without_recheck plant t0v-commit-without-recheck \
            'k2-mpc-base::tier0_verbs/head_moved_mid_plan' \
            crates/base/tests/tier0_verbs.rs 'one more plan, then stale (E110)' MPC_T0V_MUTANT=commit-without-recheck \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_claim_unchecked plant t0v-claim-unchecked \
            'k2-mpc-base::tier0_verbs/claims_respected' \
            crates/base/tests/tier0_verbs.rs 'refuses E301' MPC_T0V_MUTANT=claim-unchecked \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_merge_note_missing plant t0v-merge-note-missing \
            'k2-mpc-base::tier0_verbs/merge_bound_shown' \
            crates/base/tests/tier0_verbs.rs 'merge_over_bound on a verb over the bound' MPC_T0V_MUTANT=merge-note-missing \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_commits plant t0v-commits \
            'k2-mpc-base::tier0_verbs/dry_run_matches_real
             k2-mpc-base::tier0_verbs/dry_run_leaves_no_trace
             k2-mpc-base::tier0_verbs/dry_run_pack_is_pure
             k2-mpc-base::tier0_verbs/expect_takes_the_previewed_digest
             k2-mpc-base::tier0_verbs/view_on_blob_doc
             k2-mpc-base::tier0_verbs/verb_word
             k2-mpc-base::tier0_verbs/verbs_only_admits_verbs' \
            crates/base/tests/tier0_verbs.rs 'a dry run commits nothing' MPC_T0V_MUTANT=commits \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_takes_lease plant t0v-takes-lease \
            'k2-mpc-base::tier0_verbs/dry_run_leaves_no_trace' \
            crates/base/tests/tier0_verbs.rs 'a dry run takes no lease' MPC_T0V_MUTANT=takes-lease \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_charges_rate plant t0v-charges-rate \
            'k2-mpc-base::tier0_verbs/dry_run_leaves_no_trace' \
            crates/base/tests/tier0_verbs.rs 'a dry run is charged as a read' MPC_T0V_MUTANT=charges-rate \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_moves_presence plant t0v-moves-presence \
            'k2-mpc-base::tier0_verbs/dry_run_leaves_no_trace' \
            crates/base/tests/tier0_verbs.rs 'a dry run moves no presence' MPC_T0V_MUTANT=moves-presence \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_stores_index plant t0v-stores-index \
            'k2-mpc-base::tier0_verbs/dry_run_leaves_no_trace' \
            crates/base/tests/tier0_verbs.rs 'a dry run emits no event' MPC_T0V_MUTANT=stores-index \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_leaves_put plant t0v-leaves-put \
            'k2-mpc-base::tier0_verbs/dry_run_leaves_no_trace' \
            crates/base/tests/tier0_verbs.rs 'a dry run leaves no open put' MPC_T0V_MUTANT=leaves-put \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_writes_back plant t0v-writes-back \
            'k2-mpc-base::tier0_verbs/dry_run_leaves_no_trace' \
            crates/base/tests/tier0_verbs.rs 'a dry run leaves the file as it was' MPC_T0V_MUTANT=writes-back \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_skips_guards plant t0v-skips-guards \
            'k2-mpc-base::tier0_verbs/dry_run_refuses_alike' \
            crates/base/tests/tier0_verbs.rs 'the dry run refuses as the real run does' MPC_T0V_MUTANT=skips-guards \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_pack_impure plant t0v-pack-impure \
            'k2-mpc-base::tier0_verbs/dry_run_pack_is_pure' \
            crates/base/tests/tier0_verbs.rs 'the pack is pure' MPC_T0V_MUTANT=pack-impure \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_registry_unchecked plant t0v-registry-unchecked \
            'k2-mpc-base::tier0_verbs/registry_rules
             k2-mpc-base::tier0_verbs/verb_word' \
            crates/base/tests/tier0_verbs.rs 'the registry refuses each bad pack' MPC_T0V_MUTANT=registry-unchecked \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_verb_kept_on_erase plant t0v-verb-kept-on-erase \
            'k2-mpc-base::tier0_verbs/verb_recorded' \
            crates/base/tests/tier0_verbs.rs "the verb's record holds" MPC_T0V_MUTANT=verb-kept-on-erase \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_version_made_hidden plant t0v-version-made-hidden \
            'k2-mpc-base::tier0_verbs/verb_version_event' \
            crates/base/tests/tier0_verbs.rs "a verb's version is told as a write's" MPC_T0V_MUTANT=version-made-hidden \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_view_first_node plant t0v-view-first-node \
            'k2-mpc-base::tier0_verbs/view_on_blob_doc' \
            crates/base/tests/tier0_verbs.rs "the view is the last changed node's place" MPC_T0V_MUTANT=view-first-node \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_seed_without_carry plant t0v-seed-without-carry \
            'k2-mpc-base::tier0_verbs/seeded_index_is_the_jobs' \
            crates/base/tests/tier0_verbs.rs 'the seeded index is the job' MPC_T0V_MUTANT=seed-without-carry \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_seed_skipped plant t0v-seed-skipped \
            'k2-mpc-base::tier0_verbs/seeded_index_is_the_jobs
             k2-mpc-base::tier0_verbs/next_verb_needs_no_job' \
            crates/base/tests/tier0_verbs.rs 'the seeded index' MPC_T0V_MUTANT=seed-skipped \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_word_dropped plant t0v-word-dropped \
            'k2-mpc-base::tier0_verbs/verb_word' \
            crates/base/tests/tier0_verbs.rs 'the word is in capabilities and in the result' MPC_T0V_MUTANT=word-dropped \
            -- tests -p k2-mpc-base --test tier0_verbs
          timed plant_t0v_verbs_only_check_in_verb_path plant t0v-verbs-only-check-in-verb-path \
            'k2-mpc-base::tier0_verbs/verbs_only_admits_verbs' \
            crates/base/tests/tier0_verbs.rs 'its verb lands' MPC_T0V_MUTANT=verbs-only-check-in-verb-path \
            -- tests -p k2-mpc-base --test tier0_verbs
          # the reads across an outside save (tests/tier0_outside.rs): each plant alone
          timed control_tier0_outside control tier0-outside -- tests -p k2-mpc-base --test tier0_outside
          timed plant_t0o_reuse_reads_attached plant t0o-reuse-reads-attached \
            'k2-mpc-base::tier0_outside/outside_reuse_save
             k2-mpc-base::tier0_outside/outside_stored_state_matches_read
             k2-mpc-base::tier0_outside/outside_minted_keys_stored_matches_read' \
            crates/base/tests/tier0_outside.rs 'a reused key reads in doubt' MPC_T0V_MUTANT=reuse-reads-attached \
            -- tests -p k2-mpc-base --test tier0_outside
          timed plant_t0o_judge_against_fingerprint plant t0o-judge-against-fingerprint \
            'k2-mpc-base::tier0_outside/outside_two_honest_edits
             k2-mpc-base::tier0_outside/outside_stored_state_matches_read' \
            crates/base/tests/tier0_outside.rs 'read no doubt' MPC_T0V_MUTANT=judge-against-fingerprint \
            -- tests -p k2-mpc-base --test tier0_outside
          timed plant_t0o_keys_kept_uncounted plant t0o-keys-kept-uncounted \
            'k2-mpc-base::tier0_outside/outside_churn_save' \
            crates/base/tests/tier0_outside.rs 'keys_kept 0 of 3' MPC_T0V_MUTANT=keys-kept-uncounted \
            -- tests -p k2-mpc-base --test tier0_outside
          timed plant_t0o_blame_label_missing plant t0o-blame-label-missing \
            'k2-mpc-base::tier0_outside/outside_kit_write_between' \
            crates/base/tests/tier0_outside.rs 'labels the rows that end on an outside save' MPC_T0V_MUTANT=blame-label-missing \
            -- tests -p k2-mpc-base --test tier0_outside
          timed plant_t0o_reissues_answered plant t0o-reissues-answered \
            'k2-mpc-base::tier0_outside/outside_reissues_refuses' \
            crates/base/tests/tier0_outside.rs 'refuses E722' MPC_T0V_MUTANT=reissues-answered \
            -- tests -p k2-mpc-base --test tier0_outside
          timed plant_t0o_stored_judge_by_fingerprint plant t0o-stored-judge-by-fingerprint \
            'k2-mpc-base::tier0_outside/outside_stored_state_matches_read
             k2-mpc-base::tier0_outside/outside_minted_keys_stored_matches_read' \
            crates/base/tests/tier0_outside.rs "the recorded state (and its event) is the read's" MPC_T0V_MUTANT=stored-judge-by-fingerprint \
            -- tests -p k2-mpc-base --test tier0_outside
          timed plant_t0o_stored_skips_unminted plant t0o-stored-skips-unminted \
            'k2-mpc-base::tier0_outside/outside_minted_keys_stored_matches_read' \
            crates/base/tests/tier0_outside.rs 'across a version whose keys were never recorded' MPC_T0V_MUTANT=stored-skips-unminted \
            -- tests -p k2-mpc-base --test tier0_outside
          # per-node --base, two agents and the seed gap (tests/tier0_base.rs): each plant alone
          timed control_tier0_base control tier0-base -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_base_whole_doc plant t0b-base-whole-doc \
            'k2-mpc-base::tier0_base/base_lands_untouched
             k2-mpc-base::tier0_base/base_refuses_moved
             k2-mpc-base::tier0_base/base_field_level
             k2-mpc-base::tier0_base/base_selector_at_v
             k2-mpc-base::tier0_base/base_created_keys_never_conflict
             k2-mpc-base::tier0_base/base_unproven_writer
             k2-mpc-base::tier0_base/weak_key_if_version
             k2-mpc-base::tier0_base/base_dry_run_refuses_alike
             k2-mpc-base::tier0_base/two_agents_one_drawing
             k2-mpc-base::tier0_base/swarm_on_one_doc
             k2-mpc-base::tier0_base/base_container_children_alone
             k2-mpc-base::tier0_base/base_container_own_change_refused' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=base-whole-doc \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_base_ignored plant t0b-base-ignored \
            'k2-mpc-base::tier0_base/base_refuses_moved
             k2-mpc-base::tier0_base/base_selector_at_v
             k2-mpc-base::tier0_base/base_dry_run_refuses_alike
             k2-mpc-base::tier0_base/two_agents_one_drawing
             k2-mpc-base::tier0_base/swarm_on_one_doc
             k2-mpc-base::tier0_base/base_container_own_change_refused' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=base-ignored \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_base_node_level_only plant t0b-base-node-level-only \
            'k2-mpc-base::tier0_base/base_field_level
             k2-mpc-base::tier0_base/base_refuses_moved
             k2-mpc-base::tier0_base/two_agents_one_drawing
             k2-mpc-base::tier0_base/swarm_on_one_doc
             k2-mpc-base::tier0_base/base_container_children_alone' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=base-node-level-only \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_base_head_only plant t0b-base-head-only \
            'k2-mpc-base::tier0_base/base_selector_at_v
             k2-mpc-base::tier0_base/two_agents_one_drawing' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=base-head-only \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_created_keys_conflict plant t0b-created-keys-conflict \
            'k2-mpc-base::tier0_base/base_created_keys_never_conflict
             k2-mpc-base::tier0_base/two_agents_one_drawing' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=created-keys-conflict \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_pairing_assumed plant t0b-pairing-assumed \
            'k2-mpc-base::tier0_base/base_unproven_writer
             k2-mpc-base::tier0_base/base_dry_run_refuses_alike' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=pairing-assumed \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_weak_key_guarded plant t0b-weak-key-guarded \
            'k2-mpc-base::tier0_base/weak_key_if_version
             k2-mpc-base::tier0_base/base_dry_run_refuses_alike' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=weak-key-guarded \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_plan_on_stale_head plant t0b-plan-on-stale-head \
            'k2-mpc-base::tier0_base/base_refuses_moved
             k2-mpc-base::tier0_base/base_selector_at_v
             k2-mpc-base::tier0_base/base_dry_run_refuses_alike
             k2-mpc-base::tier0_base/two_agents_one_drawing
             k2-mpc-base::tier0_base/swarm_on_one_doc
             k2-mpc-base::tier0_base/base_container_own_change_refused' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=plan-on-stale-head \
            -- tests -p k2-mpc-base --test tier0_base
          # a container's own record (index row format 3): the old count (every change to a
          # container its own) and the opposite fault (its own record not asked)
          timed plant_t0b_container_always_counted plant t0b-container-always-counted \
            'k2-mpc-base::tier0_base/base_container_children_alone' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=container-always-counted \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_container_never_counted plant t0b-container-never-counted \
            'k2-mpc-base::tier0_base/base_container_own_change_refused' \
            crates/base/tests/tier0_base.rs '--base per node' MPC_T0V_MUTANT=container-never-counted \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_seed_gap_loses plant t0b-seed-gap-loses \
            'k2-mpc-base::tier0_base/seed_gap_race
             k2-mpc-base::tier0_base/wait_and_retry_hides_gap' \
            crates/base/tests/tier0_base.rs 'never a lost edit' MPC_T0V_MUTANT=seed-gap-loses \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_reach_ignored plant t0b-reach-ignored \
            'k2-mpc-base::tier0_base/hold_layer_covers_members
             k2-mpc-base::tier0_base/hold_block_covers_subtree
             k2-mpc-base::tier0_base/two_agents_one_drawing' \
            crates/base/tests/tier0_base.rs "a hold's reach" MPC_T0V_MUTANT=reach-ignored \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_reach_at_hold_time plant t0b-reach-at-hold-time \
            'k2-mpc-base::tier0_base/hold_layer_covers_members' \
            crates/base/tests/tier0_base.rs "a hold's reach" MPC_T0V_MUTANT=reach-at-hold-time \
            -- tests -p k2-mpc-base --test tier0_base
          timed plant_t0b_lease_held_whole_verb plant t0b-lease-held-whole-verb \
            'k2-mpc-base::tier0_base/swarm_on_one_doc
             k2-mpc-base::tier0_base/two_agents_one_drawing' \
            crates/base/tests/tier0_base.rs 'no refusal for overlap in time' MPC_T0V_MUTANT=lease-held-whole-verb \
            -- tests -p k2-mpc-base --test tier0_base
          # a hold binds an agent's whole-file write (tests/tier0_holds_write.rs): each plant alone
          timed control_tier0_holds_write control tier0-holds-write -- tests -p k2-mpc-base --test tier0_holds_write
          timed plant_t0h_whole_write_skips_holds plant t0h-whole-write-skips-holds \
            'k2-mpc-base::tier0_holds_write/whole_write_respects_holds
             k2-mpc-base::tier0_holds_write/whole_write_hold_reach' \
            crates/base/tests/tier0_holds_write.rs 'binds a whole-file write' MPC_T0V_MUTANT=whole-write-skips-holds \
            -- tests -p k2-mpc-base --test tier0_holds_write
          timed plant_t0h_hold_during_upload_missed plant t0h-hold-during-upload-missed \
            'k2-mpc-base::tier0_holds_write/whole_write_hold_during_upload' \
            crates/base/tests/tier0_holds_write.rs 'while the write uploads' MPC_T0V_MUTANT=hold-during-upload-missed \
            -- tests -p k2-mpc-base --test tier0_holds_write
          # a tier-0 batch (tests/tier0_batch.rs): each plant alone fails the rows that read what
          # it breaks, at their own assertions
          timed control_tier0_batch control tier0-batch -- tests -p k2-mpc-base --test tier0_batch
          timed plant_t0batch_commits_each plant t0batch-commits-each \
            'k2-mpc-base::tier0_batch/batch_one_version
             k2-mpc-base::tier0_batch/batch_all_or_nothing
             k2-mpc-base::tier0_batch/batch_strayed_union
             k2-mpc-base::tier0_batch/batch_base_per_node
             k2-mpc-base::tier0_batch/batch_claims_union
             k2-mpc-base::tier0_batch/batch_dry_run_matches_real
             k2-mpc-base::tier0_batch/batch_caps
             k2-mpc-base::tier0_batch/batch_seeds_index
             k2-mpc-base::tier0_batch/batch_race
             k2-mpc-base::tier0_batch/a_batch_commit_takes_the_group_durability' \
            crates/base/tests/tier0_batch.rs 'batch_' MPC_T0V_MUTANT=batch-commits-each \
            -- tests -p k2-mpc-base --test tier0_batch
          timed plant_t0batch_plans_on_head plant t0batch-plans-on-head \
            'k2-mpc-base::tier0_batch/batch_sees_staged
             k2-mpc-base::tier0_batch/batch_exact
             k2-mpc-base::tier0_batch/batch_base_per_node
             k2-mpc-base::tier0_batch/batch_dry_run_matches_real
             k2-mpc-base::tier0_batch/batch_race' \
            crates/base/tests/tier0_batch.rs 'batch_' MPC_T0V_MUTANT=batch-plans-on-head \
            -- tests -p k2-mpc-base --test tier0_batch
          timed plant_t0batch_compose_wrong plant t0batch-compose-wrong \
            'k2-mpc-base::tier0_batch/batch_sees_staged
             k2-mpc-base::tier0_batch/batch_exact
             k2-mpc-base::tier0_batch/batch_base_per_node
             k2-mpc-base::tier0_batch/batch_dry_run_matches_real
             k2-mpc-base::tier0_batch/batch_race' \
            crates/base/tests/tier0_batch.rs 'batch_' MPC_T0V_MUTANT=batch-compose-wrong \
            -- tests -p k2-mpc-base --test tier0_batch
          timed plant_t0batch_base_last_call_only plant t0batch-base-last-call-only \
            'k2-mpc-base::tier0_batch/batch_base_per_node' \
            crates/base/tests/tier0_batch.rs '--base is checked per node over every call' MPC_T0V_MUTANT=batch-base-last-call-only \
            -- tests -p k2-mpc-base --test tier0_batch
          timed plant_t0batch_claims_first_call plant t0batch-claims-first-call \
            'k2-mpc-base::tier0_batch/batch_claims_union' \
            crates/base/tests/tier0_batch.rs 'a hold any call reaches refuses' MPC_T0V_MUTANT=batch-claims-first-call \
            -- tests -p k2-mpc-base --test tier0_batch
          timed plant_t0batch_lease_whole_run plant t0batch-lease-whole-run \
            'k2-mpc-base::tier0_batch/batch_lease_rules
             k2-mpc-base::tier0_batch/batch_race' \
            crates/base/tests/tier0_batch.rs 'batch_' MPC_T0V_MUTANT=batch-lease-whole-run \
            -- tests -p k2-mpc-base --test tier0_batch
          timed plant_t0batch_stray_unchecked plant t0batch-stray-unchecked \
            'k2-mpc-base::tier0_batch/batch_strayed_union' \
            crates/base/tests/tier0_batch.rs 'a node no call declared refuses the batch' MPC_T0V_MUTANT=stray-unchecked \
            -- tests -p k2-mpc-base --test tier0_batch
          # own records under the kit's offset format (tests/blob_index_v2.rs, MPC_INDEX_MUTANT):
          # with the pack's own format used instead, the same format without own digests (an
          # older kit's) reads the row of format 3
          timed control_blob_index_v2 control blob-index-v2 -- tests -p k2-mpc-base --test blob_index_v2
          timed plant_own_offset_skipped plant own-offset-skipped \
            'k2-mpc-base::blob_index_v2/own_records_live_out_of_an_older_kits_reach' \
            crates/base/tests/blob_index_v2.rs "out of an older kit's reach" MPC_INDEX_MUTANT=own-offset-skipped \
            -- tests -p k2-mpc-base --test blob_index_v2
          # a tier-0 revert, undo and redo (tests/tier0_revert.rs): each plant alone fails the
          # rows that read what it breaks, at their own assertions
          timed control_tier0_revert control tier0-revert -- tests -p k2-mpc-base --test tier0_revert
          timed plant_t0rev_revert_takes_whole_version plant t0rev-revert-takes-whole-version \
            'k2-mpc-base::tier0_revert/revert_one_version
             k2-mpc-base::tier0_revert/undo_own_only
             k2-mpc-base::tier0_revert/revert_stray_refused
             k2-mpc-base::tier0_revert/revert_container_children_alone' \
            crates/base/tests/tier0_revert.rs 'revert' MPC_T0V_MUTANT=revert-takes-whole-version \
            -- tests -p k2-mpc-base --test tier0_revert
          timed plant_t0rev_revert_overwrites plant t0rev-revert-overwrites \
            'k2-mpc-base::tier0_revert/revert_conflict_names_who
             k2-mpc-base::tier0_revert/revert_container_own_change_refused' \
            crates/base/tests/tier0_revert.rs 'since K refuses' MPC_T0V_MUTANT=revert-overwrites \
            -- tests -p k2-mpc-base --test tier0_revert
          timed plant_t0rev_container_always_counted plant t0rev-container-always-counted \
            'k2-mpc-base::tier0_revert/revert_container_children_alone' \
            crates/base/tests/tier0_revert.rs 'revert_container_children_alone' MPC_T0V_MUTANT=container-always-counted \
            -- tests -p k2-mpc-base --test tier0_revert
          timed plant_t0rev_container_never_counted plant t0rev-container-never-counted \
            'k2-mpc-base::tier0_revert/revert_container_own_change_refused' \
            crates/base/tests/tier0_revert.rs 'revert_container_own_change_refused' MPC_T0V_MUTANT=container-never-counted \
            -- tests -p k2-mpc-base --test tier0_revert
          timed plant_t0rev_revert_ignores_author plant t0rev-revert-ignores-author \
            'k2-mpc-base::tier0_revert/revert_needs_author_or_owner' \
            crates/base/tests/tier0_revert.rs 'revert_needs_author_or_owner' MPC_T0V_MUTANT=revert-ignores-author \
            -- tests -p k2-mpc-base --test tier0_revert
          timed plant_t0rev_undo_latest_any plant t0rev-undo-latest-any \
            'k2-mpc-base::tier0_revert/undo_own_only' \
            crates/base/tests/tier0_revert.rs 'undo_own_only' MPC_T0V_MUTANT=undo-latest-any \
            -- tests -p k2-mpc-base --test tier0_revert
          timed plant_t0rev_redo_not_cleared plant t0rev-redo-not-cleared \
            'k2-mpc-base::tier0_revert/redo_cleared_by_new_edit' \
            crates/base/tests/tier0_revert.rs 'redo_cleared_by_new_edit' MPC_T0V_MUTANT=redo-not-cleared \
            -- tests -p k2-mpc-base --test tier0_revert
          timed plant_t0rev_revert_skips_claims plant t0rev-revert-skips-claims \
            'k2-mpc-base::tier0_revert/revert_held_refused' \
            crates/base/tests/tier0_revert.rs 'revert_held_refused' MPC_T0V_MUTANT=revert-skips-claims \
            -- tests -p k2-mpc-base --test tier0_revert
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
