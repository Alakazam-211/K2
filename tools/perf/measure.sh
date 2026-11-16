# The kit's measurements: the #[ignore]d tests whose figures a design note or an evidence note
# cites, one cell per job, in release, on a node. A commands file for tools/job.sh, CELL picks one:
#   k2 compute run --repo <repo> --needs rust@stable --cpus 4 --mem 20G \
#     --env CELL=blame-sizes -- sh tools/job.sh tools/perf/measure.sh
# Each cell prints its command, builds, then runs exactly its test (`ran`: a filter matching no
# test fails the cell). A measurement prints figures; it passes or fails only on its own checks.
#   blame-sizes     tests/blame_cost.rs blame_cost_at_sizes (docs/evidence/v0.9.7-blame.md)
#   blame-check     tests/blame_cost.rs blame_check_cost (docs/evidence/v0.9.7-blame.md)
#   edit-sizes      tests/edit_cost.rs edit_cost_at_sizes (docs/evidence/v0.9.3-edit-path.md;
#                   EDIT_COST_SIZES)
#   chunked-timing  tests/chunked_timing.rs a_history_only_chunk_against_a_brand_new_one_wall_time
#                   (docs/evidence/chunked-write-timing.md; MPC_TIMING_WRITES, MPC_TIMING_SEED)
#   removal         tests/outside_removal_notes.rs removal_cost_on_a_whole_document_delete
#                   (docs/evidence/v0.9.7.2-outside-removal.md)
#   perf-kit        tests/perf_kit.rs perf_kit_at_sizes (docs/evidence/perf-kit-v0.9.8.md; PERF_SIZES)
#   carry           the lib's revert_carry_per_apply_r1e5 (docs/SESSIONS.md, the revert carry)
#   writers-bytes   tests/person_writers_cost.rs measure_bytes_per_writer
#   writers-latency tests/person_writers_cost.rs measure_join_leave_latency (MPC_PW_LAT_N)
#   zip-shape       tests/version_storage.rs measure_zip_shape (docs/design/version-storage-r2.md,
#                   section 13: the deck-shaped series)
#   vs-dir          tests/version_storage.rs measure_version_dir over the versions in VS_DIR
#                   (docs/design/version-storage-r2.md, sections 14 and 15)
#   vs-samples      tests/version_storage.rs measure_edited_samples over the samples in VS_SAMPLES
#                   (the same sections)
# vs-dir and vs-samples run only where their files are: the job that holds them sets the
# folder, and the cell fails without it. They print counts and sizes only, never a path, a
# file's name or its content.
settings CELL EDIT_COST_SIZES PERF_SIZES MPC_TIMING_WRITES MPC_TIMING_SEED MPC_PW_LAT_N VS_EDITS VS_MIN_FILE
# the person-writer measurements' features: the native engine's leg runs with them
PW=""
# one CELL: TARGET TEST (TARGET a test binary's name, or --lib)
case ${CELL:?CELL: blame-sizes, blame-check, edit-sizes, chunked-timing, removal, perf-kit, carry, writers-bytes, writers-latency, zip-shape, vs-dir or vs-samples} in
  blame-sizes) T="--test blame_cost"; N=blame_cost_at_sizes ;;
  blame-check) T="--test blame_cost"; N=blame_check_cost ;;
  edit-sizes) T="--test edit_cost"; N=edit_cost_at_sizes ;;
  chunked-timing) T="--test chunked_timing"; N=a_history_only_chunk_against_a_brand_new_one_wall_time ;;
  removal) T="--test outside_removal_notes"; N=removal_cost_on_a_whole_document_delete ;;
  perf-kit) T="--test perf_kit"; N=perf_kit_at_sizes ;;
  carry) T="--lib"; N=revert_carry_per_apply_r1e5 ;;
  writers-bytes) T="$PW --test person_writers_cost"; N=measure_bytes_per_writer ;;
  writers-latency) T="$PW --test person_writers_cost"; N=measure_join_leave_latency ;;
  zip-shape) T="--test version_storage"; N=measure_zip_shape ;;
  vs-dir) [ -n "${VS_DIR:-}" ] || { echo "measure: vs-dir needs VS_DIR, the folder of versions"; exit 2; }
          T="--test version_storage"; N=measure_version_dir ;;
  vs-samples) [ -n "${VS_SAMPLES:-}" ] || { echo "measure: vs-samples needs VS_SAMPLES, the folder of samples"; exit 2; }
          T="--test version_storage"; N=measure_edited_samples ;;
  *) echo "unknown CELL $CELL" >&2; exit 2 ;;
esac
echo "measure: cargo test --release -p k2-mpc-base $T $N -- --ignored --nocapture --test-threads 1"
timed build cargo test --release -p k2-mpc-base $T --no-run
timed "$CELL" ran cargo test --release -p k2-mpc-base $T $N -- --ignored --nocapture --test-threads 1
