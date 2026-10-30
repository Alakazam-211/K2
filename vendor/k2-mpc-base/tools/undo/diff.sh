# State-set diff (<= 10 min): TLC's states (-dump) against the enumerator's at bounds where the
# counts differ; tools/undo/statediff.py prints the states only one side has.
inst w 2 1 1 0 4 "0, 1, 1" true true none
q verify w.qnt --main=w --backend=tlc --invariant=inv > /dev/null 2>&1
. "$ROOT/tools/undo/tlc.sh"
INV="inv_U9 inv_U13 inv_U15 inv_U17 inv_U19 inv_U22a inv_O5"
inst d_2x1r1 2 1 1 1 4 "0, 1, 1" true true none
TLC_EXTRA="-dump $ROOT/d_2x1r1" tlc d_2x1r1 "$INV" 180
E bfs --pids 2 --events 1 --undos 1 --reverts 1 --maxlen 4 --init abb --dump "$ROOT/d_2x1r1.jsonl" | tail -1
timed "statediff 2x1r1" python3 "$ROOT/tools/undo/statediff.py" "$ROOT/d_2x1r1.dump" "$ROOT/d_2x1r1.jsonl" 2
