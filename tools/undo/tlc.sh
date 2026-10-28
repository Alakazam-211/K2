# TLC helpers for the undo model (sourced by a commands file, in spec/quint). Needs one prior
# `quint verify ... --backend=tlc` run, which downloads Apalache (it bundles TLC).
APA_JAR=$(ls ~/.quint/apalache-dist-*/apalache/lib/apalache.jar | head -1)
# One Apalache server for the whole job, with its own heap and log (quint reuses a server
# already on port 8822). A server quint starts itself gets 4 GiB, and on the v1.7 model its
# transpile failed at random ("error:" with nothing after it), so the job runs its own.
port_open() { bash -c 'exec 3<>/dev/tcp/127.0.0.1/8822' 2>/dev/null; }
apa_up() {
  # a server quint started (the warm-up's) may still be closing: wait for the port
  i=0; while port_open && [ $i -lt 60 ]; do sleep 1; i=$((i + 1)); done
  java -Xmx${APA_HEAP:-6g} -jar "$APA_JAR" server --port=8822 > "$ROOT/apalache-server.log" 2>&1 &
  i=0; until grep -q "running on port" "$ROOT/apalache-server.log" 2>/dev/null || [ $i -ge 120 ]; do sleep 1; i=$((i + 1)); done
  echo "=== apalache server: up after ${i}s (heap ${APA_HEAP:-6g})"; tail -2 "$ROOT/apalache-server.log"
}
apa_up
# tlc NAME "INV1 INV2 ..." SECONDS: compile instance NAME to TLA+ and run TLC on it to
# completion (or the first violation), printing the distinct-state count and any trace.
tlc() {
  n=$1; inv=$2; secs=$3
  # a compile can still fail at random (quint gives "error:" and nothing else; about one
  # in six on a cold server): retried, each failure logged
  k=1; set +e
  until quint compile --target tlaplus --main="$n" --invariant="$(echo $inv | tr ' ' ',')" "$n.qnt" > "$n.tla" 2> "$n.cerr"; do
    echo "=== quint compile $n: attempt $k failed: $(head -c 200 "$n.cerr")"
    k=$((k + 1)); [ $k -le 3 ] || { set -e; return 1; }
  done; set -e
  { printf 'INIT q_init\nNEXT q_step\n'
    for i in $inv; do
      name=$(grep -oE "^[A-Za-z0-9_]*_$i ==" "$n.tla" | head -1 | sed 's/ ==//')
      [ -n "$name" ] || name=$i
      printf 'INVARIANT %s\n' "$name"
    done; } > "$n.cfg"
  s=$(date +%s); set +e
  timeout "$secs" java -XX:+UseParallelGC -Xmx${TLC_HEAP:-3g} -cp "$APA_JAR" tlc2.TLC -workers ${TLC_WORKERS:-auto} -deadlock -cleanup ${TLC_EXTRA:-} -config "$n.cfg" "$n.tla" > "$n.tlc.log" 2>&1
  rc=$?; set -e
  grep -E "states generated|distinct states|depth of|violated|Finished" "$n.tlc.log" | head -12
  # a counterexample's length in actions (TLC's BFS finds a shortest one): its states - 1
  k=$(grep -oE "^(State )?[0-9]+: <" "$n.tlc.log" | grep -oE "[0-9]+" | sort -n | tail -1)
  [ -n "$k" ] && echo "counterexample: $k states, $((k - 1)) actions"
  if [ "$rc" = 150 ] || [ "$rc" = 75 ]; then grep -B2 -A12 -iE "semantic|parse error|multiply|unknown operator|was expected" "$n.tlc.log" | head -40; fi
  echo "=== tlc $n [$inv]: exit $rc, wall $(( $(date +%s) - s ))s (124 = stopped by timeout)"
}
