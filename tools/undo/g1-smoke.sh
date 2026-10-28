# The literal G1 bound, sampled (<= 10 min): Quint simulator traces at 3 pids x 3 events,
# 2 undos, 1 revert (maxlen 4, seed abb) replayed into spec_undo::model; random walks on the
# enumerator at the same bound; every mutant on walks.
itfinst g1 3 3 2 1 4 "0, 1, 1" true true none
mkdir -p "$ROOT/itf_g1"; i=0
s=$(date +%s)
while [ $i -lt ${G1_BATCHES:-6} ]; do
  q run g1.qnt --main=g1 --init=winit --step=wstep --max-samples=500 --n-traces=500 --max-steps=12 \
    --seed=$((i + 1)) --out-itf="$ROOT/itf_g1/b${i}_{seq}.itf.json" > /dev/null 2>&1 || echo "quint run batch $i: exit $?"
  i=$((i + 1))
done
echo "=== simulate + ITF at G1: wall $(( $(date +%s) - s ))s"
timed "ITF replay at G1" R --pids 3 --events 3 --undos 2 --reverts 1 --maxlen 4 --init abb "$ROOT/itf_g1"
timed "walks at G1" E walk --g1 --probes ${WALKS:-150000} --threads "$(nproc)"
for m in i-no-taint iii-partial-hides-taint iv-redo-retries-skipped vi-o5-every-window-end vii-o5-region-base skip-u19; do
  timed "walks at G1, mutant $m (must fail)" E walk --g1 --probes 20000 --threads "$(nproc)" --mutant $m
done
