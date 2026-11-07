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
# tlc (see tools/undo-quint-job.sh). Each run (TLC, E, R) is a row: the commands file declares
# its rows with their verdicts (`expect`, tools/undo/expect.sh) and the job exits 1 when one
# didn't run, ended with another verdict, or a tool failed. tools/undo-bundle.sh's fallback jobs run this same script
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
# Row verdicts (tools/undo/expect.sh): the commands file declares its rows (`expect`), each is
# checked as it ends, and the job fails when one didn't run or ended otherwise; at any exit too
. "$ROOT/tools/undo/expect.sh"
trap '_expect_exit $?' EXIT
# a runner's warm target dir is kept (a --repo job); else ./target
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
rustc --version; nproc; free -g | head -2
settings QUINT_VERSION=0.33.0 APA_HEAP=6g TLC_HEAP=3g TLC_WORKERS=auto TLC_TRACE TLC_EXTRA NODE_HEAP_MB=4096 CARGO_TARGET_DIR
s=$(date +%s)
cargo build -q --release --features mutants -p k2-mpc-spec-undo --bins
echo "=== build: wall $(( $(date +%s) - s ))s"
E() { _expect_tool enum "$CARGO_TARGET_DIR/release/undo_enum" "$@"; }
R() { _expect_tool itf "$CARGO_TARGET_DIR/release/undo_itf_replay" "$@"; }
# timed LABEL CMD...: a step that ran no row (E, R) fails the job when it exits non-zero
timed() { l=$1; shift; s=$(date +%s); tr0=$(_expect_rows); set +e; "$@"; rc=$?; set -e; echo "=== $l: exit $rc, wall $(( $(date +%s) - s ))s"; _expect_step "$l" "$rc" "$tr0"; }
if [ "$KIND" = quint ]; then
java -version 2>&1 | head -1; node --version
mkdir -p q && ( cd q && npm init -y >/dev/null && npm install -s --no-audit --no-fund @informalsystems/quint@${QUINT_VERSION:-0.33.0} >/dev/null )
export PATH=$ROOT/q/node_modules/.bin:$PATH
# A per-job Apalache server port (as the engine's tools/quint-job.sh, e370c5f): jobs on one node
# share its network, so on Quint's default (localhost:8822) one job's compile can reach another
# job's server, which fails with "Received RST_STREAM with code 0" when that job stops it. The
# port comes from the job shell's PID, checked free (a listen probe through node), else
# stepped; a shim first on PATH adds --server-endpoint to every quint compile and verify
# (tlc.sh, the export check, lint.sh, the IR-size compile), and the servers the job starts itself use
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
# fetch Apalache (quint downloads it on its first TLA+ compile) with a one-line module: that
# compile runs on a server quint starts itself, without the Jackson limits agent, so it never
# carries the model's IR (tools/quint-jvm)
cat > q_fetch.qnt <<'QF'
module q_fetch {
  var x: int
  action init = x' = 0
  action step = x' = x
}
QF
quint compile --target tlaplus --main=q_fetch q_fetch.qnt > /dev/null 2> q_fetch.cerr
# quint's own Apalache server (4 GiB) fails this model's transpile at random ("error:" and
# nothing else; tools/undo/tlc.sh): run the check against a server with tlc.sh's heap, then
# stop it so tlc.sh starts its own as before
APA_JAR=$(ls ~/.quint/apalache-dist-*/apalache/lib/apalache.jar 2>/dev/null | head -1)
[ -n "$APA_JAR" ] || { echo "=== Apalache wasn't fetched (q_fetch's compile: $(head -c 200 q_fetch.cerr)): the Quint export guard stopped the job"; exit 1; }
# every Apalache server this job starts loads the Jackson limits agent, or the job stops here
. "$ROOT/tools/quint-jvm/require.sh"
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
  qjvm_require "the export check's Apalache server"
  java ${APA_JAVA_OPTS} -Xmx${APA_HEAP:-6g} -jar "$APA_JAR" server --port=$QJ_APA_PORT > "$ROOT/apalache-export-check.log" 2>&1 & EC_APA=$!
  i=0; until grep -q "running on port" "$ROOT/apalache-export-check.log" 2>/dev/null || [ $i -ge 120 ]; do sleep 1; i=$((i + 1)); done
  # never let a compile fall back to a server quint would start itself (no agent)
  grep -q "running on port" "$ROOT/apalache-export-check.log" || { echo "=== the export check's Apalache server didn't start in 120 s: the Quint export guard stopped the job"; tail -5 "$ROOT/apalache-export-check.log"; exit 1; }
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
  echo "=== quint export check: tool error (attempt $k of 3), retrying (the known random compile failure on a cold Apalache server)"; k=$((k + 1)); sleep 5
done
if [ -n "$EC_APA" ]; then kill $EC_APA 2>/dev/null; wait $EC_APA 2>/dev/null; fi
set -e; echo "=== quint export check (undo.qnt): exit $rc, wall $(( $(date +%s) - s ))s"
[ $rc -eq 0 ] || { echo "=== the Quint export guard stopped the job (export check)"; exit 1; }
export QUINT_EXPORT_CHECKED=1
# Apalache (it bundles TLC) is fetched by the export check's compile above. tools/undo/tlc.sh
# runs its own server with it. A commands file calls this before sourcing tlc.sh; it stops the
# job if Apalache isn't there. It replaces the old warm-up, a `quint verify` on quint's own
# server (4 GiB, without the Jackson limits agent), which checked nothing and has exited 1 on
# every job since v1.9 ("error: error" after SanyParser), its failure harmless but looking
# like one.
need_apalache() {
  APA_JAR=$(ls ~/.quint/apalache-dist-*/apalache/lib/apalache.jar 2>/dev/null | head -1)
  [ -n "$APA_JAR" ] || { echo "=== Apalache isn't in ~/.quint (the export check should have fetched it): no TLC run"; exit 1; }
  echo "=== Apalache: $APA_JAR"
}
else
cd "$ROOT"
fi
. "$ROOT/$CMDS"
expect_done || exit 1
