#!/bin/bash
# Nightly test soak (quiet-gate PRD §9). Companion to test-gate-linux.sh.
#
#   scripts/test-soak-linux.sh <sha> [workdir]
#   scripts/test-soak-linux.sh --selftest    # check the run wrapper, no build
#
# Exports <sha> with `git archive` into a fresh dir, builds the test
# binaries and the k2-ptyd bins once (own CARGO_TARGET_DIR), then runs, each test process under a
# clean `env -i` (temp HOME/TMPDIR, no SHELL, no K2_*), in its own session
# (`setsid`) and under `setpriv --no-new-privs` (sudo and other setuid
# binaries can't raise privileges, as on CI nodes: tests must never reach a
# real root helper). After every step, processes it left behind (this
# user's, not in the script's own process group, not there when the soak
# started) are logged to <workdir>/logs/leftovers.log (pid, pgid, cwd, the
# cwd's tag.txt if any, command line) and killed by process group. The
# step's temp HOME/TMPDIR are made writable, then removed.
#   (a) 200x loops of the known-flaky groups, under CPU load
#   (b) shuffled full runs of both unit-test binaries, seeds 1..10
#   (c) one full `cargo test` of both crates under CPU load
# Prints one line per item and exits non-zero if any run failed. Failing
# runs' logs are kept under <workdir>/logs; the target and src are removed
# unless K2_SOAK_KEEP=1.
#
# Env knobs:
#   K2_SOAK_LOOPS      runs per loop group (default 200)
#   K2_SOAK_SEEDS      shuffle seeds (default "1 2 3 4 5 6 7 8 9 10")
#   K2_SOAK_LOAD       CPU burners during (a) and (c) (default: nproc)
#   K2_SOAK_THREADS    --test-threads for (a) (default 12)
#   K2_SOAK_MIN_FREE_GB  refuse to start below this (default 20)
#   K2_SOAK_KEEP=1     keep target/src

set -uo pipefail

CLEAN_PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# --- the run wrapper -------------------------------------------------------

# Who and what a leftover sweep must never touch: this user's processes
# that existed before the soak (its shell, ssh session, systemd --user…)
# and the script's own process group (the script, its subshells and the
# CPU burners). Tests run in their own sessions (setsid), so even a child
# that stayed in its test's process group is outside ours.
soak_ids() {
  SOAK_UID=$(id -u)
  SOAK_PGID=$(ps -o pgid= -p $$ | tr -d ' ')
  # cgroup v2: only sweep inside the soak's own cgroup subtree.
  SOAK_CG=$(sed -n 's/^0:://p' /proc/self/cgroup 2>/dev/null)
  BASELINE=" $(ps -u "$SOAK_UID" -o pid= | tr -s ' \n' '  ') "
}

in_soak_cgroup() {
  [ -z "$SOAK_CG" ] || [ "$SOAK_CG" = / ] && return 0
  local cg
  cg=$(sed -n 's/^0:://p' "/proc/$1/cgroup" 2>/dev/null) || return 1
  [ "$cg" = "$SOAK_CG" ] || [ "${cg#"$SOAK_CG"/}" != "$cg" ]
}

# "<pid> <pgid>" for every leftover candidate.
leftover_pids() {
  local pid pgid
  ps -u "$SOAK_UID" -o pid=,pgid= | while read -r pid pgid; do
    [ "$pgid" = "$SOAK_PGID" ] && continue
    case "$BASELINE" in *" $pid "*) continue ;; esac
    in_soak_cgroup "$pid" || continue
    echo "$pid $pgid"
  done
}

# Log and kill what a step left running. $1 names the step.
sweep() {
  local label=$1 list pid pgid cmd cwd tag pgids="" pids="" n
  list=$(leftover_pids)
  [ -z "$list" ] && return 0
  sleep 0.5 # let processes already on their way out finish
  list=$(leftover_pids)
  [ -z "$list" ] && return 0
  {
    echo "== $(date -u +%FT%TZ) after: $label"
    while read -r pid pgid; do
      cmd=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null)
      cwd=$(readlink "/proc/$pid/cwd" 2>/dev/null)
      tag=""
      [ -n "$cwd" ] && [ -f "$cwd/tag.txt" ] && tag=" tag=$(head -1 "$cwd/tag.txt" | cut -c1-200)"
      echo "pid=$pid pgid=$pgid cwd=${cwd:-?}$tag cmd=${cmd:-?}"
      case " $pgids " in *" $pgid "*) ;; *) pgids="$pgids $pgid" ;; esac
      pids="$pids $pid"
    done <<< "$list"
  } >> "$LEFTOVERS_LOG"
  for pgid in $pgids; do
    # A group named after a process that was there before the soak is not
    # the run's: kill only the leftovers themselves then.
    case "$BASELINE" in *" $pgid "*) continue ;; esac
    [ "$pgid" -gt 1 ] && [ "$pgid" != "$SOAK_PGID" ] && kill -KILL -- "-$pgid" 2>/dev/null
  done
  for pid in $pids; do kill -KILL "$pid" 2>/dev/null; done
  n=$(wc -l <<< "$list")
  echo "$n" >> "$LOGS/leftover-counts"
  echo "soak: $n leftover process(es) after $label, killed (process groups:$pgids); see $LEFTOVERS_LOG"
}

# Run "$@" in a clean env with a fresh HOME/TMPDIR, in its own session and
# without the right to gain privileges; then sweep its leftovers and remove
# the temp dirs (made writable first: tests leave read-only files). Returns
# the command's exit code. $STEP names the step in the leftovers log.
clean() {
  local h t rc
  h=$(mktemp -d /tmp/k2-soak-home-XXXXXX); t=$(mktemp -d /tmp/k2-soak-tmp-XXXXXX)
  setsid --wait setpriv --no-new-privs env -i HOME="$h" TMPDIR="$t" PATH="$TOOL_PATH" USER="$(id -un)" LANG=C.UTF-8 "${CARGO_ENV[@]}" "$@"
  rc=$?
  sweep "${STEP:-$*}"
  chmod -R u+w "$h" "$t" 2>/dev/null
  rm -rf "$h" "$t" || echo "soak: could not remove $h $t"
  return $rc
}

# Refuse to run tests that could still escalate.
require_no_new_privs() {
  command -v setsid > /dev/null || { echo "setsid not found (util-linux)"; exit 2; }
  command -v setpriv > /dev/null || { echo "setpriv not found (util-linux): refusing to run tests that could escalate"; exit 2; }
  if command -v sudo > /dev/null && setpriv --no-new-privs sudo -n true > /dev/null 2>&1; then
    echo "refusing: sudo still works under setpriv --no-new-privs"
    exit 2
  fi
}

# --selftest: one trivial step through `clean` that leaves two processes
# behind (one in a new session, one in the step's group, cwd with a
# tag.txt), a read-only dir in HOME, and tries sudo. It must: block sudo,
# log both leftovers with their tag, kill them, and remove the temp dirs.
selftest() {
  local rc fail=0 before after
  require_no_new_privs
  LOGS=$(mktemp -d /tmp/k2-soak-selftest-XXXXXX); LEFTOVERS_LOG=$LOGS/leftovers.log
  TOOL_PATH=$CLEAN_PATH; CARGO_ENV=()
  soak_ids
  before=$(find /tmp -maxdepth 1 -user "$SOAK_UID" \( -name 'k2-soak-home-*' -o -name 'k2-soak-tmp-*' \) | wc -l)
  STEP=selftest clean sh -c '
    mkdir -p "$HOME/ro/sub" && touch "$HOME/ro/sub/f" && chmod -R a-w "$HOME/ro"
    mkdir "$HOME/seed" && echo selftest-tag > "$HOME/seed/tag.txt" && cd "$HOME/seed"
    setsid sleep 3001 < /dev/null > /dev/null 2>&1 &
    sleep 3002 < /dev/null > /dev/null 2>&1 &
    if command -v sudo > /dev/null && sudo -n true 2> /dev/null; then echo SUDO_WORKED; exit 3; fi
    echo step-ok'
  rc=$?
  after=$(find /tmp -maxdepth 1 -user "$SOAK_UID" \( -name 'k2-soak-home-*' -o -name 'k2-soak-tmp-*' \) | wc -l)
  [ "$rc" -eq 0 ] || { echo "selftest: step exit $rc (3 = sudo worked)"; fail=1; }
  for s in 3001 3002; do
    grep -q "cmd=sleep $s" "$LEFTOVERS_LOG" 2> /dev/null || { echo "selftest: leftover 'sleep $s' not logged"; fail=1; }
    pgrep -u "$SOAK_UID" -f "^sleep $s\$" > /dev/null && { echo "selftest: 'sleep $s' still running"; pkill -KILL -u "$SOAK_UID" -f "^sleep $s\$"; fail=1; }
  done
  grep -q "tag=selftest-tag" "$LEFTOVERS_LOG" 2> /dev/null || { echo "selftest: the leftover's tag.txt not logged"; fail=1; }
  [ "$after" -eq "$before" ] || { echo "selftest: temp dirs left behind ($before before, $after after)"; fail=1; }
  echo "--- leftovers.log"; cat "$LEFTOVERS_LOG" 2> /dev/null
  rm -rf "$LOGS"
  [ "$fail" -eq 0 ] && { echo "SELFTEST OK"; exit 0; }
  echo "SELFTEST FAILED"; exit 1
}

[ "${1:-}" = "--selftest" ] && selftest

# --- the soak --------------------------------------------------------------

SHA=${1:?usage: test-soak-linux.sh <sha> [workdir] | --selftest}
require_no_new_privs
REPO=$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel) || exit 2
FULL_SHA=$(git -C "$REPO" rev-parse --verify "$SHA^{commit}") || { echo "unknown sha $SHA"; exit 2; }
WORK=${2:-${HOME:-/root}/k2-soak-${FULL_SHA:0:8}-$(date +%s)}
LOOPS=${K2_SOAK_LOOPS:-200}
SEEDS=${K2_SOAK_SEEDS:-"1 2 3 4 5 6 7 8 9 10"}
LOAD=${K2_SOAK_LOAD:-$(nproc)}
THREADS=${K2_SOAK_THREADS:-12}
MIN_FREE_GB=${K2_SOAK_MIN_FREE_GB:-20}

mkdir -p "$WORK" || exit 2
FREE=$(df --output=avail -BG "$WORK" | tail -1 | tr -dc '0-9')
[ "${FREE:-0}" -lt "$MIN_FREE_GB" ] && { echo "refusing: ${FREE}G free, need ${MIN_FREE_GB}G"; exit 2; }
SRC=$WORK/src; LOGS=$WORK/logs; TARGET=$WORK/target
mkdir -p "$SRC" "$LOGS" "$TARGET"
LEFTOVERS_LOG=$LOGS/leftovers.log
# Before anything runs: what the leftover sweeps must leave alone.
soak_ids
git -C "$REPO" archive "$FULL_SHA" | tar -x -C "$SRC" || { echo "git archive failed"; exit 2; }

CARGO_BIN=$(command -v cargo) || { echo "cargo not on PATH"; exit 2; }
TOOL_PATH=$(dirname "$CARGO_BIN"):$CLEAN_PATH
CARGO_ENV=(CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
  CARGO_TARGET_DIR="$TARGET" CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0)

BURNERS=()
load_on()  { for _ in $(seq 1 "$LOAD"); do yes > /dev/null & BURNERS+=($!); done; }
load_off() { [ ${#BURNERS[@]} -gt 0 ] && kill "${BURNERS[@]}" 2>/dev/null; wait "${BURNERS[@]}" 2>/dev/null; BURNERS=(); }
trap load_off EXIT

# Build every test binary once and record the paths we loop.
echo "soak: building $FULL_SHA"
( cd "$SRC" && STEP=build-core clean "$CARGO_BIN" test -p k2-core --no-run ) > "$LOGS/build-core.log" 2>&1 || { echo "build failed (core)"; exit 1; }
( cd "$SRC" && STEP=build-daemon clean "$CARGO_BIN" test -p k2-daemon --no-run ) > "$LOGS/build-daemon.log" 2>&1 || { echo "build failed (daemon)"; exit 1; }
# The ptyd suites (k2-core tests/ptyd_backend.rs, k2-daemon ptyd_* tests)
# exec target/debug/k2-ptyd and k2-ptyd-test-agent; `cargo test -p k2-core`
# / `-p k2-daemon` never builds another package's binaries.
( cd "$SRC" && STEP=build-ptyd clean "$CARGO_BIN" build -p k2-ptyd --bins ) > "$LOGS/build-ptyd.log" 2>&1 || { echo "build failed (k2-ptyd bins)"; exit 1; }
for b in k2-ptyd k2-ptyd-test-agent; do
  [ -x "$TARGET/debug/$b" ] || { echo "build failed: $TARGET/debug/$b missing"; exit 1; }
done
bin_of() { grep -h -oE "\(($TARGET/debug/deps/$1-[0-9a-f]+)\)" "$LOGS"/build-*.log | tr -d '()' | head -1; }
CORE=$(grep -oE "unittests src/lib.rs \(($TARGET/debug/deps/k2_core-[0-9a-f]+)\)" "$LOGS/build-core.log" | grep -oE "$TARGET[^)]+")
DAEMON=$(grep -oE "unittests src/lib.rs \(($TARGET/debug/deps/k2_daemon-[0-9a-f]+)\)" "$LOGS/build-daemon.log" | grep -oE "$TARGET[^)]+")

STATUS=0
# Loop one test binary N times (clean env, fresh HOME/TMPDIR each run).
loop() {
  local name=$1 bin=$2; shift 2
  local pass=0 fail=0 i
  for i in $(seq 1 "$LOOPS"); do
    if (cd "$WORK" && STEP="loop-$name-$i" clean "$bin" "$@") > "$LOGS/loop-$name-$i.log" 2>&1; then
      pass=$((pass+1)); rm -f "$LOGS/loop-$name-$i.log"
    else
      fail=$((fail+1))
    fi
  done
  echo "loop $name: pass=$pass fail=$fail"
  [ $fail -gt 0 ] && STATUS=1
}

# (a) known-flaky groups, under load
load_on
loop core-s0-fixes     "$CORE"   session_archive:: cli_stage:: connect_users::tests::clear_lockout path_enrichment_tests:: perf:: push:: test_env:: --test-threads="$THREADS"
loop core-shared-db    "$CORE"   projects_ops:: workspace:: db:: chat_history:: chat_user_archive:: overlay:: --test-threads="$THREADS"
loop core-home-locks   "$CORE"   skin:: wiki_public_chat:: terminal::ensure_cli:: tunnel::connector:: --test-threads="$THREADS"
loop daemon-spawnqueue "$DAEMON" spawn_queue:: sandbox_quota:: --test-threads="$THREADS"
loop daemon-home-race  "$DAEMON" boot_verifier subscription_usage:: --test-threads="$THREADS"
loop daemon-latent     "$DAEMON" sql:: federation_routes:: tunnel_tls_listener:: remote_session_routes:: --test-threads="$THREADS"
for t in login_lock_clear agents_routes_integration triage_integration tunnel_disable_unpair_integration zen_headless_integration; do
  b=$(bin_of "$t")
  [ -n "$b" ] && loop "$t" "$b" --test-threads="$THREADS"
done
load_off

# (b) shuffled full unit runs
for seed in $SEEDS; do
  for pair in "core:$CORE" "daemon:$DAEMON"; do
    name=${pair%%:*}; bin=${pair#*:}
    if (cd "$WORK" && STEP="shuffle-$name-$seed" clean env RUSTC_BOOTSTRAP=1 "$bin" -Z unstable-options --shuffle-seed "$seed") > "$LOGS/shuffle-$name-$seed.log" 2>&1; then
      echo "shuffle $name seed=$seed: ok"; rm -f "$LOGS/shuffle-$name-$seed.log"
    else
      echo "shuffle $name seed=$seed: FAILED $(grep -h '^test .* FAILED$' "$LOGS/shuffle-$name-$seed.log" | tr '\n' ' ')"; STATUS=1
    fi
  done
done

# (c) the whole suite once under load
load_on
for pkg in k2-core k2-daemon; do
  if ( cd "$SRC" && STEP="loaded-$pkg" clean "$CARGO_BIN" test -p "$pkg" --no-fail-fast ) > "$LOGS/loaded-$pkg.log" 2>&1; then
    echo "loaded full $pkg: ok"
  else
    echo "loaded full $pkg: FAILED $(grep -h '^test .* FAILED$' "$LOGS/loaded-$pkg.log" | sort -u | tr '\n' ' ')"; STATUS=1
  fi
done
load_off

LEFT=$(awk '{ s += $1 } END { print s + 0 }' "$LOGS/leftover-counts" 2> /dev/null || echo 0)
if [ "${LEFT:-0}" -gt 0 ]; then
  echo "leftovers: $LEFT process(es) outlived their test step and were killed; see $LEFTOVERS_LOG"
else
  echo "leftovers: none"
fi

[ "${K2_SOAK_KEEP:-0}" != "1" ] && rm -rf "$TARGET" "$SRC"
if [ $STATUS -eq 0 ]; then echo "SOAK GREEN ($FULL_SHA)"; else echo "SOAK RED ($FULL_SHA) — logs in $LOGS"; fi
exit $STATUS
