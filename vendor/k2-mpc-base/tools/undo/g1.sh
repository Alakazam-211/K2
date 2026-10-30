# The model check at the G1 bound (docs/MODEL-CHECK.md "The G1 bound"). LONG: launched only
# after review, never as a smoke run. PART picks what runs (each fits a 2 h job on 12 cores):
#   PART=exhaustive (default)  3x1 + 2 undos + 1 revert, then 2x2 + 2 undos + 1 revert, exactly
#   PART=walks                 WALKS (default 5,000,000) random walks at the literal G1 bound,
#                              then every mutant on 50,000 walks (each must fail)
#   PART=long                  3x2 + 2 undos (no revert), exactly: many hours
# The literal G1 bound (3 pids x 3 events, 2 undos, 1 revert) is far beyond exhaustive reach
# (>= 1e13 states even with pid symmetry and per-seed dedup), hence the walks.
# Exhaustive runs use pid symmetry and per-seed fingerprint sets, and write a checkpoint line
# per finished seed (also logged as "ckpt seed K states N max_depth D"). Resume: rebuild
# ck-<name>.txt from the log (the run's header line, then each ckpt line rewritten as
# "seed K states N max_depth D viol -"), put it at the repo root and bundle it:
#   tools/undo-enum-job.sh tools/undo/g1.sh <out dir> ck-<name>.txt
T=${THREADS:-$(nproc)}
run_g() { # run_g NAME ARGS...
  n=$1; shift
  timed "enumerator $n" E dfs "$@" --sym --threads "$T" --checkpoint "$ROOT/ck-$n.txt" --progress 60
}
case ${PART:-exhaustive} in
  exhaustive)
    run_g 3x1u2r1 --pids 3 --events 1 --undos 2 --reverts 1 --init abb --seed-depth 2
    run_g 2x2u2r1 --pids 2 --events 2 --undos 2 --reverts 1 --init abb --seed-depth 3 ;;
  walks)
    timed "walks at the literal G1 bound" E walk --g1 --probes ${WALKS:-5000000} --threads "$T"
    for m in i-no-taint iii-partial-hides-taint iv-redo-retries-skipped vi-o5-every-window-end vii-o5-region-base skip-u19; do
      timed "walks at G1, mutant $m (must fail)" E walk --g1 --probes 50000 --threads "$T" --mutant $m
    done ;;
  long)
    run_g 3x2u2r0 --pids 3 --events 2 --undos 2 --reverts 0 --init abb --seed-depth 3 ;;
  *) echo "PART: exhaustive, walks or long" >&2; exit 2 ;;
esac
