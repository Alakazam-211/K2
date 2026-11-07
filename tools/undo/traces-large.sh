# The spec-undo trace test's large runs (the `#[ignore]`d tests of
# crates/spec-undo/tests/traces.rs: the oracle against the kit after every step), a --repo job
# per PART through tools/job.sh (release build; the tests run 4 threads):
#   k2 compute run --repo <repo> --needs rust@stable --cpus 4 --mem 20G \
#     --env PART=group -- sh tools/job.sh tools/undo/traces-large.sh
#   main1..main5  trace_equivalence_large        seeds 1,000,000..1,400,000 in five parts of 80,000
#   d1            d1_traces_large                seeds 2,000,000..2,050,000
#   endbreak      end_break_trace_class_large    seeds 2,000,000..2,050,000 (SPEC_UNDO_TRACES), its
#                                                write --base and end-of-text classes each >= 10^4
#   group         group_trace_equivalence_large  seeds 3,000,000..3,010,000, each grouping class >= 100
# Each run asserts 0 mismatches and its floors: every count at least half the count its measured
# rate gives at the traces the run checks (traces.rs LARGE_MAIN, LARGE_D1, LARGE_GROUP, each with
# its rate job). Part sizes: trace::run measured 74 traces/s on one node (301c3906) and 16.5 on
# another (841f1cba; the same generator, so the spread is the node's); 80,000 traces take about
# 81 min at 16.5/s, inside a 2 h job. SPEC_UNDO_SEEDS=START..END runs any other part of a test's
# seeds (its record is NAME_START_END); `ran` fails the step when the filter or --ignored matched
# no test.
case ${PART:?PART: main1..main5, d1, endbreak or group} in
  main[1-5]) T=trace_equivalence_large; k=${PART#main}
             SPEC_UNDO_SEEDS=$(( 1000000 + (k - 1) * 80000 ))..$(( 1000000 + k * 80000 )); export SPEC_UNDO_SEEDS ;;
  d1) T=d1_traces_large ;;
  group) T=group_trace_equivalence_large ;;
  endbreak) T=end_break_trace_class_large ;;
  *) echo "unknown PART $PART" >&2; exit 2 ;;
esac
settings PART SPEC_UNDO_SEEDS="(the test's own)" SPEC_UNDO_TRACES="(50000: endbreak only)"
echo "traces-large: $T, seeds ${SPEC_UNDO_SEEDS:-the test's own}"
timed build cargo test --release -p k2-mpc-spec-undo --test traces --no-run
timed "$T" ran cargo test --release -p k2-mpc-spec-undo --test traces "$T" -- --ignored --exact --nocapture
