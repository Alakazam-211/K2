# Smoke (one short job, <= 15 min): typecheck; a tiny simulator run exported as ITF and
# replayed into spec_undo::model (also with a restart), and the ITF regression traces
# (tools/undo/itf-regress); TLC's exact count at a tiny bound against the enumerator's; TLC
# on every mutant at the same bound, and on the restart mutant and mutants (x), (xi) at
# their own bounds (each must violate its rule).
B="2 1 1 0 4"            # pids events undos reverts maxlen
I="0, 1, 1"              # the seed text abb
EB="--pids 2 --events 1 --undos 1 --reverts 0 --maxlen 4 --init abb"
timed "typecheck undo" q typecheck undo.qnt
inst u_tiny $B "$I" true true none
timed "typecheck u_tiny" q typecheck u_tiny.qnt
# Apalache reads the model's IR as one JSON string, by default of at most 20,000,000 characters
# (Jackson's); the job's servers raise that limit (tools/quint-jvm), and the size still shows
# here (v1.9's u_tiny about 19.85 MB; the export check's instance, with its step and
# invariants, is larger)
q compile --target json --main=u_tiny u_tiny.qnt > u_tiny.json && echo "=== IR size: $(wc -c < u_tiny.json) bytes (Apalache's limit 20000000)"
itfinst itf_tiny 2 1 1 1 4 "$I" true true none
mkdir -p "$ROOT/itf"
timed "simulate + ITF (2x1 +1 undo +1 revert, 300 traces)" q run itf_tiny.qnt --main=itf_tiny --init=winit --step=wstep \
  --max-samples=300 --n-traces=300 --max-steps=4 --out-itf="$ROOT/itf/t_{seq}.itf.json" --verbosity=1
timed "ITF replay into spec_undo::model" R --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init abb "$ROOT/itf"
need_apalache
. "$ROOT/tools/undo/tlc.sh"
INV="inv_own_only inv_I1 inv_U9 inv_U13 inv_U15 inv_U17 inv_U19 inv_U22a inv_RT inv_O5 inv_O5doc inv_U17doc"
tlc u_tiny "$INV" 240
timed "enumerator, same bound" E bfs $EB
for m in i-no-taint iii-partial-hides-taint iv-redo-retries-skipped vi-o5-every-window-end vii-o5-region-base skip-u19 f3-rediff; do
  inst m_tiny $B "$I" true true $m
  tlc m_tiny "$INV" 120
  timed "enumerator, mutant $m" E bfs $EB --first --mutant $m
done
# the restart mutant at its own bound: 1 pid x 3 events + 1 revert + 1 restart, seed abba
inst m_rst 1 3 0 1 4 "0, 1, 1, 0" true true restart-single-taint 1
tlc m_rst "$INV" 240
timed "enumerator, mutant restart-single-taint" E bfs --pids 1 --events 3 --undos 0 --reverts 1 --restarts 1 --maxlen 4 --init abba --first --mutant restart-single-taint
# mutant (x) (v1.8) at its own bound: 1 pid x 1 event + 1 undo + 1 revert, seed ab
inst m_prov 1 1 1 1 4 "0, 1" true true x-provenance-ignored-in-o4
tlc m_prov "$INV" 120
timed "enumerator, mutant x-provenance-ignored-in-o4" E bfs --pids 1 --events 1 --undos 1 --reverts 1 --maxlen 4 --init ab --first --mutant x-provenance-ignored-in-o4
# mutant (xi) (v1.9: O4 on roots, no O4a) at its own bound: 2 pids x 1 event + 1 undo + 1 revert, seed ab
inst m_xi 2 1 1 1 4 "0, 1" true true xi-no-o4a
tlc m_xi "$INV" 240
timed "enumerator, mutant xi-no-o4a" E bfs --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init ab --first --mutant xi-no-o4a
# the audit's mutants at their own bounds (counts.sh PART=m has their least depths)
inst m_u13 2 1 1 1 4 "0, 1, 1" true true u13-refused-revert-applies
tlc m_u13 "$INV" 240
timed "enumerator, mutant u13-refused-revert-applies" E bfs --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init abb --first --mutant u13-refused-revert-applies
inst m_r3redo 2 1 2 1 4 "0, 1" true true r3-redo-rewrites-window
tlc m_r3redo "$INV" 240
timed "enumerator, mutant r3-redo-rewrites-window" E bfs --pids 2 --events 1 --undos 2 --reverts 1 --maxlen 4 --init ab --first --mutant r3-redo-rewrites-window
inst m_r3rev 2 1 1 1 4 "0, 1" true true r3-revert-rewrites-window
tlc m_r3rev "$INV" 240
timed "enumerator, mutant r3-revert-rewrites-window" E bfs --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init ab --first --mutant r3-revert-rewrites-window
# i1-per-request: the enumerator only here (TLC at its depth 5 is counts.sh PART=m's)
timed "enumerator, mutant i1-per-request" E bfs --pids 2 --events 2 --undos 1 --reverts 1 --maxlen 3 --init a --no-multi --no-purge --first --mutant i1-per-request
# ITF with a restart: 2 pids x 1 event + 1 undo + 1 revert + 1 restart
itfinst itf_rst 2 1 1 1 4 "$I" true true none 1
mkdir -p "$ROOT/itf_rst"
timed "simulate + ITF (with a restart, 300 traces)" q run itf_rst.qnt --main=itf_rst --init=winit --step=wstep \
  --max-samples=300 --n-traces=300 --max-steps=5 --out-itf="$ROOT/itf_rst/t_{seq}.itf.json" --verbosity=1
timed "ITF replay (restart)" R --pids 2 --events 1 --undos 1 --reverts 1 --restarts 1 --maxlen 4 --init abb "$ROOT/itf_rst"
# the ITF regression traces (tools/undo/itf-regress): each must replay with 0 mismatches
. "$ROOT/tools/undo/itf-regress/replay.sh"
