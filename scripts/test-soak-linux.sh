#!/bin/bash
# Nightly test soak (quiet-gate PRD §9). Companion to test-gate-linux.sh.
#
#   scripts/test-soak-linux.sh <sha> [workdir]
#
# Exports <sha> with `git archive` into a fresh dir, builds the test
# binaries once (own CARGO_TARGET_DIR), then runs, each test process under a
# clean `env -i` (temp HOME/TMPDIR, no SHELL, no K2_*):
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

SHA=${1:?usage: test-soak-linux.sh <sha> [workdir]}
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
git -C "$REPO" archive "$FULL_SHA" | tar -x -C "$SRC" || { echo "git archive failed"; exit 2; }

CARGO_BIN=$(command -v cargo) || { echo "cargo not on PATH"; exit 2; }
TOOL_PATH=$(dirname "$CARGO_BIN"):/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
CLEAN_PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
CARGO_ENV=(CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
  CARGO_TARGET_DIR="$TARGET" CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0)

# Run "$@" in a clean env with a fresh HOME/TMPDIR; returns its exit code.
clean() {
  local h t rc
  h=$(mktemp -d /tmp/k2-soak-home-XXXXXX); t=$(mktemp -d /tmp/k2-soak-tmp-XXXXXX)
  env -i HOME="$h" TMPDIR="$t" PATH="$TOOL_PATH" USER="$(id -un)" LANG=C.UTF-8 "${CARGO_ENV[@]}" "$@"
  rc=$?
  rm -rf "$h" "$t"
  return $rc
}

BURNERS=()
load_on()  { for _ in $(seq 1 "$LOAD"); do yes > /dev/null & BURNERS+=($!); done; }
load_off() { [ ${#BURNERS[@]} -gt 0 ] && kill "${BURNERS[@]}" 2>/dev/null; wait "${BURNERS[@]}" 2>/dev/null; BURNERS=(); }
trap load_off EXIT

# Build every test binary once and record the paths we loop.
echo "soak: building $FULL_SHA"
( cd "$SRC" && clean "$CARGO_BIN" test -p k2-core --no-run ) > "$LOGS/build-core.log" 2>&1 || { echo "build failed (core)"; exit 1; }
( cd "$SRC" && clean "$CARGO_BIN" test -p k2-daemon --no-run ) > "$LOGS/build-daemon.log" 2>&1 || { echo "build failed (daemon)"; exit 1; }
bin_of() { grep -h -oE "\(($TARGET/debug/deps/$1-[0-9a-f]+)\)" "$LOGS"/build-*.log | tr -d '()' | head -1; }
CORE=$(grep -oE "unittests src/lib.rs \(($TARGET/debug/deps/k2_core-[0-9a-f]+)\)" "$LOGS/build-core.log" | grep -oE "$TARGET[^)]+")
DAEMON=$(grep -oE "unittests src/lib.rs \(($TARGET/debug/deps/k2_daemon-[0-9a-f]+)\)" "$LOGS/build-daemon.log" | grep -oE "$TARGET[^)]+")

STATUS=0
# Loop one test binary N times (clean env, fresh HOME/TMPDIR each run).
loop() {
  local name=$1 bin=$2; shift 2
  local pass=0 fail=0 i
  for i in $(seq 1 "$LOOPS"); do
    if (cd "$WORK" && clean "$bin" "$@") > "$LOGS/loop-$name-$i.log" 2>&1; then
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
    if (cd "$WORK" && clean env RUSTC_BOOTSTRAP=1 "$bin" -Z unstable-options --shuffle-seed "$seed") > "$LOGS/shuffle-$name-$seed.log" 2>&1; then
      echo "shuffle $name seed=$seed: ok"; rm -f "$LOGS/shuffle-$name-$seed.log"
    else
      echo "shuffle $name seed=$seed: FAILED $(grep -h '^test .* FAILED$' "$LOGS/shuffle-$name-$seed.log" | tr '\n' ' ')"; STATUS=1
    fi
  done
done

# (c) the whole suite once under load
load_on
for pkg in k2-core k2-daemon; do
  if ( cd "$SRC" && clean "$CARGO_BIN" test -p "$pkg" --no-fail-fast ) > "$LOGS/loaded-$pkg.log" 2>&1; then
    echo "loaded full $pkg: ok"
  else
    echo "loaded full $pkg: FAILED $(grep -h '^test .* FAILED$' "$LOGS/loaded-$pkg.log" | sort -u | tr '\n' ' ')"; STATUS=1
  fi
done
load_off

[ "${K2_SOAK_KEEP:-0}" != "1" ] && rm -rf "$TARGET" "$SRC"
if [ $STATUS -eq 0 ]; then echo "SOAK GREEN ($FULL_SHA)"; else echo "SOAK RED ($FULL_SHA) — logs in $LOGS"; fi
exit $STATUS
