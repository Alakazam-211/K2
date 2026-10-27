# Shared helpers for the k2-ptyd real-supervisor survival runs
# (PRD prd-k2-ptyd-v1 §12 items 8 and 10). Sourced, not run.
#
# Everything lives under a scratch HOME ($SCRATCH): the daemon, its ptyd,
# its port/token files, the fake agent. Nothing reads or writes the real
# ~/.k2, the keychain, or a running K2 daemon.

set -euo pipefail

die() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

# need <bin>… — the built binaries (cargo build -p k2-daemon --bin k2-daemon
# && cargo build -p k2-ptyd --bins), from the target dir in $TARGET.
need_bins() {
    : "${TARGET:?set TARGET to the cargo target profile dir, e.g. \$PWD/target/debug}"
    DAEMON="$TARGET/k2-daemon"; PTYD="$TARGET/k2-ptyd"; AGENT="$TARGET/k2-ptyd-test-agent"
    for b in "$DAEMON" "$PTYD" "$AGENT"; do [ -x "$b" ] || die "missing $b (build it first)"; done
}

scratch_home() {
    SCRATCH="$(mktemp -d /tmp/k2sup-XXXXXX)"
    mkdir -p "$SCRATCH/.k2" "$SCRATCH/ws" "$SCRATCH/agent" "$SCRATCH/tmp" "$SCRATCH/xdg"
    chmod 700 "$SCRATCH/xdg"
    echo "scratch HOME: $SCRATCH"
}

alive() { kill -0 "$1" 2>/dev/null; }

wait_ready() {
    local i port
    for i in $(seq 1 120); do
        port=$( { tr -d "[:space:]" < "$SCRATCH/.k2/daemon.port"; } 2>/dev/null || true)
        if [ -n "$port" ] && curl -s --max-time 2 "http://127.0.0.1:$port/boot-status" | grep -q '"ready"'; then
            PORT=$port; TOKEN=$(tr -d '[:space:]' < "$SCRATCH/.k2/daemon.token"); return 0
        fi
        sleep 0.5
    done
    die "daemon not ready after 60 s (log: $SCRATCH/daemon.log)"
}

daemon_pid() { tr -dc '0-9' < "$SCRATCH/.k2/daemon.lock"; }
ptyd_pid() { tr -dc '0-9' < "$SCRATCH/.k2/run/ptyd/lock"; }

spawn_agent() {
    local body resp
    rm -f "$SCRATCH/agent/"*  # a previous case's agent records
    body=$(printf '{"agent_name":"tab-survive","cwd":"%s","command":"k2-ptyd-test-agent","args":["survive","%s","50"],"cols":100,"rows":30}' "$SCRATCH/ws" "$SCRATCH/agent")
    resp=$(curl -s -X POST "http://127.0.0.1:$PORT/cli/sessions/v2/spawn?token=$TOKEN" -H 'Content-Type: application/json' --data "$body")
    SID=$(printf '%s' "$resp" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
    [ -n "$SID" ] || die "spawn: $resp"
    for _ in $(seq 1 60); do [ -s "$SCRATCH/agent/pid" ] && break; sleep 0.25; done
    AGENT_PID=$(cat "$SCRATCH/agent/pid")
    alive "$AGENT_PID" || die "agent not running"
    ok "agent pid $AGENT_PID session $SID under ptyd $(ptyd_pid)"
}

# after_restart <what>: the same agent, adopted, unsolicited-input free.
after_restart() {
    local what="$1" before
    wait_ready
    alive "$AGENT_PID" || die "$what: the agent died"
    [ ! -e "$SCRATCH/agent/sighup" ] || die "$what: the agent got a SIGHUP"
    [ ! -s "$SCRATCH/agent/stdin" ] || die "$what: unsolicited bytes reached the agent"
    curl -s "http://127.0.0.1:$PORT/cli/terminal/read?token=$TOKEN&id=$SID&lines=50" | grep -q 'seq=' \
        || die "$what: the session is not registered again"
    grep -q "adopted session=$SID" "$SCRATCH/daemon.log" || die "$what: no adoption line in the log"
    before=$(wc -l < "$SCRATCH/agent/hook.log")
    sleep 3
    tail -n +"$((before + 1))" "$SCRATCH/agent/hook.log" | grep -qE ' (200|400)$' || die "$what: the passport is not served after the restart"
    ok "$what: agent $AGENT_PID survived and was adopted"
}

# after_stop <what>: the agent and ptyd are gone.
after_stop() {
    local what="$1" p
    p=$(ptyd_pid || true)
    for _ in $(seq 1 40); do alive "$AGENT_PID" || break; sleep 0.25; done
    alive "$AGENT_PID" && die "$what: the agent outlived a stop"
    if [ -n "$p" ]; then
        for _ in $(seq 1 40); do alive "$p" || break; sleep 0.25; done
        alive "$p" && die "$what: ptyd outlived a stop"
    fi
    ok "$what: agent and ptyd ended"
}

# ── update-swap runs (update-launchd.sh, update-systemd-user.sh) ─────────
# OLD and NEW: dirs holding a k2-daemon and the k2-ptyd built with it (e.g.
# v0.46.4 and the tip); AGENT: the k2-ptyd-test-agent binary.
need_update_bins() {
    : "${OLD:?set OLD to the dir of the old k2-daemon + k2-ptyd}" "${NEW:?set NEW to the dir of the new ones}" "${AGENT:?set AGENT to k2-ptyd-test-agent}"
    local b
    for b in "$OLD/k2-daemon" "$OLD/k2-ptyd" "$NEW/k2-daemon" "$NEW/k2-ptyd" "$AGENT"; do [ -x "$b" ] || die "missing $b"; done
}

# swap_bins <src dir> <dst dir>: the self-updater's in-place swap — stage
# beside the running binary, then rename over it (the running image keeps
# its inode). k2-ptyd ships beside the daemon.
swap_bins() {
    local src="$1" dst="$2" b
    mkdir -p "$dst"
    for b in k2-daemon k2-ptyd; do
        cp "$src/$b" "$dst/.$b.new" && chmod 755 "$dst/.$b.new" && mv -f "$dst/.$b.new" "$dst/$b" || die "swap $b"
    done
}

# S5: no call the agent made failed (refused connect, dropped socket, 503),
# counted from its first served cell call on (a call before the daemon bound
# the cell socket at spawn is a spawn race the CLI's retry covers).
assert_no_failed_calls() {
    local f bad since
    since=$(grep -E ' (200|400)$' "$SCRATCH/agent/hook.log" | head -1 | cut -d' ' -f1)
    [ -n "$since" ] || die "$1: the agent never got a served cell call"
    for f in hook.log tcp.log; do
        [ -f "$SCRATCH/agent/$f" ] || continue
        bad=$(awk -v s="$since" '$1 >= s && ($2 ~ /^ERR:/ || $2 == "503")' "$SCRATCH/agent/$f")
        [ -z "$bad" ] || die "$1: the agent's calls failed during the window ($f): $(printf '%s' "$bad" | head -3)"
    done
}

adoptions_of() { grep -c "adopted session=$1 " "$SCRATCH/daemon.log" || true; }

# after_update <what> <adoptions before> <screen: restored|replayed>: the
# same agent, adopted once more by the new daemon, its screen restored or
# replayed as expected, ptyd kept, and no call failed.
after_update() {
    local what="$1" before="$2" want="$3" now last
    after_restart "$what"
    now=$(adoptions_of "$SID")
    [ "$now" -gt "$before" ] || die "$what: the new daemon did not adopt session $SID"
    last=$(grep "adopted session=$SID " "$SCRATCH/daemon.log" | grep 'screen=' | tail -1)
    case "$last" in *"screen=$want"*) ;; *) die "$what: expected screen=$want, got: $last" ;; esac
    [ "$(ptyd_pid)" = "$PTYD_PID" ] || die "$what: ptyd changed ($PTYD_PID → $(ptyd_pid)); expected it kept (same protocol)"
    alive "$PTYD_PID" || die "$what: ptyd died"
    assert_no_failed_calls "$what"
    ok "$what: agent $AGENT_PID adopted (screen $want), ptyd $PTYD_PID kept, no failed calls"
}

enable_survive_setting() {
    curl -s -X POST "http://127.0.0.1:$PORT/cli/settings/update?token=$TOKEN" -H 'Content-Type: application/json' \
        --data '{"terminal":{"ptyd":"on"}}' | grep -q '"ptyd":"on"' || die "could not turn terminal.ptyd on"
}

write_intent() {
    mkdir -p "$SCRATCH/.k2/run"
    ( umask 077; printf '{"by":"%s","at":%s,"nonce":"t-%s"}' "$1" "$(date +%s)" "$$" > "$SCRATCH/.k2/run/stop-intent.json" )
}
