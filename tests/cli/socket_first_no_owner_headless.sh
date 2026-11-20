#!/usr/bin/env bash
# In a session, a socket-first sender's TCP fallback never carries the
# owner token from disk (0.46.17), against the real gate: a foreground
# sandbox daemon built from THIS worktree (temp HOME, no client), the REAL
# k2-node (--dev, temp home), and this checkout's CLI in a real agent
# session (the passport and cell socket K2 hands the agent's pinned chat).
#
#   1. k2 send-feedback settings from the agent's session, with its cell
#      socket unreachable (a dead socket, then a missing path): refused by
#      the daemon or the CLI, never written as the owner. The owner reads
#      the setting before and after: unchanged.
#   2. A compute read (k2 compute nodes) and a compute write (k2 compute
#      run --detach) from the session: the normal path through the cell
#      socket still works. Loopback TCP points at a closed port for these
#      runs, so an answer can only have come over the socket; the job's
#      record says the agent submitted it, not the owner token.
#   3. A TCP-only Open verb from the session (k2 feedback list, which
#      sends cli_request over TCP) still answers as before. This row
#      documents today's behaviour: it sends the owner token from disk,
#      which a later change to the TCP-only senders will end.
#
#   cargo build -p k2-daemon -p k2-node && bash tests/cli/socket_first_no_owner_headless.sh
#
# Every check fails loudly with the output. Opens ports: run it on K2 Compute.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 1; }
command -v git >/dev/null || { echo "FAIL: git is required" >&2; exit 1; }
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON_BIN="$TARGET/debug/k2-daemon"
NODE_BIN="$TARGET/debug/k2-node"
for b in "$DAEMON_BIN" "$NODE_BIN"; do
    [ -x "$b" ] || { echo "FAIL: $b not built (cargo build -p k2-daemon -p k2-node)" >&2; exit 1; }
done

# Never inherit a session's identity from the caller.
for v in $(env | grep -E '^(K2|K2SO|K5)_' | cut -d= -f1); do unset "$v"; done

# Short root: the cell socket must fit SUN_LEN.
WORK="$(mktemp -d /tmp/k2sfh.XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
SB="$WORK/h"
NODE_HOME="$WORK/node/home"
NODE_CFG="$WORK/node/etc"
mkdir -p "$SB/.k2" "$SB/shim" "$SB/tmp" "$SB/run" "$NODE_HOME" "$NODE_CFG"
DAEMON_PID=""
NODE_PID=""
DLOG="$SB/daemon.log"
pass=0
fail=0
cleanup() {
    for p in "$NODE_PID" "$DAEMON_PID"; do
        [ -n "$p" ] || continue
        kill "$p" 2>/dev/null || true
        for _ in $(seq 1 100); do kill -0 "$p" 2>/dev/null || break; sleep 0.1; done
        kill -9 "$p" 2>/dev/null || true
        wait "$p" 2>/dev/null || true
    done
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2; tail -40 "$DLOG" >&2 2>/dev/null || true
        echo "--- node log tail ---" >&2; tail -20 "$WORK/node.log" >&2 2>/dev/null || true
    fi
    chmod -R u+rwx "$WORK" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

ok()  { pass=$((pass + 1)); echo "  PASS: $*"; }
bad() { fail=$((fail + 1)); echo "  FAIL: $*" >&2; }
# die: a setup step failed; nothing after it can run.
die() { bad "$*"; exit 1; }
show() { echo "    rc=$RC stdout=$(printf %q "$OUT") stderr=$(printf %q "$ERR")" >&2; }
jget() { python3 -c 'import json,sys; j=json.loads(sys.argv[1]); print(eval(sys.argv[2], {"j": j}))' "$1" "$2"; }

echo "== daemon =="
cat >"$SB/shim/claude" <<EOF
#!/bin/sh
env > "$SB/claude.env"
exec /bin/cat > /dev/null
EOF
chmod +x "$SB/shim/claude"
cat >"$NODE_CFG/policy.toml" <<'EOF'
availability = "always"
foreign_locks = []
write_smoke_lock = ""
disk_floor_gb = 0
max_parallel = 2
EOF
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= \
    HOME="$SB" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    K2_COMPUTE=1 K2_AIRGAP=1 K2_NODE_HOME="$NODE_HOME" K2_NODE_CONFIG_DIR="$NODE_CFG" \
    "$DAEMON_BIN" >"$DLOG" 2>&1 &
DAEMON_PID=$!
phase=""
for _ in $(seq 1 400); do
    if [ -s "$SB/.k2/daemon.port" ] && [ -s "$SB/.k2/daemon.token" ]; then
        DPORT="$(cat "$SB/.k2/daemon.port")"
        phase="$(curl -s "http://127.0.0.1:$DPORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
        [ "$phase" = "ready" ] && break
    fi
    sleep 0.1
done
[ "$phase" = "ready" ] || die "daemon never reached ready"
DTOKEN="$(cat "$SB/.k2/daemon.token")"
BASE="http://127.0.0.1:$DPORT"
ok "sandbox daemon ready ($DAEMON_BIN)"

# cli [K=V ...] -- <k2 args>: one run of the CLI with HOME=$SB and the
# given environment only. OUT, ERR, RC.
cli() {
    local envs=(PATH="$PATH" HOME="$SB" TMPDIR="$SB/tmp" K2_HOST=127.0.0.1)
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    set +e
    OUT="$(cd "$WS" && env -i "${envs[@]}" "$K2_CLI" "$@" </dev/null 2>"$WORK/cli.err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/cli.err")"
}
# owner <k2 args>: the owner's own terminal (no session; the token on disk).
owner() { cli K2_PORT="$DPORT" K2_PROJECT_PATH="$WS" -- "$@"; }
# agent <sock> <port> <k2 args>: the agent's session, its passport, the
# given cell socket path and TCP port.
agent() {
    local sock="$1" port="$2"; shift 2
    cli K2_PORT="$port" K2_HOOK_TOKEN="$PASSPORT" K2_HOOK_SOCK="$sock" K2_PROJECT_PATH="$WS" -- "$@"
}

echo "== workspace + agent passport =="
WS="$WORK/acme"
mkdir -p "$WS"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git -c init.defaultBranch=main -C "$WS" init -q || die "git init"
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$WS")" \
    "$BASE/cli/projects/add-from-path?token=$DTOKEN")"
case "$reg" in *'"error"'*) die "register: $reg" ;; *"$WS"*) ;; *) die "register: $reg" ;; esac
resp="$(curl -s -X POST "$BASE/cli/workspace/ensure-pinned-chat?token=$DTOKEN" -H 'Content-Type: application/json' \
    --data-raw "$(python3 -c 'import json,sys; print(json.dumps({"project": sys.argv[1], "harness": "claude"}))' "$WS")" || true)"
case "$resp" in *'"sessionId"'*) ;; *) die "pinned chat: $resp" ;; esac
for _ in $(seq 1 1200); do grep -q '^K2_HOOK_SOCK=.' "$SB/claude.env" 2>/dev/null && break; sleep 0.05; done
[ -s "$SB/claude.env" ] || die "the pinned chat's harness never started"
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
SOCK="$(sed -n 's/^K2_HOOK_SOCK=//p' "$SB/claude.env" | head -1)"
for _ in $(seq 1 1200); do [ -S "$SOCK" ] && break; sleep 0.05; done
[ -S "$SOCK" ] || die "no cell socket at '$SOCK'"
case "$PASSPORT" in *.*) ;; *) die "the session's passport has no session part" ;; esac
[ "$PASSPORT" != "$DTOKEN" ] || die "the passport is the owner token"
ok "the agent's passport and cell socket"

# A cell socket path whose connect fails (curl 7): bound, then closed.
DEAD="$WORK/dead.sock"
python3 -I -c 'import socket, sys; s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.close()' "$DEAD"
[ -S "$DEAD" ] || die "no dead socket at $DEAD"
prc=0
curl -s -o /dev/null --unix-socket "$DEAD" --max-time 2 "http://localhost/cli/whoami" || prc=$?
[ "$prc" = 7 ] || die "the dead socket's connect: curl $prc, want 7"
GONE="$WORK/gone.sock"
[ ! -e "$GONE" ] || die "$GONE exists"
# A loopback port nothing listens on.
CLOSED="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
ok "a dead socket (curl 7), a missing path, and closed port $CLOSED"

echo "== compute: a node granted to the workspace, agents allowed =="
owner compute preview on
[ "$RC" = 0 ] || { show; die "compute preview on"; }
owner compute node add mini --url "$BASE" --json
[ "$RC" = 0 ] || { show; die "compute node add"; }
(umask 077; jget "$OUT" 'j["code"]' >"$WORK/enroll-code")
enrolled="$("$NODE_BIN" enroll --controller "$BASE" --enroll - --name mini --home "$NODE_HOME" --config-dir "$NODE_CFG" <"$WORK/enroll-code")" \
    || die "k2-node enroll: $enrolled"
SAS="$(printf '%s\n' "$enrolled" | sed -n 's/.*Code: \([0-9]\{3\} [0-9]\{3\}\).*/\1/p')"
[ -n "$SAS" ] || die "no code in: $enrolled"
owner compute node confirm mini "$SAS"
[ "$RC" = 0 ] || { show; die "compute node confirm"; }
"$NODE_BIN" run --dev --home "$NODE_HOME" --config-dir "$NODE_CFG" >"$WORK/node.log" 2>&1 &
NODE_PID=$!
online=0
for _ in $(seq 1 150); do
    owner compute nodes
    case "$OUT" in *online/loopback*) online=1; break ;; esac
    sleep 0.2
done
[ "$online" = 1 ] || die "the node never came online: $OUT"
owner compute grant mini --workspace "$WS" --max-parallel 2
[ "$RC" = 0 ] || { show; die "compute grant"; }
sw="$(curl -s -X POST -H 'Content-Type: application/json' \
    --data-raw "$(python3 -c 'import json,sys; print(json.dumps({"workspace": sys.argv[1], "value": True}))' "$WS")" \
    "$BASE/cli/compute/agents/set?token=$DTOKEN")"
case "$sw" in *'"error"'*) die "agents switch: $sw" ;; esac
ok "mini online, granted to the workspace, agents allowed"

echo "== 1. k2 send-feedback settings, the session's cell socket unreachable =="
owner send-feedback settings --json
[ "$RC" = 0 ] || { show; die "the owner reads the settings"; }
before="$(jget "$OUT" 'j["agents"]')"
case "$before" in True) flip=off ;; False) flip=cli ;; *) die "agents is not a bool: $OUT" ;; esac
for sock in "$DEAD" "$GONE"; do
    label="socket $( [ "$sock" = "$DEAD" ] && echo "connect fails" || echo "path missing")"
    agent "$sock" "$DPORT" send-feedback settings --agents "$flip"
    if [ "$RC" != 0 ]; then ok "$label: refused (exit $RC: $(printf '%s' "$ERR" | head -1))"; else bad "$label: exit 0, the change went through"; show; fi
    owner send-feedback settings --json
    [ "$RC" = 0 ] || { show; die "the owner reads the settings again"; }
    after="$(jget "$OUT" 'j["agents"]')"
    if [ "$after" = "$before" ]; then ok "$label: the setting is unchanged (agents $before)"; else bad "$label: agents went $before -> $after: written as the owner"; fi
done

echo "== 2. compute from the session: the cell socket path works =="
# TCP points at a closed port: an answer can only come over the socket.
agent "$SOCK" "$CLOSED" compute nodes
if [ "$RC" = 0 ]; then ok "compute nodes (read): exit 0 over the socket"; else bad "compute nodes (read): exit $RC"; show; fi
case "$OUT" in *mini*) ok "compute nodes (read): lists mini" ;; *) bad "compute nodes (read): no mini"; show ;; esac
agent "$SOCK" "$CLOSED" compute run --no-source --detach -- true
if [ "$RC" = 0 ]; then ok "compute run --detach (write): exit 0 over the socket"; else bad "compute run --detach (write): exit $RC"; show; fi
JOB="$(printf '%s' "$OUT" | tail -1 | tr -d '[:space:]')"
if [ -n "$JOB" ]; then
    jg="$(curl -s -G "$BASE/cli/compute/jobs/get" --data-urlencode "job=$JOB" --data-urlencode "token=$DTOKEN")"
    by="$(jget "$jg" 'j.get("submittedBy")' 2>/dev/null || echo "?")"
    via="$(jget "$jg" 'j.get("via")' 2>/dev/null || echo "?")"
    case "$by" in agent:*) ok "the job was submitted by the agent ($by)" ;; *) bad "the job's submitter is '$by': $jg" ;; esac
    if [ "$via" = None ]; then ok "the job did not come in on the owner token"; else bad "the job's via is '$via'"; fi
else
    bad "compute run --detach printed no job id"; show
fi

echo "== 3. a TCP-only Open verb from the session (today's behaviour; a later change to the TCP-only senders ends it) =="
agent "$SOCK" "$DPORT" feedback list
if [ "$CLI_UNDER_TEST" != "$CLI_BASH_SOURCE" ]; then
    # The k5 binary answers feedback list itself and never holds the owner
    # token beside a passport (scripts/cli-parity.sh's named difference
    # owner-token-with-passport, PRD 13.9 #4): it sends the session's
    # passport over TCP, which the daemon refuses there, so the agent gets
    # the refusal rather than the owner's answer.
    if [ "$RC" = 1 ]; then ok "feedback list (the k5 binary): exit 1"; else bad "feedback list (the k5 binary): exit $RC"; show; fi
    case "$ERR" in
        '{"error": {"code": "error", "hint": "Invalid or missing auth token"}}') ok "feedback list (the k5 binary): the daemon's refusal of the passport" ;;
        *) bad "feedback list (the k5 binary): stderr $(printf %q "$ERR")" ;;
    esac
else
    if [ "$RC" = 0 ]; then ok "feedback list: answers as before (exit 0)"; else bad "feedback list: exit $RC"; show; fi
    case "$ERR" in "") ok "feedback list: nothing on stderr" ;; *) bad "feedback list: stderr $(printf %q "$ERR")" ;; esac
fi

echo
echo "socket_first_no_owner_headless: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
