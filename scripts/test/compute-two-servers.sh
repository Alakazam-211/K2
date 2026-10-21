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
PIDS=()

ALL_SCENARIOS=(solo old-node rollback share both fair reconnect lost restart withdraw revoke expiry skew old-owner old-tenant)

# To stderr: a die inside $(...) must still reach the terminal.
say() { printf '[k2x2 %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() {
  say "FAIL: $*"
  say "logs: $ROOT (kept)"
  K2X2_KEEP=1
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
  if [[ "${K2X2_KEEP:-0}" != 1 ]]; then
    rm -rf "$ROOT"
  else
    say "kept $ROOT"
  fi
}
trap cleanup EXIT

# ── processes ────────────────────────────────────────────────────────

clean() { env -i HOME="$1" PATH="$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 SHELL=/bin/sh "${@:2}"; }

build() {
  [[ "${K2X2_NO_BUILD:-0}" == 1 ]] && return
  say "building k2-daemon and k2-node (debug, this worktree's target/)"
  (cd "$REPO" && clean "$HOME" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
    cargo build -q -p k2-daemon -p k2-node) || die "build"
}

# start_daemon NAME [binary]: a daemon in $ROOT/NAME; sets PORT_<NAME>, TOKEN_<NAME>.
start_daemon() {
  local name="$1" bin="${2:-$BIN/k2-daemon}" h="$ROOT/$1"
  mkdir -p "$h/.k2" "$h/tmp" "$h/run" "$h/agent-shim-empty"
  chmod 700 "$h/tmp" "$h/run"
  rm -f "$h/.k2/daemon.token"
  # Keep the port across restarts (the daemon reuses daemon.port if free).
  clean "$h" TMPDIR="$h/tmp" XDG_RUNTIME_DIR="$h/run" K2_RUNTIME_DIR= \
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
  clean "$d" "$bin" run --home "$d/home" --config-dir "$d/config" --dev >>"$d/node.log" 2>&1 &
  PIDS+=("$!")
  disown "$!"
  echo "$!" >"$d/pid"
  api "$server" POST /cli/compute/nodes/confirm "{\"node\":\"$node\",\"sas\":\"$sas\"}" >/dev/null
  wait_online "$server" "$node"
  say "node $node enrolled with $server and online"
}

stop_node() { local pid; pid="$(cat "$ROOT/node-$1/pid")"; kill "$pid" 2>/dev/null || true; sleep 1; kill -9 "$pid" 2>/dev/null || true; }

restart_node() {
  local node="$1" bin="${2:-$BIN/k2-node}" d="$ROOT/node-$1"
  stop_node "$node"
  clean "$d" ${K2X2_NODE_ENV:-} "$bin" run --home "$d/home" --config-dir "$d/config" --dev >>"$d/node.log" 2>&1 &
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
  (cd "$REPO" && git archive --format=tar v0.46.2) | tar -x -C "$OLD_DIR/src"
  (cd "$OLD_DIR/src" && clean "$HOME" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
    CARGO_TARGET_DIR="$OLD_DIR/target" cargo build -q -p k2-daemon -p k2-node) || die "v0.46.2 build"
}

# ── sharing steps (S1 + S4a; the PRD §9 verbs) ───────────────────────

# share_add NODE OWNER OTHER [extra add flags]: OTHER mints a code, OWNER
# adds OTHER (approved), OTHER confirms the SAS the owner shows.
share_add() {
  local node="$1" owner="$2" other="$3"
  shift 3
  local code req sas
  code="$(k2 "$other" compute node add "$node" --url "$(url_of "$other")" --json | jq -r '.enroll')"
  [[ "$code" == *.* ]] || die "no code from $other"
  req="$(k2 "$owner" compute node servers "$node" add --url "$(url_of "$other")" --code "$code" --trust own "$@" --json)" ||
    die "servers add: $req"
  # Approval: the vault confirm with the test key (never Touch ID here).
  k2 "$owner" compute node servers "$node" approve "$(printf '%s' "$req" | jq -r '.request.id // .id')" --json >/dev/null ||
    die "approve the add"
  for _ in $(seq 1 150); do
    sas="$(k2 "$owner" compute node servers "$node" --json | jq -r --arg u "$(url_of "$other")" '.servers[] | select(.state == "pending") | .sas // empty' | tr -d ' ')"
    [[ ${#sas} == 6 ]] && break
    sleep 0.2
  done
  [[ ${#sas} == 6 ]] || die "no pending SAS for $other on $node"
  k2 "$other" compute node confirm "$node" "$sas" >/dev/null || die "confirm on $other"
}

# share_node NODE OWNER OTHER [flags]: share_add, then both see it shared.
share_node() {
  local node="$1" owner="$2" other="$3"
  share_add "$@"
  wait_online "$other" "$node"
  node_row "$other" "$node" | jq -e '.share.shared == true and .role == "tenant"' >/dev/null ||
    die "$other doesn't see $node as shared: $(node_row "$other" "$node" | jq -c '{role,share}')"
  node_row "$owner" "$node" | jq -e '.role == "owner" and .share.shared == true' >/dev/null ||
    die "$owner doesn't see $node as shared"
}

# The owner A with its workspace and node n1, made once per run.
ensure_a() {
  [[ -n "${PORT_A:-}" ]] || { start_daemon A; seed_ws A; }
  [[ -d "$ROOT/node-n1" ]] || start_node n1 A
}

# ── scenarios ────────────────────────────────────────────────────────

s_solo() {
  ensure_a
  local j
  j="$(run_job A n1 solo '["sh","-c","echo hello; exit 3"]')"
  j="$(wait_job A "$j" "$ended")"
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
    node_row A n1 | jq -e '.online == true and .offer.protocol == 3' >/dev/null 2>&1 && break
    sleep 0.2
  done
  node_row A n1 | jq -e '.offer.protocol == 3' >/dev/null || die "rollback: n1 didn't come back on 0.46.3"
  pass "rollback: the owner works on the v0.46.2 node binary and again after rolling forward"
}

s_share() {
  ensure_a
  start_daemon B
  seed_ws B
  share_node n1 A B --max-running 3
  pass "share: B added, approved, SAS confirmed; both see n1 shared"
}

s_both() {
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
  local n=14 secs=15 ids_a=() ids_b=() i t0 req
  k2 A compute node servers n1 --json | jq -e '[.servers[] | .share.weight // .weight] | all(. == 1)' >/dev/null ||
    die "fair: servers don't all start at weight 1: $(k2 A compute node servers n1 --json)"
  req="$(k2 A compute node servers n1 limits slot:0 --weight 2 --json)" || die "limits --weight: $req"
  k2 A compute node servers n1 approve "$(printf '%s' "$req" | jq -r '.request.id // .id')" --json >/dev/null || die "approve the weight"
  for _ in $(seq 1 100); do
    k2 A compute node servers n1 --json | jq -e '[.servers[] | select(.slot == 0) | .share.weight // .weight] == [2]' >/dev/null && break
    sleep 0.2
  done
  t0="$(date +%s)"
  for i in $(seq 1 $n); do
    ids_a+=("$(run_job A n1 "fair-a$i" "$(burn $secs)" '"cpus":1')")
    ids_b+=("$(run_job B n1 "fair-b$i" "$(burn $secs)" '"cpus":1')")
  done
  # Measure while both sides still have queued work: [t0+20, t0+20+150].
  local from=$((t0 + 20)) to=$((t0 + 170))
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
  for j in "${ids_a[@]}"; do api A POST /cli/compute/jobs/cancel "{\"job\":\"$j\"}" >/dev/null; done
  for j in "${ids_b[@]}"; do api B POST /cli/compute/jobs/cancel "{\"job\":\"$j\"}" >/dev/null; done
}

# Kill B mid-job: its running job finishes, its queued ones wait, A's go on;
# restart B: logs replay, each job ends once (§16 tier 3 step 4).
s_reconnect() {
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
  [[ "${K2X2_DAEMON_ENV:-}" == *K2_COMPUTE_NODE_LOST_SECS* ]] || die "lost: start B with K2X2_DAEMON_ENV='K2_COMPUTE_NODE_LOST_SECS=20' (the test knob)"
  start_node n2 B
  api B POST /cli/compute/nodes/pause '{"node":"n2","action":"pause"}' >/dev/null
  local hold any
  hold="$(run_job A n1 lost-hold "$(burn 60)" '"cpus":3')"
  any="$(api B POST /cli/compute/run "{\"any\":true,\"argv\":[\"sh\",\"-c\",\"echo moved\"],\"clientJobId\":\"k2x2-lost-any-00000000\",\"workspace\":\"$WS_B\",\"noSource\":true,\"detach\":true}" | jq -r '.job.id')"
  wait_job B "$any" '.nodeQueued == true or .reason == "node_queued"' 90 >/dev/null
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
  local hold q
  hold="$(run_job A n1 rs-hold "$(burn 30)" '"cpus":3,"retryInterrupted":true')"
  wait_job A "$hold" '.state == "running"' >/dev/null
  q="$(run_job B n1 rs-q '["sh","-c","echo survived"]' '"cpus":1')"
  wait_job B "$q" '.reason == "node_queued"' 30 >/dev/null
  restart_node n1
  wait_online A n1
  wait_online B n1
  wait_job B "$q" '.state == "done"' 120 >/dev/null
  local h
  h="$(wait_job A "$hold" '.attempt >= 2 or .generation >= 2' 120)"
  pass "restart: the queued job survived; the running one was interrupted and retried ($(printf '%s' "$h" | jq -c '{attempt,generation,state}'))"
  api A POST /cli/compute/jobs/cancel "{\"job\":\"$hold\"}" >/dev/null
}

# A withdrawn job reruns under a new generation and runs once (V46, V71).
s_withdraw() {
  [[ -n "$(node_row B n2 2>/dev/null)" ]] || start_node n2 B
  api B POST /cli/compute/nodes/pause '{"node":"n2","action":"pause"}' >/dev/null
  local hold any
  hold="$(run_job A n1 wd-hold "$(burn 40)" '"cpus":3')"
  any="$(api B POST /cli/compute/run "{\"any\":true,\"argv\":[\"sh\",\"-c\",\"echo once >> /tmp/k2x2-withdraw-$$\"],\"clientJobId\":\"k2x2-wd-any-00000000\",\"workspace\":\"$WS_B\",\"noSource\":true,\"detach\":true}" | jq -r '.job.id')"
  wait_job B "$any" '.node == "n1"' 120 >/dev/null
  api B POST /cli/compute/nodes/pause '{"node":"n2","action":"resume"}' >/dev/null
  local j
  j="$(wait_job B "$any" '.state == "done"' 120)"
  printf '%s' "$j" | jq -e '.node == "n2" and .generation >= 2 and .ctl.retriesUsed == 0' >/dev/null ||
    die "withdraw: $(printf '%s' "$j" | jq -c '{node,generation,ctl}')"
  [[ "$(wc -l <"/tmp/k2x2-withdraw-$$" | tr -d ' ')" == 1 ]] || die "withdraw: the job ran more than once"
  rm -f "/tmp/k2x2-withdraw-$$"
  api A POST /cli/compute/jobs/cancel "{\"job\":\"$hold\"}" >/dev/null
  pass "withdraw: moved to n2 under a new generation, ran exactly once"
}

# Revoke B --now: its job stops, its session closes, B sees the node revoked.
s_revoke() {
  local run q
  run="$(run_job B n1 rv-run "$(burn 60)" '"cpus":1')"
  wait_job B "$run" '.state == "running"' >/dev/null
  q="$(run_job B n1 rv-q '["sh","-c","echo never"]' '"cpus":3')"
  k2 A compute node servers n1 revoke "$(url_of B | sed 's|http://||')" --now >/dev/null || die "revoke"
  wait_job B "$run" "$ended" 60 >/dev/null
  wait_job B "$q" '.state == "cancelled" or .reason == "share_revoked"' 60 >/dev/null
  for _ in $(seq 1 100); do
    node_row B n1 | jq -e '.state == "revoked" or .online == false' >/dev/null && break
    sleep 0.2
  done
  node_row B n1 | jq -e '.state == "revoked" or .online == false' >/dev/null || die "B still sees n1: $(node_row B n1 | jq -c '{state,online}')"
  node_row A n1 | jq -e '.online == true' >/dev/null || die "A lost n1 when B was revoked"
  pass "revoke: B's job stopped, its queue cleared, A untouched"
}

# A share past its end date (node clock jumped with the test knob):
# queued jobs end share_expired, running ones finish.
s_expiry() {
  die "expiry: needs the node's test clock offset (K2_NODE_FAKE_CLOCK_OFFSET) and a fresh B share --for 1h; wire at S5"
}

# Clock skew (§16 step 10): health and nodes show it; order unchanged.
s_skew() {
  die "skew: needs K2_NODE_FAKE_CLOCK_OFFSET=+600 on the node; wire at S5"
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
  share_add n1 A O
  for _ in $(seq 1 100); do
    k2 A compute node servers n1 --json | jq -e '[.servers[] | select(.state == "refused" and .reason == "server_too_old")] | length == 1' >/dev/null && break
    sleep 0.2
  done
  k2 A compute node servers n1 --json | jq -e '[.servers[] | select(.state == "refused" and .reason == "server_too_old")] | length == 1' >/dev/null ||
    die "old-tenant: no server_too_old: $(k2 A compute node servers n1 --json)"
  node_row O n1 | jq -e '.online != true' >/dev/null || die "old-tenant: the 0.46.2 server attached"
  pass "old-tenant: a 0.46.2 server is refused server_too_old after its SAS"
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
