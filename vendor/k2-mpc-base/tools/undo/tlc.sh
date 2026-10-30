# TLC helpers for the undo model (sourced by a commands file, in spec/quint). Needs one prior
# `quint verify ... --backend=tlc` run, which downloads Apalache (it bundles TLC).
APA_JAR=$(ls ~/.quint/apalache-dist-*/apalache/lib/apalache.jar | head -1)
# tlc NAME "INV1 INV2 ..." SECONDS: compile instance NAME to TLA+ and run TLC on it to
# completion (or the first violation), printing the distinct-state count and any trace.
tlc() {
  n=$1; inv=$2; secs=$3
  quint compile --target tlaplus --main="$n" --invariant="$(echo $inv | tr ' ' ',')" "$n.qnt" > "$n.tla"
  { printf 'INIT q_init\nNEXT q_step\n'
    for i in $inv; do
      name=$(grep -oE "^[A-Za-z0-9_]*_$i ==" "$n.tla" | head -1 | sed 's/ ==//')
      [ -n "$name" ] || name=$i
      printf 'INVARIANT %s\n' "$name"
    done; } > "$n.cfg"
  s=$(date +%s); set +e
  timeout "$secs" java -XX:+UseParallelGC -Xmx${TLC_HEAP:-8g} -cp "$APA_JAR" tlc2.TLC -workers ${TLC_WORKERS:-auto} -deadlock -cleanup ${TLC_EXTRA:-} -config "$n.cfg" "$n.tla" > "$n.tlc.log" 2>&1
  rc=$?; set -e
  grep -E "states generated|distinct states|depth of|violated|Finished" "$n.tlc.log" | head -12
  # a counterexample's length in actions (TLC's BFS finds a shortest one): its states - 1
  k=$(grep -oE "^(State )?[0-9]+: <" "$n.tlc.log" | grep -oE "[0-9]+" | sort -n | tail -1)
  [ -n "$k" ] && echo "counterexample: $k states, $((k - 1)) actions"
  if [ "$rc" = 150 ] || [ "$rc" = 75 ]; then grep -B2 -A12 -iE "semantic|parse error|multiply|unknown operator|was expected" "$n.tlc.log" | head -40; fi
  echo "=== tlc $n [$inv]: exit $rc, wall $(( $(date +%s) - s ))s (124 = stopped by timeout)"
}
