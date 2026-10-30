#!/bin/sh
# The undo model check's job (docs/MODEL-CHECK.md, tools/README.md "The undo model check"), run
# at the root of a checkout of this repo on a compute node. The default way: the runner ships
# the commit (`k2 compute run --repo`), nothing travels in the environment, and the job is
#
#   sh tools/undo/job.sh <quint|enum> <commands file>
#
# It builds the spec-undo binaries (release, mutants on) and runs the commands file with
# E ARGS (undo_enum), R ARGS (undo_itf_replay) and timed LABEL CMD.... `enum`: at the repo
# root, no Quint or Java. `quint`: installs Quint (0.33; TLC comes with Apalache), runs the
# export guard (tools/quint), then the commands file in spec/quint with inst, itfinst, q and
# tlc (see tools/undo-quint-job.sh). tools/undo-bundle.sh's fallback jobs run this same script
# after unpacking. Extra files a commands file reads (a checkpoint) travel with the commit, or
# with --dirty as untracked files.
set -e
KIND=${1:?usage: sh tools/undo/job.sh <quint|enum> <commands file>}
CMDS=${2:?commands file}
case $KIND in quint|enum) ;; *) echo "kind: quint or enum" >&2; exit 2 ;; esac
[ -f "$CMDS" ] || { echo "undo/job.sh: no commands file $CMDS (run from the repo root)" >&2; exit 2; }
ROOT=$PWD
# Job epilogue (as the engine's tools/quint-job.sh): at exit, list the processes still in this
# job's session (an Apalache server, a TLC or enumerator run cut short, a test's child) and
# stop them, so none outlives the job. The job's own shell is left out.
JOB_SID=$(ps -o sid= -p $$ 2>/dev/null | tr -d ' ')
job_epilogue() {
  [ -n "$JOB_SID" ] || return 0
  echo "=== job epilogue: processes left in this job's session (sid $JOB_SID)"
  ps -eo pid=,ppid=,sid=,etime=,args= 2>/dev/null | awk -v s="$JOB_SID" -v me=$$ '$3==s && $1!=me && $0 !~ /tools\/undo\/job\.sh/ && $0 !~ /sh -ec set -e.mkdir -p job/ && $0 !~ / (ps -eo pid=|cut -c1-160)/' | cut -c1-160 || true
  for p in $(ps -eo pid=,sid=,args= 2>/dev/null | awk -v s="$JOB_SID" -v me=$$ '$2==s && $1!=me && $0 !~ /tools\/undo\/job\.sh/ && $0 !~ /sh -ec set -e.mkdir -p job/ {print $1}'); do kill $p 2>/dev/null || true; done
}
trap job_epilogue EXIT
# a runner's warm target dir is kept (a --repo job); else ./target
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
rustc --version; nproc; free -g | head -2
s=$(date +%s)
cargo build -q --release --features mutants -p k2-mpc-spec-undo --bins
echo "=== build: wall $(( $(date +%s) - s ))s"
E() { "$CARGO_TARGET_DIR/release/undo_enum" "$@"; }
R() { "$CARGO_TARGET_DIR/release/undo_itf_replay" "$@"; }
timed() { l=$1; shift; s=$(date +%s); set +e; "$@"; rc=$?; set -e; echo "=== $l: exit $rc, wall $(( $(date +%s) - s ))s"; }
if [ "$KIND" = quint ]; then
java -version 2>&1 | head -1; node --version
mkdir -p q && ( cd q && npm init -y >/dev/null && npm install -s --no-audit --no-fund @informalsystems/quint@${QUINT_VERSION:-0.33.0} >/dev/null )
export PATH=$ROOT/q/node_modules/.bin:$PATH
# A per-job Apalache server port (as the engine's tools/quint-job.sh, e370c5f): jobs on one node
# share its network, so on Quint's default (localhost:8822) one job's compile can reach another
# job's server, which fails with "Received RST_STREAM with code 0" when that job stops it. The
# port comes from the job shell's PID, checked free (a listen probe through node), else
# stepped; a shim first on PATH adds --server-endpoint to every quint compile and verify
# (tlc.sh, the export check, lint.sh, the warm-ups), and the servers the job starts itself use
# it too (QJ_APA_PORT; set it to choose one).
qj_port_free() { node -e 'const s=require("net").createServer();s.once("error",()=>process.exit(1));s.listen(+process.argv[1],"127.0.0.1",()=>s.close(()=>process.exit(0)))' "$1" 2>/dev/null; }
if [ -z "${QJ_APA_PORT:-}" ]; then
  QJ_APA_PORT=$(( 20000 + $$ % 20000 )); qj_i=0
  while ! qj_port_free $QJ_APA_PORT && [ $qj_i -lt 64 ]; do QJ_APA_PORT=$(( 20000 + (QJ_APA_PORT - 20000 + 7919) % 20000 )); qj_i=$((qj_i + 1)); done
fi
export QJ_APA_PORT
mkdir -p "$ROOT/qbin"
printf '#!/bin/sh\ncase "$1" in compile|verify) c=$1; shift; exec "%s" "$c" --server-endpoint=localhost:%s "$@" ;; *) exec "%s" "$@" ;; esac\n' "$ROOT/q/node_modules/.bin/quint" "$QJ_APA_PORT" "$ROOT/q/node_modules/.bin/quint" > "$ROOT/qbin/quint"
chmod +x "$ROOT/qbin/quint"; export PATH=$ROOT/qbin:$PATH; echo "=== apalache server port $QJ_APA_PORT (per job)"
export NODE_OPTIONS=--max-old-space-size=${NODE_HEAP_MB:-4096}
echo "quint $(quint --version)"
q() { quint "$@"; }
# inst NAME PIDS EVENTS UNDOS REVERTS MAXLEN "INIT" MULTI PURGE MUTANT [RESTARTS]: an instance
# of `undo` (INIT like "0, 1, 1"; MULTI, PURGE true|false; MUTANT none or a mutant's name;
# RESTARTS default 0)
params() { printf 'PIDS = %s, EVENTS = %s, UNDOS = %s, REVERTS = %s, RESTARTS = %s, MAXLEN = %s, INIT = [%s], MULTI = %s, PURGE = %s, REDUCE = true, MUTANT = "%s"' "$2" "$3" "$4" "$5" "${11:-0}" "$6" "$7" "$8" "$9" "${10}"; }
inst() { printf 'module %s {\n  import undo(%s).* from "./undo"\n}\n' "$1" "$(params "$@")" > "$1.qnt"; }
# itfinst NAME ...same...: the instance plus `act` and `out` (the ITF wrapper; init winit, step wstep)
itfinst() { sed -e "s|__NAME__|$1|" -e "s|__PARAMS__|$(params "$@")|" "$ROOT/tools/undo/itf.qnt.in" > "$1.qnt"; }
cd spec/quint
# The guard against a Quint -> TLA+ export issue (tools/quint, v0.9.3), before anything else
# runs on the model: the source lint, then the exact export check of an instance of undo.qnt
# (its `quint compile --target tlaplus` fetches Apalache, so no TLC run comes first). A
# finding (exit 1) or a tool error (exit 2) stops the job; tools/undo/tlc.sh refuses to run
# TLC unless the check passed.
s=$(date +%s); set +e
sh "$ROOT/tools/quint/lint.sh" undo.qnt; rc=$?
set -e; echo "=== quint lint (undo.qnt): exit $rc, wall $(( $(date +%s) - s ))s"
[ $rc -eq 0 ] || { echo "=== the Quint export guard stopped the job (lint)"; exit 1; }
inst q_export_check 2 1 1 1 4 "0, 1, 1" true true none 1
s=$(date +%s); set +e
quint compile --target tlaplus --main=q_export_check q_export_check.qnt > /dev/null 2> q_export_check.cerr
# quint's own Apalache server (4 GiB) fails this model's transpile at random ("error:" and
# nothing else; tools/undo/tlc.sh): run the check against a server with tlc.sh's heap, then
# stop it so tlc.sh starts its own as before
APA_JAR=$(ls ~/.quint/apalache-dist-*/apalache/lib/apalache.jar 2>/dev/null | head -1)
EC_APA=""
if [ -n "$APA_JAR" ]; then
  # quint stops the server it started after its compile: wait for the port (as tlc.sh does)
  i=0; while bash -c "exec 3<>/dev/tcp/127.0.0.1/$QJ_APA_PORT" 2>/dev/null && [ $i -lt 60 ]; do sleep 1; i=$((i + 1)); done
  # the Jackson limits agent (tools/quint-jvm): the IR travels as one JSON string, past
  # Jackson's 20,000,000-character default; tools/undo/tlc.sh's server uses it too
  if [ -z "${APA_JAVA_OPTS+x}" ]; then
    APA_JAVA_OPTS=$(sh "$ROOT/tools/quint-jvm/agent.sh" "$APA_JAR" "$ROOT/qjvm") || APA_JAVA_OPTS=""
    export APA_JAVA_OPTS
  fi
  echo "=== apalache server JVM options: ${APA_JAVA_OPTS:-none}"
  java ${APA_JAVA_OPTS} -Xmx${APA_HEAP:-6g} -jar "$APA_JAR" server --port=$QJ_APA_PORT > "$ROOT/apalache-export-check.log" 2>&1 & EC_APA=$!
  i=0; until grep -q "running on port" "$ROOT/apalache-export-check.log" 2>/dev/null || [ $i -ge 120 ]; do sleep 1; i=$((i + 1)); done
fi
# a tool error (exit 2; quint's compile can fail at random on a cold Apalache server, as
# tools/undo/tlc.sh notes) is retried, up to 3 attempts; a finding (exit 1) is never retried
# A tool error from Jackson's string limit (tools/quint-jvm) is not random: it is said
# plainly and not retried.
k=1
while :; do
  sh "$ROOT/tools/quint/lint.sh" --export --quiet q_export_check.qnt q_export_check > "$ROOT/export-check.out" 2>&1; rc=$?
  cat "$ROOT/export-check.out"
  if [ $rc -eq 2 ] && grep -q -E "StreamConstraintsException|String value length \([0-9]+\) exceeds the maximum allowed" "$ROOT/export-check.out" "$ROOT/apalache-export-check.log" 2>/dev/null; then
    if [ -z "${APA_JAVA_OPTS:-}" ]; then
      echo "=== quint export check: the IR is over Jackson's 20M limit and the agent isn't loaded (tools/quint-jvm; see its build line above)"
    else
      echo "=== quint export check: the IR is over the agent's string limit too ($APA_JAVA_OPTS; tools/quint-jvm)"
    fi
    break
  fi
  [ $rc -eq 2 ] && [ $k -lt 3 ] || break
  echo "=== quint export check: tool error (attempt $k of 3), retrying"; k=$((k + 1)); sleep 5
done
if [ -n "$EC_APA" ]; then kill $EC_APA 2>/dev/null; wait $EC_APA 2>/dev/null; fi
set -e; echo "=== quint export check (undo.qnt): exit $rc, wall $(( $(date +%s) - s ))s"
[ $rc -eq 0 ] || { echo "=== the Quint export guard stopped the job (export check)"; exit 1; }
export QUINT_EXPORT_CHECKED=1
else
cd "$ROOT"
fi
. "$ROOT/$CMDS"
