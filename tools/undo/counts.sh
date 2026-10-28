# Exact counts (<= 10 min): TLC's "distinct states found" on undo.qnt at small bounds against
# undo_enum bfs at the same bound (no --sym: both count the same states); then ITF traces at
# 2 pids x 1 event + 1 undo + 1 revert replayed into spec_undo::model.
INV="inv_U9 inv_U13 inv_U15 inv_U17 inv_U19 inv_U22a inv_O5"
inst w 2 1 1 0 4 "0, 1, 1" true true none
timed "warm-up (fetches Apalache)" q verify w.qnt --main=w --backend=tlc --invariant=inv
. "$ROOT/tools/undo/tlc.sh"
# cmp NAME PIDS EVENTS UNDOS REVERTS MAXLEN "INIT" MULTI PURGE INITLETTERS SECONDS
cmp() {
  inst "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8" "$9" none
  tlc "$1" "$INV" "${11}"
  f=""; [ "$8" = false ] && f="$f --no-multi"; [ "$9" = false ] && f="$f --no-purge"
  timed "enumerator $1" E bfs --pids "$2" --events "$3" --undos "$4" --reverts "$5" --maxlen "$6" --init "${10}" $f
}
cmp c_2x1u1     2 1 1 0 4 "0, 1, 1" true  true  abb 120
cmp c_2x1u1r1   2 1 1 1 4 "0, 1, 1" true  true  abb 180
cmp c_3x1u1     3 1 1 0 4 "0, 1, 1" true  true  abb 300
cmp c_2x2u1_l3  2 2 1 0 3 "0, 1"    true  true  ab  240
itfinst itf_r 2 1 1 1 4 "0, 1, 1" true true none
mkdir -p "$ROOT/itf2"
timed "simulate + ITF (2x1 +1 undo +1 revert, 1000 traces)" q run itf_r.qnt --main=itf_r --init=winit --step=wstep \
  --max-samples=1000 --n-traces=1000 --max-steps=4 --out-itf="$ROOT/itf2/t_{seq}.itf.json" --verbosity=1
timed "ITF replay" R --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init abb "$ROOT/itf2"
# every mutant on TLC (one worker: a shortest counterexample) and on the enumerator (the least depth)
for m in i-no-taint iii-partial-hides-taint iv-redo-retries-skipped vi-o5-every-window-end vii-o5-region-base skip-u19; do
  inst m_tiny 2 1 1 0 4 "0, 1, 1" true true $m
  TLC_WORKERS=1 tlc m_tiny "$INV" 120
  timed "enumerator, mutant $m" E bfs --pids 2 --events 1 --undos 1 --reverts 0 --maxlen 4 --init abb --first --mutant $m
done
