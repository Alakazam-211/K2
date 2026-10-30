# TLC helpers for the undo model (sourced by a commands file, in spec/quint). Needs one prior
# `quint verify ... --backend=tlc` run, which downloads Apalache (it bundles TLC).
# The job's Quint export guard must have passed first (tools/undo/job.sh, v0.9.3).
[ "${QUINT_EXPORT_CHECKED:-}" = 1 ] || { echo "=== tlc.sh: the Quint export check has not passed; no TLC run" >&2; exit 1; }
APA_JAR=$(ls ~/.quint/apalache-dist-*/apalache/lib/apalache.jar | head -1)
# One Apalache server for the whole job, with its own heap and log, on the job's own port
# (QJ_APA_PORT, tools/undo/job.sh; quint reuses a server already on it). A server quint starts itself gets 4 GiB, and on the v1.7 model its
# transpile failed at random ("error:" with nothing after it), so the job runs its own.
port_open() { bash -c "exec 3<>/dev/tcp/127.0.0.1/${QJ_APA_PORT:-8822}" 2>/dev/null; }
apa_up() {
  # a server quint started (the warm-up's) may still be closing: wait for the port
  i=0; while port_open && [ $i -lt 60 ]; do sleep 1; i=$((i + 1)); done
  # the Jackson limits agent (tools/quint-jvm; tools/undo/job.sh builds it and exports
  # APA_JAVA_OPTS): the IR travels as one JSON string, past Jackson's 20,000,000 default
  if [ -z "${APA_JAVA_OPTS+x}" ]; then
    APA_JAVA_OPTS=$(sh "$ROOT/tools/quint-jvm/agent.sh" "$APA_JAR" "$ROOT/qjvm") || APA_JAVA_OPTS=""
    export APA_JAVA_OPTS
  fi
  java ${APA_JAVA_OPTS} -Xmx${APA_HEAP:-6g} -jar "$APA_JAR" server --port=${QJ_APA_PORT:-8822} > "$ROOT/apalache-server.log" 2>&1 &
  i=0; until grep -q "running on port" "$ROOT/apalache-server.log" 2>/dev/null || [ $i -ge 120 ]; do sleep 1; i=$((i + 1)); done
  echo "=== apalache server: up after ${i}s on port ${QJ_APA_PORT:-8822} (heap ${APA_HEAP:-6g}; ${APA_JAVA_OPTS:-no agent})"; tail -2 "$ROOT/apalache-server.log"
}
# the Apalache server runs in the background: the job's epilogue stops it with anything else
# left (tools/undo/job.sh defines it); sourced without one, install it here
if ! command -v job_epilogue >/dev/null 2>&1; then
  # Job epilogue (as the engine's tools/quint-job.sh): at exit, list the processes still in this
  # job's session (an Apalache server, a TLC or enumerator run cut short, a test's child) and
  # stop them, so none outlives the job. The job's own shell is left out.
  JOB_SID=$(ps -o sid= -p $$ 2>/dev/null | tr -d ' ')
  job_epilogue() {
    [ -n "$JOB_SID" ] || return 0
    echo "=== job epilogue: processes left in this job's session (sid $JOB_SID)"
    ps -eo pid=,ppid=,sid=,etime=,args= 2>/dev/null | awk -v s="$JOB_SID" -v me=$$ '$3==s && $1!=me && $0 !~ /sh -ec set -e.mkdir -p job/ && $0 !~ / (ps -eo pid=|cut -c1-160)/' | cut -c1-160 || true
    for p in $(ps -eo pid=,sid= 2>/dev/null | awk -v s="$JOB_SID" -v me=$$ '$2==s && $1!=me {print $1}'); do kill $p 2>/dev/null || true; done
  }
  trap job_epilogue EXIT
fi
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
    if grep -q -E "StreamConstraintsException|String value length \([0-9]+\) exceeds the maximum allowed" "$n.cerr" "$ROOT/apalache-server.log" 2>/dev/null; then
      if [ -z "${APA_JAVA_OPTS:-}" ]; then echo "=== quint compile $n: the IR is over Jackson's 20M limit and the agent isn't loaded (tools/quint-jvm)"
      else echo "=== quint compile $n: the IR is over the agent's string limit too ($APA_JAVA_OPTS)"; fi
      set -e; return 1
    fi
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
  tlc_record "$n" "$n.tlc.log" "$secs" "$rc" "$(( $(date +%s) - s ))"
  echo "=== tlc $n [$inv]: exit $rc, wall $(( $(date +%s) - s ))s (124 = stopped by timeout)"
}
# tlc_record RUN LOG CAP_S EXIT WALL_S: the run's record, one `RUNREC {json}` line
# (tools/evidence/README.md; the same rules as the engine repository's tools/quint-job.sh):
# COMPLETE only when TLC printed the complete state-graph depth with 0 states left on queue;
# a run stopped by its cap is PARTIAL (a lower bound); VIOLATION with the trace's length in
# states; ERROR for an exit other than 0, 10-14 or 124.
tlc_record() {
  # the job id K2 Compute's nodes set (only [A-Za-z0-9_-]); null elsewhere (collect.py fills it)
  job=$(printf '%s' "${K2_COMPUTE_JOB:-}" | tr -cd 'A-Za-z0-9_-')
  awk -v run="$1" -v cap="$3" -v rc="$4" -v wall="$5" -v job="$job" '
    function jstr(x) { gsub(/\\/, "\\\\", x); gsub(/"/, "\\\"", x); return "\"" x "\"" }
    function jnum(x) { gsub(/,/, "", x); return (x == "" ? "null" : x) }
    /(is|was|were) violated/ { if (viol == "") { viol = $0; sub(/^Error: */, "", viol); sub(/\.$/, "", viol) } }
    /^Error:/ && !/behavior up to this point|violated|counter-example/ { if (err == "") err = $0 }
    /^State [0-9]+:/ { st++ }
    /distinct states found/ { line = $0 }
    /depth of the complete state graph search is/ { d = $NF; sub(/\.$/, "", d) }
    / with [0-9]+ workers? on / { if (w == "" && match($0, / with [0-9]+ worker/)) { w = substr($0, RSTART + 6, RLENGTH - 13) } }
    END {
      n = ""; q = ""; g = ""
      if (match(line, /[0-9,]+ distinct states found/)) { n = substr(line, RSTART, RLENGTH); sub(/ distinct.*/, "", n) }
      if (match(line, /[0-9,]+ states left on queue/)) { q = substr(line, RSTART, RLENGTH); sub(/ states.*/, "", q) }
      if (match(line, /[0-9,]+ states generated/)) { g = substr(line, RSTART, RLENGTH); sub(/ states.*/, "", g) }
      bad = (rc != "" && rc != 0 && rc != 124 && !(rc >= 10 && rc <= 14))
      if (viol != "") status = "VIOLATION"
      else if (bad || err != "") status = "ERROR"
      else if (d != "" && q == "0" && line !~ /^Progress/) status = "COMPLETE"
      else if (n != "") status = "PARTIAL"
      else status = "NO_RESULT"
      printf "RUNREC {\"job\":%s,\"run\":%s,\"tool\":\"tlc\",\"status\":\"%s\",\"states\":%s,\"depth\":%s,\"queued\":%s,\"cap_s\":%s,\"generated\":%s,\"exit\":%s,\"wall_s\":%s", (job != "" ? jstr(job) : "null"), jstr(run), status, jnum(n), (status == "COMPLETE" || status == "VIOLATION" ? jnum(d) : "null"), jnum(q), jnum(cap), jnum(g), jnum(rc), jnum(wall)
      # the TLC worker count, from its own log (a least depth needs one worker: a strict BFS)
      printf ",\"workers\":%s", jnum(w)
      if (status == "VIOLATION") printf ",\"violation\":%s,\"trace_states\":%s", jstr(viol), (st >= 2 ? st : "null")
      if (status == "ERROR") printf ",\"error\":%s", jstr(err != "" ? err : "TLC exit " rc)
      printf "}\n"
    }' "$2"
}
