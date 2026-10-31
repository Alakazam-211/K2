# The grid's (C1) long runs, beside tools/tests/full.sh's parts (which run every grid test at
# CI size): a --repo job (tools/README.md, "Test jobs"), PART picks one:
#   k2 compute run <node> --repo <repo> --needs rust@stable --cpus 4 --mem 8G \
#     --env PART=grid-a -- sh tools/job.sh tools/tests/grid.sh
# Release builds. Trace shards (grid-a, grid-b, grid-m) run on build nodes, never on a slow one.
#   grid-a, grid-b  the grid trace classes at 10^4 traces each (two halves of the classes), and
#                   their mutants at 200 traces per class
#   grid-m          every grid mutant over every class, 300 traces per class
#   pt              the table properties (tests/table.rs) at 10^4 cases each
#   measure         the grid's measurements: a cell set at the size cap, grid_cost
#   nobyte          the no-byte twin (tests/grid_store.rs, no_byte) at 10^4 cases
#   gridident       the re-identification property and the outside-merge property at 10^3
#   gerasure        tests/grid_erasure.rs at 10^3 histories
GA="identical_value_write,sort_vs_foreign_move,delete_foreign_cell_in_hidden_row,insert_then_foreign_cell,move_back_past_deleted_neighbour,redo_after_partial_sort_undo,revert_after_restart,own_lifo_chain,move_back_then_restore_left_neighbour"
GB="own_later_write_off_stack,own_cell_clear_then_undo,already_by_value,drop_between_batch_and_undo,header_switch_partial,undo_across_outside_save,revert_vs_undo,write_with_no_identity,random,row_rewritten_like_another,text_row_both_changed,undo_after_text_pair,revert_after_text_pair,undo_for_a_person,column_and_block_verbs,revert_over_a_drop,partial_write_over_a_column_change"
case ${PART:?PART: grid-a, grid-b, grid-m, pt, measure, nobyte, gridident or gerasure} in
  grid-a|grid-b) C=$( [ "$PART" = grid-a ] && echo "$GA" || echo "$GB" )
        timed build cargo test --release -p k2-mpc-spec-undo --test grid_traces --no-run
        timed grid_traces env GRID_TRACES=10000 GRID_CLASSES="$C" cargo test --release -p k2-mpc-spec-undo --test grid_traces grid_traces_model -- --nocapture
        timed grid_mutants env GRID_MUTANT_TRACES=200 GRID_CLASSES="$C" cargo test --release -p k2-mpc-spec-undo --test grid_traces grid_mutants -- --nocapture ;;
  grid-m) timed build cargo test --release -p k2-mpc-spec-undo --test grid_traces --no-run
        timed grid_mutants_all env GRID_MUTANT_TRACES=300 cargo test --release -p k2-mpc-spec-undo --test grid_traces grid_mutants -- --nocapture ;;
  pt) timed build cargo test --release -p k2-mpc-base --test table --no-run
      timed table_pt_10k env PROPTEST_CASES=10000 cargo test --release -p k2-mpc-base --test table -- --nocapture ;;
  measure) timed build cargo test --release -p k2-mpc-base --test structured_mem --test grid_cost --no-run
        timed cell_set_at_cap cargo test --release -p k2-mpc-base --test structured_mem measure_cell_set_latency_at_the_cap -- --ignored --nocapture --test-threads 1
        timed grid_cost cargo test --release -p k2-mpc-base --test grid_cost -- --ignored --nocapture ;;
  nobyte) timed build cargo test --release -p k2-mpc-base --test grid_store --no-run
        timed nobyte_grid env NO_BYTE_CASES=10000 cargo test --release -p k2-mpc-base --test grid_store no_byte -- --nocapture ;;
  gridident) timed build cargo test --release -p k2-mpc-base --test grid_store --no-run
        timed grid_ident_1k env GRID_IDENT_CASES=1000 cargo test --release -p k2-mpc-base --test grid_store grid_outside_save_after_reidentification -- --nocapture
        timed grid_outside_1k env GRID_OUTSIDE_CASES=1000 cargo test --release -p k2-mpc-base --test grid_store grid_an_outside_save_amid_any_mix -- --nocapture ;;
  gerasure) timed build cargo test --release -p k2-mpc-base --test grid_erasure --no-run
        timed grid_erasure_1000 env GRID_ERASURE_CASES=1000 cargo test --release -p k2-mpc-base --test grid_erasure -- --nocapture ;;
  *) echo "unknown PART $PART" >&2; exit 2 ;;
esac
