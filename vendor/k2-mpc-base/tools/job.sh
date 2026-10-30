#!/bin/sh
# The job harness: run a commands file at the root of a checkout of this repo on a compute
# node. The default way to run any part of the suite (tools/README.md, "Test jobs"): the
# runner ships the commit itself (`k2 compute run --repo`), so nothing travels in the
# environment, and the job is
#
#   sh tools/job.sh <commands file>         e.g. PART=base-a1 with tools/tests/full.sh
#
# The commands file is sourced at the repo root. It runs each step as `timed LABEL CMD...`,
# which prints `=== LABEL: exit N, wall Ss` and makes the job exit non-zero if any step failed
# (the later steps still run). The harness prints a heartbeat every 4 minutes (a runner may
# stop a job that prints nothing), the job's cgroup memory peak at the end (to size --mem),
# and stops any process the job left behind. tools/test-bundle.sh's fallback jobs run the
# same harness after unpacking.
#
# Env: CARGO_TARGET_DIR (a runner's warm target dir is kept; else ./target); CARGO_BUILD_JOBS,
# RUST_TEST_THREADS, NEXTEST_TEST_THREADS (a value already set wins; else the job's CPU quota).
set -e
CMDS=${1:?usage: sh tools/job.sh <commands file>}
ROOT=$PWD
case $CMDS in /*) ;; *) CMDS=$ROOT/$CMDS ;; esac
[ -f "$CMDS" ] || { echo "job.sh: no commands file $CMDS (run from the repo root)" >&2; exit 2; }
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
uname -sm; nproc; free -g 2>/dev/null | head -2 || true
# threads = the job's CPU quota: under a cgroup quota nproc counts the whole machine (16 in a
# 4-CPU job), so cargo and the test runners would start 4x the processes, throttled, each
# with its own memory. A value already set (by the runner, or --env) wins.
Q=$(cat "/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup 2>/dev/null | head -1)/cpu.max" 2>/dev/null || true)
N=$(echo "$Q" | awk '$1 != "max" && $2 > 0 { printf "%d", ($1 + $2 - 1) / $2 }')
if [ -n "$N" ]; then
  export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-$N} RUST_TEST_THREADS=${RUST_TEST_THREADS:-$N} NEXTEST_TEST_THREADS=${NEXTEST_TEST_THREADS:-$N}
  echo "cpu quota $N: build jobs $CARGO_BUILD_JOBS, test threads $RUST_TEST_THREADS"
fi
command -v rustc >/dev/null && rustc --version || true
# nextest's JUnit files land in the target dir's nextest/<profile>/junit.xml; a runner's warm
# target dir sits outside the checkout, so they are copied to ./junit/<profile>/junit.xml at
# the end (an --artifact glob is relative to the checkout). Earlier runs' files in a warm dir
# go first, so only this job's are copied.
JUNIT_DIRS="$CARGO_TARGET_DIR/nextest"
[ "$CARGO_TARGET_DIR" = "$ROOT/target" ] || JUNIT_DIRS="$JUNIT_DIRS $ROOT/target/nextest"
for d in $JUNIT_DIRS; do rm -f "$d"/*/junit.xml 2>/dev/null || true; done
junit_copy() {
  n=0
  for d in $JUNIT_DIRS; do
    for f in "$d"/*/junit.xml; do
      [ -f "$f" ] || continue
      p=$(basename "$(dirname "$f")"); mkdir -p "$ROOT/junit/$p" && cp "$f" "$ROOT/junit/$p/junit.xml" && n=$((n + 1))
    done
  done
  echo "junit: $n file(s) copied to junit/ (from $JUNIT_DIRS)"
}
( while sleep 240; do echo "[hb $(date -u +%H:%M:%S)] $(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null)"; done ) & HB=$!
# at the end: the job's peak memory (its cgroup's, page cache included), to size --mem; the
# heartbeat stopped (its sleep too), then the epilogue
CG=/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup 2>/dev/null | head -1)
# Job epilogue (as the engine's tools/quint-job.sh): at exit, list the processes still in this
# job's session (an Apalache server, a TLC or enumerator run cut short, a test's child) and
# stop them, so none outlives the job. The job's own shell is left out.
JOB_SID=$(ps -o sid= -p $$ 2>/dev/null | tr -d ' ')
job_epilogue() {
  [ -n "$JOB_SID" ] || return 0
  echo "=== job epilogue: processes left in this job's session (sid $JOB_SID)"
  ps -eo pid=,ppid=,sid=,etime=,args= 2>/dev/null | awk -v s="$JOB_SID" -v me=$$ '$3==s && $1!=me && $0 !~ /tools\/job\.sh/ && $0 !~ /sh -ec set -e.mkdir -p job/ && $0 !~ / (ps -eo pid=|cut -c1-160)/' | cut -c1-160 || true
  for p in $(ps -eo pid=,sid=,args= 2>/dev/null | awk -v s="$JOB_SID" -v me=$$ '$2==s && $1!=me && $0 !~ /tools\/job\.sh/ && $0 !~ /sh -ec set -e.mkdir -p job/ {print $1}'); do kill $p 2>/dev/null || true; done
}
trap 'junit_copy; for c in $(ps -o pid= --ppid $HB 2>/dev/null); do kill $c 2>/dev/null; done; kill $HB 2>/dev/null || true; [ -r "$CG/memory.peak" ] && echo "job memory peak: $(( $(cat "$CG/memory.peak") / 1048576 )) MiB (cgroup, page cache included)" || true; job_epilogue' EXIT
# lint CMD...: run a fmt or clippy step. The node's toolchain has clippy and rustfmt when the
# job asks for them (`--needs rust@stable+clippy+rustfmt`, which needs k2-node 0.46.7 on the
# node: an older one refuses it, node_upgrade_required). Without them (`--needs rust@stable`
# on an older node) the toolchain has neither and a read-only rustup home (`rustup component
# add` fails there), so then, and only for these steps, a job-local stable toolchain with both is installed under
# the job dir (rustup-init, minimal profile) and the step runs on it; test steps keep the
# node's toolchain. One function, to drop once every node runs 0.46.7.
lint() {
  if [ -z "${LINT_HOME:-}" ] && cargo clippy --version >/dev/null 2>&1 && cargo fmt --version >/dev/null 2>&1; then
    "$@"; return
  fi
  LINT_HOME=${LINT_HOME:-$ROOT/../lint-toolchain}
  if [ ! -x "$LINT_HOME/cargo/bin/cargo-clippy" ]; then
    echo "note: no clippy/rustfmt in the node's toolchain (--needs rust@stable+clippy+rustfmt needs k2-node 0.46.7): installing a job-local stable toolchain for the fmt and clippy steps"
    mkdir -p "$LINT_HOME"
    curl -sSf --retry 3 https://sh.rustup.rs -o "$LINT_HOME/rustup-init.sh"
    RUSTUP_HOME=$LINT_HOME/rustup CARGO_HOME=$LINT_HOME/cargo sh "$LINT_HOME/rustup-init.sh" -y -q \
      --profile minimal --default-toolchain stable -c clippy,rustfmt --no-modify-path
  fi
  RUSTUP_HOME=$LINT_HOME/rustup CARGO_HOME=$LINT_HOME/cargo PATH=$LINT_HOME/cargo/bin:$PATH "$@"
}
RC=0
timed() { l=$1; shift; s=$(date +%s); set +e; "$@"; rc=$?; set -e; echo "=== $l: exit $rc, wall $(( $(date +%s) - s ))s"; [ $rc -eq 0 ] || RC=$rc; }
. "$CMDS"
exit $RC
