# The literal G1 bound, sampled (<= 10 min): Quint simulator traces at 3 pids x 3 events,
# 2 undos, 1 revert (maxlen 4, seed abb) replayed into spec_undo::model; random walks on the
# enumerator at the same bound; every mutant on walks.
# The rows (tools/undo/expect.sh); a simulator batch that fails fails the job.
expect_complete itf:itf_pids_3_events_3_undos_2_reverts_1_restarts_0_maxlen_4_init__abb__multi_true_purge_true_reduce_true_u12_false
expect enum:walk_3x3_u2_r1_rs0_none SAMPLED; expect_mutants walk -i1-per-request
# i1-per-request is not caught on walks: its least witness at G1 is six exact steps, which
# uniform walks don't draw. Its row here is the evidence of that, declared SAMPLED (all 20,000
# walks ran, none violated); it is caught exhaustively at its own bound (counts.sh PART=m)
expect enum@i1-per-request SAMPLED
settings G1_BATCHES=6 WALKS=150000 THREADS="$(nproc) (nproc)"
itfinst g1 3 3 2 1 4 "0, 1, 1" true true none
mkdir -p "$ROOT/itf_g1"; i=0
s=$(date +%s)
while [ $i -lt ${G1_BATCHES:-6} ]; do
  q run g1.qnt --main=g1 --init=winit --step=wstep --max-samples=500 --n-traces=500 --max-steps=12 \
    --seed=$((i + 1)) --out-itf="$ROOT/itf_g1/b${i}_{seq}.itf.json" > /dev/null 2>&1 || _expect_fail "quint run batch $i: exit $?"
  i=$((i + 1))
done
echo "=== simulate + ITF at G1: wall $(( $(date +%s) - s ))s"
timed "ITF replay at G1" R --pids 3 --events 3 --undos 2 --reverts 1 --maxlen 4 --init abb "$ROOT/itf_g1"
timed "walks at G1" E walk --g1 --probes ${WALKS:-150000} --threads "$(nproc)"
for m in i-no-taint iii-partial-hides-taint iv-redo-retries-skipped vi-o5-every-window-end vii-o5-region-base skip-u19 f3-rediff; do
  timed "walks at G1, mutant $m (must fail)" E walk --g1 --probes 20000 --threads "$(nproc)" --mutant $m
done
timed "walks at G1 + 1 restart, mutant restart-single-taint (must fail)" E walk --g1 --restarts 1 --probes 20000 --threads "$(nproc)" --mutant restart-single-taint
timed "walks at G1, mutant x-provenance-ignored-in-o4 (must fail)" E walk --g1 --probes 20000 --threads "$(nproc)" --mutant x-provenance-ignored-in-o4
timed "walks at G1, mutant xi-no-o4a (must fail)" E walk --g1 --probes 20000 --threads "$(nproc)" --mutant xi-no-o4a
# the audit's mutants (2026-11-04), at the literal G1 bound too (tools/undo/g1.sh, AUDIT_MUTANTS)
for m in u13-refused-revert-applies r3-redo-rewrites-window r3-revert-rewrites-window; do
  timed "walks at G1, mutant $m (must fail)" E walk --g1 --probes 20000 --threads "$(nproc)" --mutant $m
done
# i1-per-request on the same 20,000 walks: declared SAMPLED above (not drawn on walks)
timed "walks at G1, mutant i1-per-request (not drawn on walks: SAMPLED)" E walk --g1 --probes 20000 --threads "$(nproc)" --mutant i1-per-request
