#!/bin/bash
# tools/fuzz/nightly.sh [--dry-run]: one fuzz night. Every line of the plan becomes a node
# job (tools/fuzz/bundle.sh); the script waits for all of them, brings each target's corpus
# and crashes home, and writes a report. tools/README.md, "Fuzzing".
#
# Not scheduled by anything here: whoever runs it picks the night (and keeps it off other
# people's long runs on the same nodes).
#
# FUZZ_HOME (required): the private folder the corpus lives in, outside every repo:
#   <FUZZ_HOME>/plan                    what to run, one job per line, `|` between
#                                       fields (paths may hold spaces):
#                                         <repo dir> | <rev> | <node> | <target>... [| <features>]
#                                       (<repo dir>: a repo or worktree inside the K2
#                                       workspace: the runner ships it at <rev>, unpushed
#                                       commits too; FUZZ_SHIP=env ships it in the job)
#                                       (<features>: the fuzz crate's cargo features)
#                                       (`#` starts a comment; up to 4 targets a job, all
#                                       run at once, one CPU each; node `auto`: the
#                                       allowed node free now, pick-node.py)
#   <FUZZ_HOME>/<target>/corpus/        the corpus (grown by every run; files named by hash)
#   <FUZZ_HOME>/<target>/crashes/       crashes and their minimized forms, by run
#   <FUZZ_HOME>/runs/<stamp>/           each run: the jobs, their artifacts, report.txt
# FUZZ_SECS: seconds per target (default 3600; a job's timeout is this plus 40 minutes,
#   which must stay within the node's grant).
# FUZZ_NODES: the nodes `auto` may pick (space-separated; required for `auto`).
# FUZZ_WINDOW: "HH-HH", the hours (local, or FUZZ_TZ's) jobs may run in, e.g. 01-05: the
#   script submits nothing outside it (a run started late does nothing), no job starts after
#   the end less FUZZ_TIMEOUT (k2 compute run --queue-timeout), and it stops waiting at the
#   end. Unset: no window.
# FUZZ_TIMEOUT: minutes per job (default FUZZ_SECS/60 + 40). FUZZ_MAX_JOBS: at most this
#   many jobs (default 5). FUZZ_SUMMARY_DIR: also copy the report there, as <date>.txt.
# FUZZ_CPUS: CPUs per job (default 4: one per target, or a forking target's workers).
# FUZZ_MEM: memory per job (default 8G). Other env goes to bundle.sh (MALLOC, FLAGS, RSS_MB,
#   ...); MALLOC and FLAGS default to what the kit's targets want.
#
# Exit: 0 when every job ran clean, 1 when any target found something (see the report),
# 2 on a setup or job failure.
set -euo pipefail
DRY=0; [ "${1:-}" = "--dry-run" ] && DRY=1
: "${FUZZ_HOME:?set FUZZ_HOME to the private corpus folder}"
SECS=${FUZZ_SECS:-3600}; MEM=${FUZZ_MEM:-8G}
TIMEOUT=${FUZZ_TIMEOUT:-$(( SECS / 60 + 40 ))}; MAX_JOBS=${FUZZ_MAX_JOBS:-5}
tz() { if [ -n "${FUZZ_TZ:-}" ]; then TZ=$FUZZ_TZ date "$@"; else date "$@"; fi; }
export MALLOC=${MALLOC:-store_records=256}
export FLAGS=${FLAGS:-outside_edit=-len_control=0,-fork=3 text_merge=-len_control=0}
HERE=$(cd "$(dirname "$0")" && pwd)
PLAN="$FUZZ_HOME/plan"
[ -f "$PLAN" ] || { echo "nightly: no plan at $PLAN" >&2; exit 2; }
STAMP=$(date -u +%Y%m%dT%H%MZ)
RUN="$FUZZ_HOME/runs/$STAMP"; mkdir -p "$RUN"
REPORT="$RUN/report.txt"
echo "fuzz night $STAMP: ${SECS}s per target, jobs of ${TIMEOUT} min at most" | tee "$REPORT"
summary() {
  [ -n "${FUZZ_SUMMARY_DIR:-}" ] || return 0
  mkdir -p "$FUZZ_SUMMARY_DIR"; cp "$REPORT" "$FUZZ_SUMMARY_DIR/$(tz +%Y-%m-%d).txt"
}
# the window: minutes left to start a job in (QUEUE) and to wait in (LEFT)
QUEUE=""; LEFT=$(( TIMEOUT + 60 ))
if [ -n "${FUZZ_WINDOW:-}" ]; then
  from=$((10#${FUZZ_WINDOW%-*})); to=$((10#${FUZZ_WINDOW#*-}))
  now=$(( 10#$(tz +%H) * 60 + 10#$(tz +%M) ))
  LEFT=$(( to * 60 - now )); QUEUE=$(( LEFT - TIMEOUT ))
  if [ "$now" -lt $(( from * 60 )) ] || [ "$QUEUE" -lt 1 ]; then
    echo "outside the window $FUZZ_WINDOW ($(tz +%H:%M)): nothing submitted" | tee -a "$REPORT"
    summary; exit 0
  fi
  echo "window $FUZZ_WINDOW: jobs may start for $QUEUE more minutes" | tee -a "$REPORT"
fi

JOBS=(); n=0
# the plan on fd 3: the k2 CLI in the loop must not read it
while IFS='|' read -r repo rev node targets features <&3; do
  case "$repo" in ''|\#*) continue ;; esac
  trim() { echo "$1" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'; }
  repo=$(trim "$repo"); rev=$(trim "$rev"); node=$(trim "$node"); targets=$(trim "$targets")
  features=$(trim "${features:-}")
  n=$((n+1)); b="$RUN/bundle-$n"
  [ $n -le "$MAX_JOBS" ] || { echo "plan line $n: over FUZZ_MAX_JOBS ($MAX_JOBS), skipped" | tee -a "$REPORT"; continue; }
  # shellcheck disable=SC2086
  JOB_SECS=$(( TIMEOUT * 60 )) FEATURES=$features "$HERE/bundle.sh" "$repo" "$rev" "$b" "$FUZZ_HOME" "$SECS" $targets | tee -a "$REPORT"
  cpus=${FUZZ_CPUS:-4}
  if [ "$node" = auto ]; then
    : "${FUZZ_NODES:?set FUZZ_NODES for node auto}"
    # shellcheck disable=SC2086
    node=$(k2 compute nodes --json 2>/dev/null | FUZZ_USED="${USED:-}" python3 -I "$HERE/pick-node.py" "$cpus" $FUZZ_NODES) \
      || { echo "nightly: none of $FUZZ_NODES is online" >&2; exit 2; }
  fi
  USED="${USED:-} $node"
  ENVA=(); while IFS= read -r l || [ -n "$l" ]; do [ -n "$l" ] && ENVA+=("$l"); done < "$b/env-args"
  # the code: the runner ships the repo at the commit (run from inside it: K2 finds the repo or
  # worktree), or (FUZZ_SHIP=env) it travels in the job
  SRCA=(--no-source); FROM=$PWD
  if [ -f "$b/sha-args" ]; then
    SRCA=(); while IFS= read -r l || [ -n "$l" ]; do [ -n "$l" ] && SRCA+=("$l"); done < "$b/sha-args"
    FROM=$(cat "$b/run-from")
  fi
  cmd=(k2 compute run "$node" "${SRCA[@]}" --needs rust@nightly --artifact 'out/*.tgz'
       --cpus "$cpus" --mem "$MEM" --timeout "${TIMEOUT}m" --detach
       ${QUEUE:+--queue-timeout "${QUEUE}m"})
  if [ $DRY = 1 ]; then
    echo "dry run (from $FROM): ${cmd[*]} <${#ENVA[@]} env args> -- sh -ec <job>" | tee -a "$REPORT"
    continue
  fi
  job=$(cd "$FROM" && "${cmd[@]}" "${ENVA[@]}" -- sh -ec "$(cat "$b/job.sh")" 2>/dev/null | tail -1)
  case "$job" in [0-9a-f]*) ;; *) echo "nightly: submitting to $node failed" >&2; exit 2 ;; esac
  echo "job $job on $node: $(basename "$repo")@$rev: $targets${features:+ (features $features)}" | tee -a "$REPORT"
  JOBS+=("$job")
done 3< "$PLAN"
[ $DRY = 1 ] && exit 0
[ ${#JOBS[@]} -gt 0 ] || { echo "nightly: the plan has no jobs" >&2; summary; exit 2; }

RC=0
for job in "${JOBS[@]}"; do
  if [ -n "${FUZZ_WINDOW:-}" ]; then
    # every job ends by the window's end (its timeout); a few minutes' grace to finish
    LEFT=$(( 10#${FUZZ_WINDOW#*-} * 60 + 10 - 10#$(tz +%H) * 60 - 10#$(tz +%M) ))
    [ "$LEFT" -ge 1 ] || LEFT=1
  fi
  k2 compute wait "$job" --quiet --max-wait "${LEFT}m" || true
  k2 compute logs "$job" 2>/dev/null | grep -E '^(FUZZ |=== build|fuzz: )' | sed "s/^/$job /" | tee -a "$REPORT"
  ok=$(k2 compute get "$job" --json 2>/dev/null | python3 -Ic 'import json,sys; r=json.load(sys.stdin); print(r.get("state"), r.get("exit"))' || echo "? ?")
  echo "$job: state/exit $ok" | tee -a "$REPORT"
  case "$ok" in "done 0") ;; "done 1") RC=1 ;; *) [ $RC = 1 ] || RC=2 ;; esac
  d="$RUN/$job"; mkdir -p "$d"
  k2 compute fetch "$job" --out "$d" >/dev/null 2>&1 || { echo "$job: no artifacts" | tee -a "$REPORT"; continue; }
  # a job stopped early left only <target>.partial.tgz: its corpus and crashes, laid out as
  # the final archive is
  find "$d" -name '*.partial.tgz' | while read -r tgz; do
    t=$(basename "$tgz" .partial.tgz)
    [ -f "$(dirname "$tgz")/$t.tgz" ] && continue
    p="$d/partial-$t"; mkdir -p "$p" "$d/$t"; tar xzf "$tgz" -C "$p"
    mv "$p/corpus/$t" "$d/$t/corpus" 2>/dev/null || true
    mv "$p/src/fuzz/artifacts/$t" "$d/$t/crashes" 2>/dev/null || true
    echo "$t: the job stopped early; its last heartbeat's corpus" | tee -a "$REPORT"
  done
  for t in $(cd "$d" && find . -name '*.tgz' ! -name '*.partial.tgz' -exec basename {} .tgz \;) \
           $(cd "$d" && ls -d partial-* 2>/dev/null | sed 's/^partial-//'); do
    tgz=$(find "$d" -name "$t.tgz" | head -1)
    [ -n "$tgz" ] && tar xzf "$tgz" -C "$d"
    mkdir -p "$FUZZ_HOME/$t/corpus" "$FUZZ_HOME/$t/crashes"
    before=$(ls "$FUZZ_HOME/$t/corpus" | wc -l | tr -d ' ')
    [ -d "$d/$t/corpus" ] && find "$d/$t/corpus" -type f -exec cp -n {} "$FUZZ_HOME/$t/corpus/" \;
    after=$(ls "$FUZZ_HOME/$t/corpus" | wc -l | tr -d ' ')
    echo "$t: corpus $before -> $after" | tee -a "$REPORT"
    if [ -d "$d/$t/crashes" ] && [ -n "$(ls "$d/$t/crashes")" ]; then
      for f in "$d/$t/crashes"/*; do cp "$f" "$FUZZ_HOME/$t/crashes/$STAMP-$job-$(basename "$f")"; done
      echo "$t: CRASHES kept in $FUZZ_HOME/$t/crashes/ ($STAMP-$job-*)" | tee -a "$REPORT"
    fi
  done
done
echo "report: $REPORT (exit $RC)" | tee -a "$REPORT"
summary
exit $RC
