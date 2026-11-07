# Exact counts (PART a, b <= 15 min each; m about 18 min): TLC's "distinct states found" on undo.qnt at small bounds
# against undo_enum bfs at the same bound (no --sym: both count the same states); ITF traces
# replayed into spec_undo::model; every mutant on TLC (one worker) and on the enumerator.
# Records (tools/evidence/): TLC's runs are read from their `=== tlc NAME` output; the
# enumerator's and the replay's are their RUNREC lines (`e_NAME`, `e_m_MUTANT`, `itf_*`).
#   PART=a  2x1+1 undo, 2x1+1 undo+1 revert, 3x1+1 undo, 1x1+1 undo+1 revert; ITF (1,000 traces
#           at 2x1+1 undo+1 revert, 1,000 at 2x1+2 undos+1 revert, where restorations meet;
#           the ITF regression traces, tools/undo/itf-regress)
#   PART=b  2x2+1 undo (ab, 3), 1x3+1 revert+1 restart (abba), 2x1+2 undos+1 revert (ab, v1.8)
#   PART=m  every mutant, TLC with one worker (a shortest counterexample) and the enumerator;
#           the audit's four (u13-refused-revert-applies, r3-redo-rewrites-window,
#           r3-revert-rewrites-window, i1-per-request, each counterexample printed) and the
#           (vi), (vii) and (iv) one invariant a row: O5 and O5doc, U17 and U17doc
#   (unset: all three, which needs more than one 15-min job since v1.8)
settings PART=a
INV="inv_own_only inv_I1 inv_U9 inv_U13 inv_U15 inv_U17 inv_U19 inv_U22a inv_RT inv_O5 inv_O5doc inv_U17doc"
need_apalache
. "$ROOT/tools/undo/tlc.sh"
# the rows each PART must end with, each with its verdict (tools/undo/expect.sh)
case ${PART:-a} in *a*) expect_complete tlc:c_2x1u1 enum:e_c_2x1u1 tlc:c_2x1u1r1 enum:e_c_2x1u1r1 \
  tlc:c_3x1u1 enum:e_c_3x1u1 tlc:c_1x1u1r1 enum:e_c_1x1u1r1 itf:itf_2x1u1r1 itf:itf_2x1u2r1 itf:itf_regress ;; esac
case ${PART:-a} in *b*) expect_complete tlc:c_2x2u1_l3 enum:e_c_2x2u1_l3 tlc:c_1x3r1s1 enum:e_c_1x3r1s1 \
  tlc:c_2x1u2r1 enum:e_c_2x1u2r1 ;; esac
case ${PART:-a} in *m*) _one='-iv-redo-retries-skipped -vi-o5-every-window-end -vii-o5-region-base'
  expect_mutants tlc $_one; expect_mutants enum $_one    # (those three: one invariant a row)
  for _o in tlc enum; do for _r in O5 O5doc; do expect $_o@vi-o5-every-window-end/$_r VIOLATION $_r
    expect $_o@vii-o5-region-base/$_r VIOLATION $_r; done
    for _r in U17 U17doc; do expect $_o@iv-redo-retries-skipped/$_r VIOLATION $_r; done; done ;; esac
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
# (iv), (vi) and (vii) at the end, one invariant at a time
for m in i-no-taint iii-partial-hides-taint skip-u19 f3-rediff; do
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
# the audit's mutants (each with its counterexample printed, TLC_TRACE): U13 (a refused
# all-or-nothing revert applies its clean regions) at 2 x 1 + 1 undo + 1 revert, seed abb
inst m_u13 2 1 1 1 4 "0, 1, 1" true true u13-refused-revert-applies
TLC_TRACE=400 TLC_WORKERS=1 tlc m_u13 "$INV" 240
timed "enumerator, mutant u13-refused-revert-applies" E bfs --run e_m_u13-refused-revert-applies --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init abb --first --mutant u13-refused-revert-applies
# own_only through redo only, at 2 x 1 + 2 undos + 1 revert, seed ab (c_2x1u2r1)
inst m_r3redo 2 1 2 1 4 "0, 1" true true r3-redo-rewrites-window
TLC_TRACE=600 TLC_WORKERS=1 tlc m_r3redo "$INV" 300
timed "enumerator, mutant r3-redo-rewrites-window" E bfs --run e_m_r3-redo-rewrites-window --pids 2 --events 1 --undos 2 --reverts 1 --maxlen 4 --init ab --first --mutant r3-redo-rewrites-window
# own_only through revert only, at 2 x 1 + 1 undo + 1 revert, seed ab
inst m_r3rev 2 1 1 1 4 "0, 1" true true r3-revert-rewrites-window
TLC_TRACE=400 TLC_WORKERS=1 tlc m_r3rev "$INV" 240
timed "enumerator, mutant r3-revert-rewrites-window" E bfs --run e_m_r3-revert-rewrites-window --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init ab --first --mutant r3-revert-rewrites-window
# I1 at an intermediate step only (v1.8's I1, per request), at 2 x 2 + 1 undo + 1 revert,
# seed a, maxlen 3, no two-region edits, no purge (depth 5; about 10^5 states before it)
inst m_i1pr 2 2 1 1 3 "0" false false i1-per-request
TLC_TRACE=800 TLC_WORKERS=1 tlc m_i1pr "$INV" 5400
timed "enumerator, mutant i1-per-request" E bfs --run e_m_i1-per-request --pids 2 --events 2 --undos 1 --reverts 1 --maxlen 3 --init a --no-multi --no-purge --first --mutant i1-per-request
# (vi), (vii) and (iv) against each invariant meant to catch them, that invariant alone (TLC:
# alone in the cfg; the enumerator: --stop it): O5 and the independent O5doc, U17 and U17doc.
# Each row shows its invariant failing, whatever order or company the others would fail in.
for mr in vi-o5-every-window-end:O5 vii-o5-region-base:O5 iv-redo-retries-skipped:U17; do
  m=${mr%%:*}; r=${mr#*:}
  inst m_tiny 2 1 1 0 4 "0, 1, 1" true true $m
  for rr in $r ${r}doc; do
    TLC_WORKERS=1 tlc m_tiny "inv_$rr" 120
    n=e_m_$m; [ "$rr" = "$r" ] || n=e_m_$(echo "$rr" | tr 'A-Z' 'a-z')_$m
    timed "enumerator, mutant $m, $rr only" E bfs --run "$n" --pids 2 --events 1 --undos 1 --reverts 0 --maxlen 4 --init abb --first --stop "$rr" --mutant $m
  done
done
;; esac
