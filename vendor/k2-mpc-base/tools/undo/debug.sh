# Debug assertions on (about 12 min on 4 threads): the release jobs build `undo_enum` without
# them, so the oracle's own `debug_assert!`s (theirs rebuilds the window, ours gives before, the
# identity reading equals the v1.7 carry, the last step's state is the request's, the oracle's
# owners are the authors, a root per character, ...) never run in a model job. This one builds
# it with CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true in its own target dir (`target-dbg`, so
# the job's release binaries stay as they are) and runs the oracle of record at the seven
# exact bounds (counts.sh's: the distinct counts must be counts.sh's, no rule violated) and on
# random walks at the literal G1 bound, with and without a restart. A failed assertion is the
# enumerator's abort pseudo-rule, with its message (exit 1). Run as:
#
#   sh tools/undo/job.sh enum tools/undo/debug.sh
#
# (THREADS: the walks' threads, default 4; WALKS: walks per run, default 100,000.)
T=${THREADS:-4}
DT="$ROOT/target-dbg"
s=$(date +%s)
CARGO_TARGET_DIR="$DT" CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true cargo build -q --release --features mutants -p k2-mpc-spec-undo --bins
echo "=== build (debug assertions): wall $(( $(date +%s) - s ))s"
D() { "$DT/release/undo_enum" "$@"; }
for spec in "c_2x1u1:--pids 2 --events 1 --undos 1 --reverts 0 --maxlen 4 --init abb" \
            "c_2x1u1r1:--pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init abb" \
            "c_3x1u1:--pids 3 --events 1 --undos 1 --reverts 0 --maxlen 4 --init abb" \
            "c_1x1u1r1:--pids 1 --events 1 --undos 1 --reverts 1 --maxlen 4 --init ab" \
            "c_2x2u1_l3:--pids 2 --events 2 --undos 1 --reverts 0 --maxlen 3 --init ab" \
            "c_1x3r1s1:--pids 1 --events 3 --undos 0 --reverts 1 --restarts 1 --maxlen 4 --init abba" \
            "c_2x1u2r1:--pids 2 --events 1 --undos 2 --reverts 1 --maxlen 4 --init ab"; do
  n=${spec%%:*}; a=${spec#*:}
  timed "debug assertions: bfs $n" D bfs --run "dbg_$n" $a
done
timed "debug assertions: walks at G1" D walk --g1 --probes ${WALKS:-100000} --threads "$T" --run dbg_walk_g1
timed "debug assertions: walks at G1 + 1 restart" D walk --g1 --restarts 1 --probes ${WALKS:-100000} --threads "$T" --run dbg_walk_g1_rs1
