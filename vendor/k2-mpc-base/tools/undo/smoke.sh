# Smoke (one short job, <= 10 min): typecheck; a tiny simulator run exported as ITF and
# replayed into spec_undo::model; TLC's exact count at a tiny bound against the
# enumerator's; TLC on every mutant at the same bound (each must violate its rule).
B="2 1 1 0 4"            # pids events undos reverts maxlen
I="0, 1, 1"              # the seed text abb
EB="--pids 2 --events 1 --undos 1 --reverts 0 --maxlen 4 --init abb"
timed "typecheck undo" q typecheck undo.qnt
inst u_tiny $B "$I" true true none
timed "typecheck u_tiny" q typecheck u_tiny.qnt
itfinst itf_tiny 2 1 1 1 4 "$I" true true none
mkdir -p "$ROOT/itf"
timed "simulate + ITF (2x1 +1 undo +1 revert, 300 traces)" q run itf_tiny.qnt --main=itf_tiny --init=winit --step=wstep \
  --max-samples=300 --n-traces=300 --max-steps=4 --out-itf="$ROOT/itf/t_{seq}.itf.json" --verbosity=1
timed "ITF replay into spec_undo::model" R --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init abb "$ROOT/itf"
timed "warm-up (fetches Apalache)" q verify u_tiny.qnt --main=u_tiny --backend=tlc --invariant=inv
. "$ROOT/tools/undo/tlc.sh"
INV="inv_U9 inv_U13 inv_U15 inv_U17 inv_U19 inv_U22a inv_O5"
tlc u_tiny "$INV" 240
timed "enumerator, same bound" E bfs $EB
for m in i-no-taint iii-partial-hides-taint iv-redo-retries-skipped vi-o5-every-window-end vii-o5-region-base skip-u19; do
  inst m_tiny $B "$I" true true $m
  tlc m_tiny "$INV" 120
  timed "enumerator, mutant $m" E bfs $EB --first --mutant $m
done
