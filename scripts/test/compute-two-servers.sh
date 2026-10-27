#!/usr/bin/env bash
# One node, two servers, headless (prd-compute-shared-queue-v1 §16 tier 3
# and the compatibility runs C1–C5). Two real `k2-daemon` processes (A owns
# the node, B is the second server) in short temp HOMEs, and real `k2-node`s
# in temp dirs. Driven only by curl and cli/k2. Never touches the real
# ~/.k2, the keychain, launchd or the running daemon: every process starts
# from `env -i` with its own HOME, TMPDIR and XDG_RUNTIME_DIR, every K2_*
# and K2SO_* variable dropped, and the guards the daemon's own two-daemon
# tests use (agent shim dir, no tunnel lease, air-gapped, no hook install).
#
# Usage: scripts/test/compute-two-servers.sh [scenario ...]
#   (no scenario = all, in order; `list` prints them)
#
# Env:
#   K2X2_KEEP=1          keep the temp root and processes for a look
#   K2X2_NO_BUILD=1      use target/debug as built
#   K2X2_OLD_REF=ref     what to build them from (default the v0.46.2 tag)
#   K2X2_OLD_DIR=dir     where the v0.46.2 binaries are (or get built:
#                        `old-build`); default ~/.cache/k2x2-v0.46.2
#
# Fails loudly: the first failed check stops the run with its evidence.
# Exit 0 only when every chosen scenario passed.
#
# Scenarios marked [S5] need the integrated lanes (S1 sessions per server,
# S4a `node servers` verbs, the queue on the wire). Until then they fail at
# their first sharing step; that's expected and says which piece is missing.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$REPO/target/debug"
CLI="$REPO/cli/k2"
OLD_DIR="${K2X2_OLD_DIR:-$HOME/.cache/k2x2-v0.46.2}"
# Short: the daemon's vault socket path must fit in 104 bytes on macOS.
ROOT="$(mktemp -d /tmp/k2x2-XXXX)"
PATH_CLEAN="/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin:$HOME/.cargo/bin"
# The cargo that runs this script (a compute node's toolchain lives outside
# the usual places) goes first.
if command -v cargo >/dev/null 2>&1; then
    PATH_CLEAN="$(dirname "$(command -v cargo)"):$PATH_CLEAN"
fi
PIDS=()

ALL_SCENARIOS=(solo old-node rollback share both fair reconnect lost restart withdraw revoke expiry skew old-owner old-tenant tiers-solo tiers-shared tiers-old-node tiers-rollback)

# To stderr: a die inside $(...) must still reach the terminal.
say() { printf '[k2x2 %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() {
  say "FAIL: $*"
  say "logs: $ROOT (kept)"
  K2X2_KEEP=1
  : >"$ROOT/.keep" 2>/dev/null || true # a die inside $(...) must keep the logs too
  exit 1
}
pass() { say "PASS: $*"; }

cleanup() {
  for p in "${PIDS[@]:-}"; do
    [[ -n "$p" ]] && kill "$p" 2>/dev/null || true
  done
  sleep 0.5
  for p in "${PIDS[@]:-}"; do
    [[ -n "$p" ]] && kill -9 "$p" 2>/dev/null || true
  done
  pkill -9 -f "frpc -c $ROOT/" 2>/dev/null || true
  # Nodes carry the root in their arguments: nothing of this run survives it.
  pkill -9 -f -- "--home $ROOT/" 2>/dev/null || true
  if [[ "${K2X2_KEEP:-0}" != 1 && ! -e "$ROOT/.keep" ]]; then
    rm -rf "$ROOT"
  else
    say "kept $ROOT"
  fi
}
trap cleanup EXIT

# ── processes ────────────────────────────────────────────────────────

clean() { env -i HOME="$1" PATH="$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 SHELL=/bin/sh "${@:2}"; }
# The builds keep the caller's PATH (a compute node's toolchains, e.g. the
# cmake that prost-build runs, live only there); nothing else of the
# caller's environment reaches them. What runs under test stays on
# PATH_CLEAN.
build_env() { env -i HOME="$1" PATH="$PATH:$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 SHELL=/bin/sh "${@:2}"; }

build() {
  [[ "${K2X2_NO_BUILD:-0}" == 1 ]] && return
  say "building k2-daemon and k2-node (debug, this worktree's target/)"
  (cd "$REPO" && build_env "$HOME" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
    ${RUSTUP_TOOLCHAIN:+RUSTUP_TOOLCHAIN="$RUSTUP_TOOLCHAIN"} ${CARGO_BUILD_JOBS:+CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS"} \
    cargo build -q -p k2-daemon -p k2-node) || die "build"
}

# start_daemon NAME [binary]: a daemon in $ROOT/NAME; sets PORT_<NAME>, TOKEN_<NAME>.
start_daemon() {
  local name="$1" bin="${2:-$BIN/k2-daemon}" h="$ROOT/$1"
  mkdir -p "$h/.k2" "$h/tmp" "$h/run" "$h/agent-shim-empty"
  chmod 700 "$h/tmp" "$h/run"
  rm -f "$h/.k2/daemon.token"
  # Keep the port across restarts (the daemon reuses daemon.port if free).
  env -i HOME="$h" PATH="$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 SHELL=/bin/sh TMPDIR="$h/tmp" XDG_RUNTIME_DIR="$h/run" K2_RUNTIME_DIR= \
    K2_TEST_AGENT_SHIM_DIR="$h/agent-shim-empty" K2SO_WATCHDOG_DISABLED=1 K2_HEARTBEAT_NO_SELF_HEAL=1 \
    K2_SUBSCRIPTION_PROBE=deny K2_COMPUTE=1 K2_AIRGAP=1 K2_TUNNEL_LEASE=off K2_HOOK_INSTALL=0 \
    K2_LLM_ACCOUNTS_KEYCHAIN=0 K2_LLM_ACCOUNTS_LOOP=0 K2_COMPUTE_ADVERTISE=0 K2_USAGE_PRICES_FETCH=0 \
    ${K2X2_DAEMON_ENV:-} "$bin" >>"$h/daemon.log" 2>&1 &
  local pid=$!
  disown "$pid"
  PIDS+=("$pid")
  echo "$pid" >"$h/pid"
  local i
  for i in $(seq 1 300); do
    if [[ -s "$h/.k2/daemon.token" && -s "$h/.k2/daemon.port" ]]; then
      local port
      port="$(cat "$h/.k2/daemon.port")"
      if curl -fsS "http://127.0.0.1:$port/boot-status" 2>/dev/null | jq -e '.phase == "ready"' >/dev/null 2>&1; then
        printf -v "PORT_${name}" '%s' "$port"
        printf -v "TOKEN_${name}" '%s' "$(cat "$h/.k2/daemon.token")"
        say "daemon $name up on :$port (pid $pid)"
        return
      fi
    fi
    kill -0 "$pid" 2>/dev/null || die "daemon $name exited; see $h/daemon.log"
    sleep 0.2
  done
  die "daemon $name never became ready; see $h/daemon.log"
}

stop_daemon() {
  local name="$1" pid
  pid="$(cat "$ROOT/$name/pid")"
  kill "$pid" 2>/dev/null || true
  for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.1; done
  kill -9 "$pid" 2>/dev/null || true
}

url_of() { local p="PORT_$1"; echo "http://127.0.0.1:${!p}"; }

# api NAME METHOD PATH [JSON] [QUERY] → body on stdout; dies on non-2xx
# unless API_OK=any.
api() {
  local name="$1" method="$2" path="$3" body="${4:-}" query="${5:-}" t="TOKEN_$1" out code
  out="$(curl -sS -X "$method" "$(url_of "$name")$path?token=${!t}${query:+&$query}" \
    -H 'content-type: application/json' ${body:+--data "$body"} -w $'\n%{http_code}')" || die "curl $name $path"
  code="${out##*$'\n'}"
  out="${out%$'\n'*}"
  if [[ "${API_OK:-2xx}" != any && "$code" != 2?? ]]; then
    die "$name $method $path → $code: $out"
  fi
  printf '%s' "$out"
}

# k2 NAME args… : the CLI against daemon NAME (only HOME tells it which).
k2() { local name="$1"; shift; clean "$ROOT/$name" bash "$CLI" "$@"; }

# A workspace (a git repo) on server NAME; sets WS_<NAME>.
seed_ws() {
  local name="$1" d="$ROOT/ws-$1"
  mkdir -p "$d"
  (cd "$d" && git init -q && git -c user.email=k2x2@test -c user.name=k2x2 commit -q --allow-empty -m init)
  local id
  id="$(api "$name" POST /cli/projects/create "{\"name\":\"ws-$name\",\"path\":\"$d\"}" | jq -r '.id')"
  [[ -n "$id" && "$id" != null ]] || die "workspace on $name"
  printf -v "WS_${name}" '%s' "$id"
}

# start_node NODE SERVER [binary] [policy-extra]: enrol with SERVER, run,
# confirm the SAS there, wait until online with an offer.
start_node() {
  local node="$1" server="$2" bin="${3:-$BIN/k2-node}" extra="${4:-}" d="$ROOT/node-$1"
  mkdir -p "$d/home" "$d/config"
  printf 'availability = "always"\nforeign_locks = []\nwrite_smoke_lock = ""\ndisk_floor_gb = 0\nmax_parallel = 3\ncpu_cap_percent = 100\n%s' "$extra" >"$d/config/policy.toml"
  local code out sas
  code="$(api "$server" POST /cli/compute/nodes/enroll-code "{\"name\":\"$node\",\"url\":\"$(url_of "$server")\"}" | jq -r '.enroll')"
  out="$(clean "$d" "$bin" enroll --controller "$(url_of "$server")" --enroll "$code" --name "$node" --home "$d/home" --config-dir "$d/config" 2>&1)" ||
    die "enroll $node with $server: $out"
  sas="$(printf '%s' "$out" | sed -n 's/.*Code: \([0-9]\{3\}\) \([0-9]\{3\}\).*/\1\2/p')"
  [[ ${#sas} == 6 ]] || die "no SAS in: $out"
  env -i HOME="$d" PATH="$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 SHELL=/bin/sh "$bin" run --home "$d/home" --config-dir "$d/config" --dev >>"$d/node.log" 2>&1 &
  PIDS+=("$!")
  disown "$!"
  echo "$!" >"$d/pid"
  api "$server" POST /cli/compute/nodes/confirm "{\"node\":\"$node\",\"sas\":\"$sas\"}" >/dev/null
  wait_online "$server" "$node"
  say "node $node enrolled with $server and online"
}

stop_node() { local pid; pid="$(cat "$ROOT/node-$1/pid")"; kill "$pid" 2>/dev/null || true; sleep 1; kill -9 "$pid" 2>/dev/null || true; }

# restart_node NODE [binary] [clock-offset]: e.g. +3700 runs the node's
# clock ahead (test-only: --dev --allow-fake-clock, K2_NODE_FAKE_CLOCK_OFFSET).
restart_node() {
  local node="$1" bin="${2:-$BIN/k2-node}" off="${3:-}" d="$ROOT/node-$1"
  stop_node "$node"
  if [[ -n "$off" ]]; then
    env -i HOME="$d" PATH="$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 SHELL=/bin/sh K2_NODE_FAKE_CLOCK_OFFSET="$off" "$bin" run --home "$d/home" --config-dir "$d/config" --dev --allow-fake-clock >>"$d/node.log" 2>&1 &
  else
    env -i HOME="$d" PATH="$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 SHELL=/bin/sh "$bin" run --home "$d/home" --config-dir "$d/config" --dev >>"$d/node.log" 2>&1 &
  fi
  PIDS+=("$!")
  disown "$!"
  echo "$!" >"$d/pid"
}

node_row() { api "$1" GET /cli/compute/nodes | jq -c --arg n "$2" '.nodes[] | select(.name == $n)'; }

wait_online() {
  for _ in $(seq 1 150); do
    node_row "$1" "$2" | jq -e '.online == true and (.offer | type) == "object"' >/dev/null 2>&1 && return
    sleep 0.2
  done
  die "$2 never came online on $1: $(node_row "$1" "$2")"
}

# run_job SERVER NODE ID ARGV-JSON [extra JSON fields] → job id
run_job() {
  local server="$1" node="$2" id="$3" argv="$4" extra="${5:-}" ws="WS_$1"
  api "$server" POST /cli/compute/run "{\"node\":\"$node\",\"argv\":$argv,\"clientJobId\":\"k2x2-$id-00000000\",\"workspace\":\"${!ws}\",\"noSource\":true,\"detach\":true${extra:+,$extra}}" | jq -r '.job.id'
}

job() { api "$1" GET /cli/compute/jobs/get '' "job=$2"; }

# wait_job SERVER JOB JQ-PREDICATE [secs]
wait_job() {
  local secs="${4:-60}" j
  for _ in $(seq 1 $((secs * 5))); do
    j="$(job "$1" "$2")"
    printf '%s' "$j" | jq -e "$3" >/dev/null 2>&1 && { printf '%s' "$j"; return; }
    sleep 0.2
  done
  die "job $2 on $1 never matched $3: $(job "$1" "$2" | jq -c '{state,reason,detail,exitCode,generation}')"
}

ended='.state as $s | ["done","failed","cancelled","timeout","interrupted","unknown"] | index($s) != null'

# A CPU-bound job for N seconds (one core).
burn() { echo "[\"sh\",\"-c\",\"end=\$((\$(date +%s)+$1)); while [ \$(date +%s) -lt \$end ]; do :; done\"]"; }

# ── v0.46.2 binaries (C2–C5) ─────────────────────────────────────────

old_build() {
  if [[ -x "$OLD_DIR/target/debug/k2-daemon" && -x "$OLD_DIR/target/debug/k2-node" ]]; then
    return
  fi
  say "building v0.46.2 k2-daemon and k2-node into $OLD_DIR (once)"
  mkdir -p "$OLD_DIR/src"
  # Archive to a file first: a git failure must stop here, not feed tar
  # an empty stream.
  # The tag, or its commit (an ancestor of every 0.46.3+ checkout) where a
  # compute job's mirror carries no tags.
  (cd "$REPO" && git archive --format=tar -o "$OLD_DIR/v0.46.2.tar" "${K2X2_OLD_REF:-v0.46.2}") ||
    die "git archive ${K2X2_OLD_REF:-v0.46.2} failed (is the ref there, the checkout trusted?)"
  tar -x -C "$OLD_DIR/src" -f "$OLD_DIR/v0.46.2.tar" || die "unpack v0.46.2"
  (cd "$OLD_DIR/src" && build_env "$HOME" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
    ${RUSTUP_TOOLCHAIN:+RUSTUP_TOOLCHAIN="$RUSTUP_TOOLCHAIN"} ${CARGO_BUILD_JOBS:+CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS"} CARGO_TARGET_DIR="$OLD_DIR/target" cargo build -q -p k2-daemon -p k2-node) || die "v0.46.2 build"
}

# ── sharing steps (S1 + S4a; the PRD §9 verbs) ───────────────────────

# node_servers NODE args… : `k2-node servers …` at the machine. The harness
# nodes are --dev debug builds in temp dirs, so the machine owner is this
# user (no sudo) and there's no root executor: sharing goes through the
# machine path. The remote path (k2 compute node servers add + approve)
# is covered by the daemon's tier-2 tests and the real-mini drill.
node_servers() {
  local node="$1" d="$ROOT/node-$1"
  shift
  clean "$d" "$BIN/k2-node" servers "$@" --home "$d/home" --config-dir "$d/config" --state-dir "$d/state"
}

# share_add NODE OWNER OTHER [extra add flags]: OTHER mints a code, the
# machine owner adds OTHER, OTHER confirms the SAS the node shows. Prints
# OTHER's slot.
share_add() {
  local node="$1" owner="$2" other="$3" pending_only=0
  shift 3
  # --pending: stop once the SAS is confirmed (a server the node then
  # refuses, e.g. a 0.46.2 one, never gets promoted).
  [[ "${1:-}" == --pending ]] && { pending_only=1; shift; }
  local code out sas slot
  mkdir -p "$ROOT/node-$node/state"
  code="$(api "$other" POST /cli/compute/nodes/enroll-code "{\"name\":\"$node\",\"url\":\"$(url_of "$other")\"}" | jq -r '.enroll')"
  [[ "$code" == *.* ]] || die "no code from $other"
  # stdout is the JSON; stderr carries the --dev node's warning.
  out="$(node_servers "$node" add --url "$(url_of "$other")" --code "$code" "$@" --json 2>"$ROOT/add-$node.err")" || die "servers add: $out $(cat "$ROOT/add-$node.err")"
  grep -q "no job wall" "$ROOT/add-$node.err" || die "a --dev node's add doesn't warn: $(cat "$ROOT/add-$node.err")"
  slot="$(printf '%s' "$out" | jq -r '.slot')"
  for _ in $(seq 1 150); do
    sas="$(node_servers "$node" list --json 2>/dev/null | jq -r --argjson s "$slot" '.node[] | select(.slot == $s and .state == "pending") | .sas // empty' | tr -d ' ')"
    [[ ${#sas} == 6 ]] && break
    sleep 0.2
  done
  [[ ${#sas} == 6 ]] || die "no pending SAS for $other (slot $slot) on $node: $(node_servers "$node" list --json 2>&1)"
  # The SAS lives only in the node's 0600 files, never in its log (B5).
  if grep -q -e "${sas:0:3} ${sas:3}" -e "SAS $sas" "$ROOT/node-$node/node.log"; then
    die "the SAS is in $node's log"
  fi
  api "$other" POST /cli/compute/nodes/confirm "{\"node\":\"$node\",\"sas\":\"$sas\"}" >/dev/null
  if [[ $pending_only == 1 ]]; then
    echo "$slot"
    return
  fi
  # The executor promotes a confirmed add every 30 s; these --dev nodes have
  # none, so the machine owner prunes (promotes) now.
  for _ in $(seq 1 100); do
    node_servers "$node" prune >/dev/null 2>&1 || true
    node_servers "$node" list --json 2>/dev/null | jq -e --argjson s "$slot" '.file.servers[] | select(.slot == $s) | .state == "active"' >/dev/null && break
    sleep 0.2
  done
  node_servers "$node" list --json | jq -e --argjson s "$slot" '.file.servers[] | select(.slot == $s) | .state == "active"' >/dev/null ||
    die "slot $slot never became active on $node: $(node_servers "$node" list --json 2>&1)"
  echo "$slot"
}

# share_node NODE OWNER OTHER [flags]: share_add, then both see it shared.
share_node() {
  local node="$1" owner="$2" other="$3"
  share_add "$@" >/dev/null
  wait_online "$other" "$node"
  # The node's servers loop and the next offer carry it (seconds).
  for _ in $(seq 1 200); do
    node_row "$other" "$node" | jq -e '.share.shared == true and .role == "tenant"' >/dev/null 2>&1 &&
      node_row "$owner" "$node" | jq -e '.role == "owner" and .share.shared == true' >/dev/null 2>&1 && return
    sleep 0.2
  done
  die "$node isn't shared on both: $other $(node_row "$other" "$node" | jq -c '{role,shared: .share.shared,servers: .share.servers}'), $owner $(node_row "$owner" "$node" | jq -c '{role,shared: .share.shared}')"
}

# The owner A with its workspace and node n1, made once per run.
ensure_a() {
  [[ -n "${PORT_A:-}" ]] || { start_daemon A; seed_ws A; }
  [[ -d "$ROOT/node-n1" ]] || start_node n1 A
}

# A, B and n1 shared with B (slot 1), made once per run.
ensure_shared() {
  ensure_a
  if [[ -z "${PORT_B:-}" ]]; then
    start_daemon B
    seed_ws B
  fi
  node_row B n1 2>/dev/null | jq -e '.share.shared == true' >/dev/null 2>&1 || share_node n1 A B --max-running 3 --max-queued 30
}

# ── scenarios ────────────────────────────────────────────────────────

s_solo() {
  ensure_a
  local j
  j="$(run_job A n1 solo '["sh","-c","echo hello; exit 3"]')"
  # The receipt frame follows the final state: wait for both.
  j="$(wait_job A "$j" "($ended) and .receiptOk != null")"
  printf '%s' "$j" | jq -e '.state == "done" and .exitCode == 3 and .receiptOk == true' >/dev/null ||
    die "solo job: $(printf '%s' "$j" | jq -c '{state,exitCode,receiptOk,reason}')"
  local logs
  logs="$(api A GET /cli/compute/jobs/logs '' "job=$(printf '%s' "$j" | jq -r .id)" | jq -r '[.chunks[] | select(.stream == "out") | .data | @base64d] | join("")')"
  [[ "$logs" == "hello"* ]] || die "solo logs: $logs"
  pass "solo: a node with one server runs a job as 0.46.2 did"
}

# C: a 0.46.3 server keeps using a 0.46.2 node (JOBS_MIN_PROTOCOL).
s_old_node() {
  old_build
  [[ -n "${PORT_A:-}" ]] || { start_daemon A; seed_ws A; }
  start_node n0 A "$OLD_DIR/target/debug/k2-node"
  node_row A n0 | jq -e '.offer.protocol == 2' >/dev/null || die "n0 isn't protocol 2: $(node_row A n0 | jq -c '.offer.protocol')"
  local j
  j="$(run_job A n0 oldnode '["sh","-c","echo old; exit 0"]')"
  wait_job A "$j" '.state == "done" and .exitCode == 0' >/dev/null
  pass "old-node: a 0.46.3 server runs jobs on a 0.46.2 node"
}

# C4: roll the node back to the v0.46.2 binary (no pre-step, V67): the owner
# keeps working on the same home and ledger.
s_rollback() {
  old_build
  ensure_a
  local j
  j="$(run_job A n1 rb-new '["sh","-c","echo on-0463"]')"
  wait_job A "$j" '.state == "done"' 60 >/dev/null
  restart_node n1 "$OLD_DIR/target/debug/k2-node"
  for _ in $(seq 1 150); do
    node_row A n1 | jq -e '.online == true and .offer.protocol == 2' >/dev/null 2>&1 && break
    sleep 0.2
  done
  node_row A n1 | jq -e '.offer.protocol == 2' >/dev/null || die "rollback: n1 isn't on the old binary: $(node_row A n1 | jq -c '{online,p: .offer.protocol}')"
  j="$(run_job A n1 rb-old '["sh","-c","echo on-0462"]')"
  wait_job A "$j" '.state == "done"' 60 >/dev/null
  restart_node n1
  for _ in $(seq 1 150); do
    node_row A n1 | jq -e '.online == true and .offer.protocol >= 3' >/dev/null 2>&1 && break
    sleep 0.2
  done
  node_row A n1 | jq -e '.offer.protocol >= 3' >/dev/null || die "rollback: n1 didn't come back on the new binary"
  pass "rollback: the owner works on the v0.46.2 node binary and again after rolling forward"
}

s_share() {
  ensure_shared
  pass "share: B added, approved, SAS confirmed; both see n1 shared"
}

s_both() {
  ensure_shared
  # Mixed classes from both, two workspaces on B (PRD §16 tier 3 step 3).
  local a1 a2 b1 b2
  a1="$(run_job A n1 both-a1 "$(burn 6)" '"cpus":1')"
  a2="$(run_job A n1 both-a2 '["sh","-c","echo a2"]' '"cpus":1')"
  b1="$(run_job B n1 both-b1 "$(burn 6)" '"cpus":1')"
  b2="$(run_job B n1 both-b2 '["sh","-c","echo b2"]' '"cpus":1')"
  for p in "A $a1" "A $a2" "B $b1" "B $b2"; do
    set -- $p
    wait_job "$1" "$2" '.state == "done"' 120 >/dev/null
  done
  # B never sees A's job ids or commands in its queue view.
  local v
  v="$(k2 B compute queue n1 --json)" || die "queue view on B"
  printf '%s' "$v" | jq -e --arg a "$a1" '[.entries[] | select(.mine == false) | .job_id // empty] | length == 0' >/dev/null ||
    die "B's view shows another server's job ids: $v"
  pass "both: jobs from both servers ran; views are redacted"
}

# Fair share on CPU time. Every server starts at weight 1 (the owner too,
# Rosson 2026-10-21); the node owner sets A to 2 (a raise is a widening, so
# it's approved), and the CPU then converges to 2:1 within 10 %.
s_fair() {
  local n=30 secs=15 ids_a=() ids_b=() i t0
  ensure_shared
  node_servers n1 list --json | jq -e '[.file.servers[].limits.weight] | all(. == 1)' >/dev/null ||
    die "fair: servers don't all start at weight 1: $(node_servers n1 list --json)"
  node_servers n1 limits slot:0 --weight 2 >/dev/null || die "limits --weight"
  # A change made at the machine reaches the owner server's mirror on its
  # next refresh; --refresh asks the node now.
  k2 A compute node servers n1 --json --refresh | jq -e '[.servers[] | select(.slot == 0) | .weight // .limits.weight] == [2]' >/dev/null ||
    die "A's server list doesn't show weight 2: $(k2 A compute node servers n1 --json --refresh)"
  sleep 4 # the node's servers loop hands the weight to the queue
  # Each server's workspace may run 3 at once (its max_parallel travels in
  # the plan as ws_max_running; 1 by default would cap both at one slot).
  api A POST /cli/compute/grants/set "{\"node\":\"n1\",\"workspace\":\"$WS_A\",\"maxParallel\":3,\"maxQueued\":30}" >/dev/null
  api B POST /cli/compute/grants/set "{\"node\":\"n1\",\"workspace\":\"$WS_B\",\"maxParallel\":3,\"maxQueued\":30}" >/dev/null
  t0="$(date +%s)"
  for i in $(seq 1 $n); do
    ids_a+=("$(run_job A n1 "fair-a$i" "$(burn $secs)" '"cpus":1')")
    ids_b+=("$(run_job B n1 "fair-b$i" "$(burn $secs)" '"cpus":1')")
  done
  # Measure while both sides still have queued work (A needs 30 x 15 s / 2
  # slots = 225 s to drain): [t0+20, t0+140].
  local from=$((t0 + 20)) to=$((t0 + 140))
  sleep $((to - $(date +%s) + 2))
  cpu_in() {
    local server="$1" sum=0 s e j
    shift
    for j in "$@"; do
      read -r s e < <(job "$server" "$j" | jq -r '"\(.startedAt // 0) \(.endedAt // 0)"')
      [[ "$s" == 0 ]] && continue
      [[ "$e" == 0 ]] && e="$(date +%s)"
      ((s < from)) && s=$from
      ((e > to)) && e=$to
      ((e > s)) && sum=$((sum + e - s))
    done
    echo "$sum"
  }
  local ca cb
  ca="$(cpu_in A "${ids_a[@]}")"
  cb="$(cpu_in B "${ids_b[@]}")"
  ((cb > 0)) || die "B got no CPU at all (A $ca s)"
  # 2:1 within 10 %: 1.8 ≤ ca/cb ≤ 2.2 (integer math ×10).
  local r=$((ca * 10 / cb))
  ((r >= 18 && r <= 22)) || die "fair share: A $ca CPU-s, B $cb CPU-s (ratio $((r / 10)).$((r % 10)), want 2.0 ± 10 %)"
  pass "fair: A $ca CPU-s vs B $cb CPU-s (ratio $((r / 10)).$((r % 10)))"
  for j in "${ids_a[@]}"; do API_OK=any api A POST /cli/compute/jobs/cancel "{\"job\":\"$j\"}" >/dev/null; done
  for j in "${ids_b[@]}"; do API_OK=any api B POST /cli/compute/jobs/cancel "{\"job\":\"$j\"}" >/dev/null; done
}

# Kill B mid-job: its running job finishes, its queued ones wait, A's go on;
# restart B: logs replay, each job ends once (§16 tier 3 step 4).
s_reconnect() {
  ensure_shared
  local hold run q a
  hold="$(run_job A n1 rc-hold "$(burn 8)" '"cpus":1')"
  run="$(run_job B n1 rc-run '["sh","-c","for i in 1 2 3 4 5 6; do echo line$i; sleep 1; done"]' '"cpus":1')"
  wait_job B "$run" '.state == "running"' >/dev/null
  q="$(run_job B n1 rc-q '["sh","-c","echo queued-b"]' '"cpus":1,"exclusive":false')"
  stop_daemon B
  a="$(run_job A n1 rc-a '["sh","-c","echo a-while-b-away"]' '"cpus":1')"
  wait_job A "$a" '.state == "done"' 60 >/dev/null
  start_daemon B
  wait_online B n1
  local j
  j="$(wait_job B "$run" '.state == "done"' 60)"
  local lines
  lines="$(api B GET /cli/compute/jobs/logs '' "job=$run" | jq -r '[.chunks[] | select(.stream == "out") | .data | @base64d] | join("")' | grep -c '^line')"
  [[ "$lines" == 6 ]] || die "B's running job's logs after the reconnect: $lines of 6 lines"
  wait_job B "$q" '.state == "done"' 60 >/dev/null
  wait_job A "$hold" "$ended" 60 >/dev/null
  pass "reconnect: B's running job finished while away, logs replayed, queued job ran after"
}

# B down past node_lost_secs with an --any job queued on n1 and a free n2:
# re-placed without a retry; the returning node drops the stale row
# (§16 step 5, V47/V56). Needs K2_COMPUTE_NODE_LOST_SECS on B.
s_lost() {
  ensure_shared
  [[ "${K2X2_DAEMON_ENV:-}" == *K2_COMPUTE_NODE_LOST_SECS* ]] || die "lost: start B with K2X2_DAEMON_ENV='K2_COMPUTE_NODE_LOST_SECS=20' (the test knob)"
  [[ -d "$ROOT/node-n2" ]] || start_node n2 B
  api B POST /cli/compute/nodes/pause '{"node":"n2","action":"pause"}' >/dev/null
  local hold any
  hold="$(run_job A n1 lost-hold "$(burn 60)" '"exclusive":true')"
  wait_job A "$hold" '.state == "running"' >/dev/null
  any="$(api B POST /cli/compute/run "{\"any\":true,\"argv\":[\"sh\",\"-c\",\"echo moved\"],\"clientJobId\":\"k2x2-lost-any-00000000\",\"workspace\":\"$WS_B\",\"noSource\":true,\"detach\":true}" | jq -r '.job.id')"
  wait_job B "$any" '.queue != null or .nodeQueued == true or .reason == "node_queued"' 90 >/dev/null
  stop_node n1
  api B POST /cli/compute/nodes/pause '{"node":"n2","action":"resume"}' >/dev/null
  local j
  j="$(wait_job B "$any" '.state == "done"' 120)"
  printf '%s' "$j" | jq -e '.node == "n2" and .ctl.retriesUsed == 0 and .generation >= 2' >/dev/null ||
    die "lost: $(printf '%s' "$j" | jq -c '{node,generation,ctl}')"
  restart_node n1
  wait_online A n1
  pass "lost: a node-queued job re-placed with a new generation and no retry"
}

# Restart the node with jobs queued: queued rows survive, running ones are
# interrupted and retried (§16 step 7).
s_restart() {
  ensure_shared
  local hold q
  hold="$(run_job A n1 rs-hold "$(burn 30)" '"exclusive":true,"retryInterrupted":true')"
  wait_job A "$hold" '.state == "running"' >/dev/null
  q="$(run_job B n1 rs-q '["sh","-c","echo survived"]' '"cpus":1')"
  wait_job B "$q" '.queue != null or .reason == "node_queued"' 30 >/dev/null
  restart_node n1
  wait_online A n1
  wait_online B n1
  wait_job B "$q" '.state == "done"' 120 >/dev/null
  local h
  h="$(wait_job A "$hold" '.attempt >= 2 or .generation >= 2' 120)"
  pass "restart: the queued job survived; the running one was interrupted and retried ($(printf '%s' "$h" | jq -c '{attempt,generation,state}'))"
  API_OK=any api A POST /cli/compute/jobs/cancel "{\"job\":\"$hold\"}" >/dev/null
}

# A withdrawn job reruns under a new generation and runs once (V46, V71).
s_withdraw() {
  ensure_shared
  [[ -d "$ROOT/node-n2" ]] || start_node n2 B
  wait_online A n1
  wait_online B n1
  api B POST /cli/compute/nodes/pause '{"node":"n2","action":"pause"}' >/dev/null
  local hold any
  hold="$(run_job A n1 wd-hold "$(burn 150)" '"exclusive":true')"
  wait_job A "$hold" '.state == "running"' >/dev/null
  any="$(api B POST /cli/compute/run "{\"any\":true,\"argv\":[\"sh\",\"-c\",\"echo once >> /tmp/k2x2-withdraw-$$\"],\"clientJobId\":\"k2x2-wd-any-00000000\",\"workspace\":\"$WS_B\",\"noSource\":true,\"detach\":true}" | jq -r '.job.id')"
  # V71: an `--any` job waits on its server (any_hold, 60 s) for a node
  # that starts it now, then joins n1's queue; only a node-queued job is
  # withdrawn (a new generation).
  wait_job B "$any" '.node == "n1" and .reason == "node_queued" and .generation == 1' 120 >/dev/null
  api B POST /cli/compute/nodes/pause '{"node":"n2","action":"resume"}' >/dev/null
  local j
  j="$(wait_job B "$any" '.state == "done"' 120)"
  printf '%s' "$j" | jq -e '.node == "n2" and .generation >= 2 and .ctl.retriesUsed == 0' >/dev/null ||
    die "withdraw: $(printf '%s' "$j" | jq -c '{node,generation,ctl}')"
  [[ "$(wc -l <"/tmp/k2x2-withdraw-$$" | tr -d ' ')" == 1 ]] || die "withdraw: the job ran more than once"
  rm -f "/tmp/k2x2-withdraw-$$"
  API_OK=any api A POST /cli/compute/jobs/cancel "{\"job\":\"$hold\"}" >/dev/null
  pass "withdraw: moved to n2 under a new generation, ran exactly once"
}

# Revoke B --now: its job stops, its session closes, B sees the node revoked.
s_revoke() {
  ensure_shared
  local run q
  # One at a time for B (a narrowing applies at once), so its second waits.
  node_servers n1 limits slot:1 max_running=1 >/dev/null || die "limits max_running=1"
  sleep 3
  run="$(run_job B n1 rv-run "$(burn 60)" '"cpus":1')"
  wait_job B "$run" '.state == "running"' >/dev/null
  q="$(run_job B n1 rv-q '["sh","-c","echo never"]' '"cpus":1')"
  wait_job B "$q" '.queue != null or .reason == "node_queued"' 30 >/dev/null
  node_servers n1 revoke slot:1 --now --wipe >/dev/null || die "revoke"
  local j
  j="$(wait_job B "$run" "$ended" 60)"
  # B heard how it ended before its session closed: never lost, never retried.
  printf '%s' "$j" | jq -e '.state == "cancelled" and .reason == "share_revoked"' >/dev/null ||
    die "revoke: B's running job: $(printf '%s' "$j" | jq -c '{state,reason,generation}')"
  wait_job B "$q" '.state == "cancelled" or .reason == "share_revoked"' 60 >/dev/null
  for _ in $(seq 1 100); do
    node_row B n1 | jq -e '.state == "revoked" or .online == false' >/dev/null && break
    sleep 0.2
  done
  node_row B n1 | jq -e '.state == "revoked" or .online == false' >/dev/null || die "B still sees n1: $(node_row B n1 | jq -c '{state,online}')"
  node_row A n1 | jq -e '.online == true' >/dev/null || die "A lost n1 when B was revoked"
  # Clean slate for later scenarios: B forgets n1, the machine owner wipes
  # the slot now (`--wipe`), and the next share starts fresh.
  api B POST /cli/compute/nodes/remove '{"node":"n1"}' >/dev/null
  node_servers n1 prune >/dev/null || die "prune after revoke --wipe"
  node_servers n1 list --json | jq -e '[.file.servers[] | select(.slot == 1)] | length == 0' >/dev/null ||
    die "revoke: slot 1 not wiped: $(node_servers n1 list --json | jq -c '.file.servers')"
  pass "revoke: B's job stopped (cancelled/share_revoked), its queue cleared, A untouched"
}

# A share past its end date (node clock jumped with the test knob):
# queued jobs end share_expired, running ones finish.
s_expiry() {
  ensure_shared
  local hold q
  node_servers n1 limits slot:1 --for 1h >/dev/null || die "limits --for 1h"
  hold="$(run_job A n1 ex-hold "$(burn 40)" '"exclusive":true')"
  wait_job A "$hold" '.state == "running"' >/dev/null
  q="$(run_job B n1 ex-q '["sh","-c","echo never"]' '"cpus":1')"
  wait_job B "$q" '.reason == "node_queued" or .queue != null' 30 >/dev/null
  # An hour and a bit later on the node's clock: B's share has ended.
  restart_node n1 "$BIN/k2-node" +3700
  wait_online A n1
  local j
  j="$(wait_job B "$q" "$ended" 90)"
  printf '%s' "$j" | jq -e '.state == "cancelled" and .reason == "share_expired"' >/dev/null ||
    die "expiry: B's queued job: $(printf '%s' "$j" | jq -c '{state,reason}')"
  local late
  late="$(API_OK=any api B POST /cli/compute/run "{\"node\":\"n1\",\"argv\":[\"sh\",\"-c\",\"echo late\"],\"clientJobId\":\"k2x2-ex-late-00000000\",\"workspace\":\"$WS_B\",\"noSource\":true,\"detach\":true}")"
  printf '%s' "$late" | grep -q 'share_expired' || {
    local id
    id="$(printf '%s' "$late" | jq -r '.job.id // empty')"
    [[ -n "$id" ]] && wait_job B "$id" '.reason == "share_expired"' 60 >/dev/null
  } || die "expiry: new work from B wasn't refused share_expired: $late"
  API_OK=any api A POST /cli/compute/jobs/cancel "{\"job\":\"$hold\"}" >/dev/null
  # Clean slate for later scenarios: the node's clock back, B forgets n1,
  # the ended share is wiped.
  restart_node n1
  wait_online A n1
  api B POST /cli/compute/nodes/remove '{"node":"n1"}' >/dev/null
  node_servers n1 revoke slot:1 --wipe >/dev/null || die "expiry: wipe slot 1"
  node_servers n1 prune >/dev/null || true
  pass "expiry: past its end date B's queued job ended share_expired and new work is refused"
}

# Clock skew (§16 step 10): health and nodes show it; order unchanged.
s_skew() {
  ensure_shared
  restart_node n1 "$BIN/k2-node" +600
  wait_online A n1
  for _ in $(seq 1 150); do
    node_row A n1 | jq -e '((.share.skewSecs // .skewSecs // 0) | . >= 590 and . <= 610)' >/dev/null 2>&1 && break
    sleep 0.2
  done
  node_row A n1 | jq -e '((.share.skewSecs // .skewSecs // 0) | . >= 590 and . <= 610)' >/dev/null ||
    die "skew: A doesn't show the node 10 min ahead: $(node_row A n1 | jq -c '{skew: (.share.skewSecs // .skewSecs)}')"
  k2 A compute nodes 2>&1 | grep -q 'skew' || die "skew: k2 compute nodes doesn't show it: $(k2 A compute nodes 2>&1 | head -5)"
  local j
  j="$(run_job A n1 skew-run '["sh","-c","echo fine"]')"
  wait_job A "$j" '.state == "done"' 60 >/dev/null
  restart_node n1
  wait_online A n1
  pass "skew: a node 10 min ahead shows its skew and still runs jobs"
}

# C2/C3: a 0.46.2 owner against the 0.46.3 node: solo as before; shared,
# it runs only when nobody else waits (fair_share_wait otherwise).
s_old_owner() {
  old_build
  start_daemon O "$OLD_DIR/target/debug/k2-daemon"
  seed_ws O
  start_node n9 O
  local j
  j="$(run_job O n9 oldowner '["sh","-c","echo from-0462"]')"
  wait_job O "$j" '.state == "done"' 60 >/dev/null
  pass "old-owner: a 0.46.2 owner runs jobs on a 0.46.3 solo node"
}

# C5: a 0.46.2 daemon as a NEW server: SAS confirms, then attach refused.
s_old_tenant() {
  old_build
  [[ -n "${PORT_O:-}" ]] || { start_daemon O "$OLD_DIR/target/debug/k2-daemon"; seed_ws O; }
  ensure_a
  share_add n1 A O --pending >/dev/null
  for _ in $(seq 1 100); do
    k2 A compute node servers n1 --json --refresh | jq -e '[.servers[] | select(.state == "refused" and ((.refusal // .reason) | tostring | contains("server_too_old")))] | length == 1' >/dev/null && break
    sleep 0.2
  done
  k2 A compute node servers n1 --json --refresh | jq -e '[.servers[] | select(.state == "refused" and ((.refusal // .reason) | tostring | contains("server_too_old")))] | length == 1' >/dev/null ||
    die "old-tenant: no server_too_old: $(k2 A compute node servers n1 --json)"
  node_row O n1 | jq -e '.online != true' >/dev/null || die "old-tenant: the 0.46.2 server attached"
  pass "old-tenant: a 0.46.2 server is refused server_too_old after its SAS"
}

# ── job tiers (prd-compute-job-tiers-v1, 0.46.5) ─────────────────────

# The server's own queue for a solo node, queued entries' short ids in order.
queued_order() { api "$1" GET /cli/compute/queue '' "node=$2" | jq -r '[.entries[] | select(.state == "queued") | .short] | join(",")'; }

# Grants that never get in the way of the node's own room (the node's
# max_parallel is 3; a grant cap of 1 would refuse before the node does).
wide_grant() {
  local server="$1" node="$2" ws="WS_$1"
  api "$server" POST /cli/compute/grants/set "{\"node\":\"$node\",\"workspace\":\"${!ws}\",\"maxParallel\":8,\"maxQueued\":30}" >/dev/null
}

# Tiers on a solo node: the server orders CI, then Priority, then Normal;
# a Priority job waiting for room holds a later Normal one (the next free
# slot); nothing running is ever stopped.
s_tiers_solo() {
  ensure_a
  wide_grant A n1
  api A POST /cli/compute/nodes/pause '{"node":"n1","action":"pause"}' >/dev/null
  local n p c
  n="$(run_job A n1 ts-n '["true"]')"
  p="$(run_job A n1 ts-p '["true"]' '"tier":"priority"')"
  c="$(run_job A n1 ts-c '["true"]' '"tier":"ci"')"
  local order
  order="$(queued_order A n1)"
  [[ "$order" == "${c:0:8},${p:0:8},${n:0:8}" ]] || die "tiers-solo: queue order $order (want CI ${c:0:8}, P ${p:0:8}, N ${n:0:8})"
  api A GET /cli/compute/queue '' node=n1 | jq -e '.tiers == {"ci": 1, "priority": 1, "normal": 1}' >/dev/null ||
    die "tiers-solo: counts $(api A GET /cli/compute/queue '' node=n1 | jq -c .tiers)"
  api A POST /cli/compute/nodes/pause '{"node":"n1","action":"resume"}' >/dev/null
  local a_c a_p a_n
  a_c="$(wait_job A "$c" '.state == "done"' | jq -r '.assignedAt')"
  a_p="$(wait_job A "$p" '.state == "done"' | jq -r '.assignedAt')"
  a_n="$(wait_job A "$n" '.state == "done"' | jq -r '.assignedAt')"
  ((a_c <= a_p && a_p <= a_n)) || die "tiers-solo: assigned CI $a_c, P $a_p, N $a_n"
  # The next free slot: three runs fill the node; a Priority job waits for
  # room and holds a later Normal one, which says why.
  local b1 b2 b3 p2 n2
  b1="$(run_job A n1 ts-b1 "$(burn 8)" '"cpus":1')"
  b2="$(run_job A n1 ts-b2 "$(burn 8)" '"cpus":1')"
  b3="$(run_job A n1 ts-b3 "$(burn 8)" '"cpus":1')"
  for b in "$b1" "$b2" "$b3"; do wait_job A "$b" '.state == "running"' 30 >/dev/null; done
  p2="$(run_job A n1 ts-p2 '["true"]' '"tier":"priority","cpus":1')"
  n2="$(run_job A n1 ts-n2 '["true"]' '"cpus":1')"
  wait_job A "$n2" '.state == "queued" and .reason == "tier_hold"' 20 >/dev/null
  local s_p s_n
  s_p="$(wait_job A "$p2" '.state == "done"' 60 | jq -r '.startedAt')"
  s_n="$(wait_job A "$n2" '.state == "done"' 60 | jq -r '.startedAt')"
  ((s_p <= s_n)) || die "tiers-solo: Priority started $s_p, after the Normal job ($s_n)"
  for b in "$b1" "$b2" "$b3"; do
    wait_job A "$b" '.state == "done" and .exitCode == 0' 60 >/dev/null || die "tiers-solo: a running job was stopped"
  done
  pass "tiers-solo: CI, Priority, Normal; Priority took the next free slot; nothing running stopped"
}

# Tiers on a shared node (B is a guest without gate or Priority): B's
# Priority runs as Normal with a note, B's CI is refused; A's CI jumps B's
# queued Normal job on the node; the node's owner lets B's Priority count
# (queue.toml) and B's offer says so.
s_tiers_shared() {
  ensure_shared
  wide_grant A n1
  wide_grant B n1
  local out
  out="$(api B POST /cli/compute/run "{\"node\":\"n1\",\"argv\":[\"true\"],\"clientJobId\":\"k2x2-sh-bp-00000000\",\"workspace\":\"$WS_B\",\"noSource\":true,\"detach\":true,\"tier\":\"priority\"}")"
  printf '%s' "$out" | jq -e '.job.tier == "normal" and .job.tierAsked == "priority" and ([.notices[].code] | index("priority_downgraded") != null)' >/dev/null ||
    die "tiers-shared: B's Priority on a guest share: $(printf '%s' "$out" | jq -c '{tier: .job.tier, asked: .job.tierAsked, notices}')"
  out="$(API_OK=any api B POST /cli/compute/run "{\"node\":\"n1\",\"argv\":[\"true\"],\"clientJobId\":\"k2x2-sh-bc-00000000\",\"workspace\":\"$WS_B\",\"noSource\":true,\"detach\":true,\"tier\":\"ci\"}")"
  printf '%s' "$out" | jq -e '.error.code == "gate_not_allowed"' >/dev/null || die "tiers-shared: B's CI without gate: $out"
  local b1 b2 b3 bn ac
  b1="$(run_job B n1 sh-b1 "$(burn 10)" '"cpus":1')"
  b2="$(run_job B n1 sh-b2 "$(burn 10)" '"cpus":1')"
  b3="$(run_job B n1 sh-b3 "$(burn 10)" '"cpus":1')"
  for b in "$b1" "$b2" "$b3"; do wait_job B "$b" '.state == "running"' 30 >/dev/null; done
  bn="$(run_job B n1 sh-bn '["true"]' '"cpus":1')"
  wait_job B "$bn" '.nodeQueued == true' 30 >/dev/null
  ac="$(run_job A n1 sh-ac '["true"]' '"tier":"ci","cpus":1')"
  # B sees a CI job ahead of its own (another server's: no id).
  local v
  for _ in $(seq 1 50); do
    v="$(k2 B compute queue n1 --json)"
    printf '%s' "$v" | jq -e '[.entries[] | select(.state == "queued")][0] | .tier == "ci" and .mine == false' >/dev/null && break
    sleep 0.2
  done
  printf '%s' "$v" | jq -e '[.entries[] | select(.state == "queued")][0] | .tier == "ci" and .mine == false' >/dev/null ||
    die "tiers-shared: B's view doesn't show the CI job first: $v"
  printf '%s' "$v" | jq -e '.tiers.ci >= 1' >/dev/null || die "tiers-shared: no tier counts: $v"
  local s_ac s_bn
  s_ac="$(wait_job A "$ac" '.state == "done"' 60 | jq -r '.startedAt')"
  s_bn="$(wait_job B "$bn" '.state == "done"' 60 | jq -r '.startedAt')"
  ((s_ac <= s_bn)) || die "tiers-shared: A's CI started $s_ac, after B's older Normal job ($s_bn)"
  for b in "$b1" "$b2" "$b3"; do
    wait_job B "$b" '.state == "done" and .exitCode == 0' 60 >/dev/null || die "tiers-shared: CI stopped a running job"
  done
  # The node's owner lets B's Priority count (queue.toml, at the machine).
  node_servers n1 limits slot:1 priority=true >/dev/null || die "servers limits priority=true"
  grep -q 'priority = true' "$ROOT/node-n1/config/queue.toml" || die "no queue.toml: $(cat "$ROOT/node-n1/config/queue.toml" 2>&1)"
  grep -q 'priority' "$ROOT/node-n1/config/servers.toml" && die "the Priority word leaked into servers.toml (an old k2-node couldn't read it)"
  for _ in $(seq 1 100); do
    node_row B n1 | jq -e '.share.priority == true' >/dev/null 2>&1 && break
    sleep 0.2
  done
  node_row B n1 | jq -e '.share.priority == true' >/dev/null || die "B's offer never said its Priority counts: $(node_row B n1 | jq -c .share)"
  out="$(api B POST /cli/compute/run "{\"node\":\"n1\",\"argv\":[\"true\"],\"clientJobId\":\"k2x2-sh-bp-00000001\",\"workspace\":\"$WS_B\",\"noSource\":true,\"detach\":true,\"tier\":\"priority\"}")"
  printf '%s' "$out" | jq -e '.job.tier == "priority" and ([.notices[].code] | index("priority_downgraded") == null)' >/dev/null ||
    die "tiers-shared: B's Priority after the owner's word: $(printf '%s' "$out" | jq -c '{tier: .job.tier, notices}')"
  pass "tiers-shared: guest Priority downgraded then allowed; guest CI refused; CI jumped the node's queue; nothing stopped"
}

# Mixed versions (§3.6): a 0.46.5 server orders a v0.46.2 solo node by
# tier (the server holds a solo node's queue), and the plans it sends carry
# no tier (an old node would refuse the digest).
s_tiers_old_node() {
  old_build
  [[ -n "${PORT_A:-}" ]] || { start_daemon A; seed_ws A; }
  [[ -d "$ROOT/node-n0" ]] || start_node n0 A "$OLD_DIR/target/debug/k2-node"
  node_row A n0 | jq -e '.offer.protocol == 2' >/dev/null || die "n0 isn't protocol 2"
  wide_grant A n0
  api A POST /cli/compute/nodes/pause '{"node":"n0","action":"pause"}' >/dev/null
  local n p c
  n="$(run_job A n0 to-n '["true"]')"
  p="$(run_job A n0 to-p '["true"]' '"tier":"priority"')"
  c="$(run_job A n0 to-c '["true"]' '"tier":"ci"')"
  [[ "$(queued_order A n0)" == "${c:0:8},${p:0:8},${n:0:8}" ]] || die "tiers-old-node: order $(queued_order A n0)"
  api A POST /cli/compute/nodes/pause '{"node":"n0","action":"resume"}' >/dev/null
  for j in "$c" "$p" "$n"; do wait_job A "$j" '.state == "done" and .exitCode == 0' 60 >/dev/null; done
  pass "tiers-old-node: a v0.46.2 node runs CI, Priority, Normal in tier order"
}

# Rollback (T11, T17): a Priority job queued on the server for a 0.46.5
# node (its plan carries the tier) still runs after the node is rolled
# back to the old binary: the plan loses its tier before it is sent.
s_tiers_rollback() {
  old_build
  ensure_a
  wide_grant A n1
  api A POST /cli/compute/nodes/pause '{"node":"n1","action":"pause"}' >/dev/null
  local p
  p="$(run_job A n1 tr-p '["sh","-c","echo tiered"]' '"tier":"priority"')"
  restart_node n1 "$OLD_DIR/target/debug/k2-node"
  for _ in $(seq 1 150); do
    node_row A n1 | jq -e '.online == true and .offer.protocol == 2' >/dev/null 2>&1 && break
    sleep 0.2
  done
  node_row A n1 | jq -e '.offer.protocol == 2' >/dev/null || die "tiers-rollback: n1 isn't on the old binary"
  api A POST /cli/compute/nodes/pause '{"node":"n1","action":"resume"}' >/dev/null
  wait_job A "$p" '.state == "done" and .exitCode == 0' 60 >/dev/null || die "tiers-rollback: the tiered job didn't run on the rolled-back node"
  job A "$p" | jq -e '.tier == "priority"' >/dev/null || die "tiers-rollback: the server forgot the tier"
  restart_node n1
  wait_online A n1
  for _ in $(seq 1 150); do
    node_row A n1 | jq -e '.offer.protocol >= 5' >/dev/null 2>&1 && break
    sleep 0.2
  done
  node_row A n1 | jq -e '(.offer.features | index("tiers")) != null' >/dev/null || die "tiers-rollback: n1 back on 0.46.5 doesn't offer tiers"
  pass "tiers-rollback: a tiered job runs on the rolled-back node; tiers come back with the new binary"
}

# ── main ─────────────────────────────────────────────────────────────

if [[ "${1:-}" == list ]]; then
  printf '%s\n' "${ALL_SCENARIOS[@]}"
  K2X2_KEEP=0
  exit 0
fi
if [[ "${1:-}" == old-build ]]; then
  old_build
  exit 0
fi
SCENARIOS=("${@:-${ALL_SCENARIOS[@]}}")
for s in "${SCENARIOS[@]}"; do
  [[ " ${ALL_SCENARIOS[*]} " == *" $s "* ]] || die "unknown scenario $s (try: list)"
done
say "root $ROOT"
build
for s in "${SCENARIOS[@]}"; do
  say "── $s"
  "s_${s//-/_}"
done
say "all passed: ${SCENARIOS[*]}"
