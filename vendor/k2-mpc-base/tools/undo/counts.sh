# Exact counts (each PART <= 15 min): TLC's "distinct states found" on undo.qnt at small bounds
# against undo_enum bfs at the same bound (no --sym: both count the same states); ITF traces
# replayed into spec_undo::model; every mutant on TLC (one worker) and on the enumerator.
# Records (tools/evidence/): TLC's runs are read from their `=== tlc NAME` output; the
# enumerator's and the replay's are their RUNREC lines (`e_NAME`, `e_m_MUTANT`, `itf_*`).
#   PART=a  2x1+1 undo, 2x1+1 undo+1 revert, 3x1+1 undo, 1x1+1 undo+1 revert; ITF (1,000 traces
#           at 2x1+1 undo+1 revert, 1,000 at 2x1+2 undos+1 revert, where restorations meet;
#           the ITF regression traces, tools/undo/itf-regress)
#   PART=b  2x2+1 undo (ab, 3), 1x3+1 revert+1 restart (abba), 2x1+2 undos+1 revert (ab, v1.8)
#   PART=m  every mutant, TLC with one worker (a shortest counterexample) and the enumerator
#   (unset: all three, which needs more than one 15-min job since v1.8)
INV="inv_own_only inv_I1 inv_U9 inv_U13 inv_U15 inv_U17 inv_U19 inv_U22a inv_RT inv_O5"
inst w 2 1 1 0 4 "0, 1, 1" true true none
timed "warm-up (fetches Apalache)" q verify w.qnt --main=w --backend=tlc --invariant=inv
. "$ROOT/tools/undo/tlc.sh"
# cmp NAME PIDS EVENTS UNDOS REVERTS MAXLEN "INIT" MULTI PURGE INITLETTERS SECONDS [RESTARTS]
cmp() {
  inst "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8" "$9" none "${12:-0}"
  tlc "$1" "$INV" "${11}"
  f=""; [ "$8" = false ] && f="$f --no-multi"; [ "$9" = false ] && f="$f --no-purge"
  timed "enumerator $1" E bfs --run "e_$1" --pids "$2" --events "$3" --undos "$4" --reverts "$5" --restarts "${12:-0}" --maxlen "$6" --init "${10}" $f
}
case ${PART:-a} in *a*)
cmp c_2x1u1     2 1 1 0 4 "0, 1, 1" true  true  abb 120
cmp c_2x1u1r1   2 1 1 1 4 "0, 1, 1" true  true  abb 240
cmp c_3x1u1     3 1 1 0 4 "0, 1, 1" true  true  abb 420
cmp c_1x1u1r1   1 1 1 1 4 "0, 1"    true  true  ab  60
itfinst itf_r 2 1 1 1 4 "0, 1, 1" true true none
mkdir -p "$ROOT/itf2"
timed "simulate + ITF (2x1 +1 undo +1 revert, 1000 traces)" q run itf_r.qnt --main=itf_r --init=winit --step=wstep \
  --max-samples=1000 --n-traces=1000 --max-steps=4 --out-itf="$ROOT/itf2/t_{seq}.itf.json" --verbosity=1
export RUNREC_RUN=itf_2x1u1r1
timed "ITF replay" R --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init abb "$ROOT/itf2"
itfinst itf_cr 2 1 2 1 4 "0, 1" true true none
mkdir -p "$ROOT/itf3"
timed "simulate + ITF (2x1 +2 undos +1 revert, ab, 1000 traces)" q run itf_cr.qnt --main=itf_cr --init=winit --step=wstep \
  --max-samples=1000 --n-traces=1000 --max-steps=5 --out-itf="$ROOT/itf3/t_{seq}.itf.json" --verbosity=1
export RUNREC_RUN=itf_2x1u2r1
timed "ITF replay (restorations meet)" R --pids 2 --events 1 --undos 2 --reverts 1 --maxlen 4 --init ab "$ROOT/itf3"
unset RUNREC_RUN
# the ITF regression traces (tools/undo/itf-regress): each must replay with 0 mismatches
. "$ROOT/tools/undo/itf-regress/replay.sh"
;; esac
case ${PART:-a} in *b*)
cmp c_2x2u1_l3  2 2 1 0 3 "0, 1"    true  true  ab  540
cmp c_1x3r1s1   1 3 0 1 4 "0, 1, 1, 0" true true abba 300 1
cmp c_2x1u2r1   2 1 2 1 4 "0, 1"    true  true  ab  240
;; esac
case ${PART:-a} in *m*)
for m in i-no-taint iii-partial-hides-taint iv-redo-retries-skipped vi-o5-every-window-end vii-o5-region-base skip-u19 f3-rediff; do
  inst m_tiny 2 1 1 0 4 "0, 1, 1" true true $m
  TLC_WORKERS=1 tlc m_tiny "$INV" 120
  timed "enumerator, mutant $m" E bfs --run "e_m_$m" --pids 2 --events 1 --undos 1 --reverts 0 --maxlen 4 --init abb --first --mutant $m
done
inst m_rst 1 3 0 1 4 "0, 1, 1, 0" true true restart-single-taint 1
TLC_WORKERS=1 tlc m_rst "$INV" 240
timed "enumerator, mutant restart-single-taint" E bfs --run e_m_restart-single-taint --pids 1 --events 3 --undos 0 --reverts 1 --restarts 1 --maxlen 4 --init abba --first --mutant restart-single-taint
inst m_prov 1 1 1 1 4 "0, 1" true true x-provenance-ignored-in-o4
TLC_WORKERS=1 tlc m_prov "$INV" 120
timed "enumerator, mutant x-provenance-ignored-in-o4" E bfs --run e_m_x-provenance-ignored-in-o4 --pids 1 --events 1 --undos 1 --reverts 1 --maxlen 4 --init ab --first --mutant x-provenance-ignored-in-o4
# mutant (xi) (v1.9: no O4a) at its own bound: 2 pids x 1 event + 1 undo + 1 revert, seed ab
inst m_xi 2 1 1 1 4 "0, 1" true true xi-no-o4a
TLC_WORKERS=1 tlc m_xi "$INV" 240
timed "enumerator, mutant xi-no-o4a" E bfs --run e_m_xi-no-o4a --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init ab --first --mutant xi-no-o4a
;; esac
