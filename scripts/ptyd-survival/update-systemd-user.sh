#!/bin/bash
# k2-ptyd: a REAL update (in-place binary swap + systemd restart or the
# hand-off reload) keeps agents (PRD prd-k2-ptyd-v1 P60; T2). Linux, a
# running systemd user manager for this user, no root. Its own unit
# `k2-ptydupd.service` and a scratch HOME: a real K2 unit and ~/.k2 are
# never touched. Not inside the K2 compute job wall (no user manager).
#
#   OLD=<dir: v0.46.4 k2-daemon + k2-ptyd> NEW=<dir: tip ones> \
#   AGENT=<k2-ptyd-test-agent> scripts/ptyd-survival/update-systemd-user.sh
#
# The unit carries the shipped hand-off lines (runtime stage H drop-in:
# KillMode=process, NotifyAccess=main, ExecReload=SIGUSR2, OOMPolicy=
# continue), so `systemctl --user reload` is the no-downtime hand-off the
# Linux self-update uses after its in-place swap.
#   0. the OLD daemon, terminal.ptyd on (the 0.46.4 Beta toggle), one
#      restart so it boots with it on, then an agent;
#   1. swap OLD → NEW in place, `systemctl --user reload` (hand-off from the
#      old daemon to the new binary): same agent, adopted, screen replayed
#      (the old daemon saves none), ptyd kept, no failed call;
#   2. swap NEW → NEW, `systemctl --user restart`: screen restored (S4);
#   3. `systemctl --user reload` again (new → new hand-off): restored;
#   4. kill -9 (Restart=always): restored from the last save;
#   5. `systemctl --user stop`: the agent and ptyd end.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=common.sh
. "$here/common.sh"
[ "$(uname -s)" = Linux ] || die "Linux only"
need_update_bins
systemctl --user show-environment >/dev/null 2>&1 || die "no systemd user manager for $(id -un)"
UNIT=k2-ptydupd.service
UNIT_FILE="$HOME/.config/systemd/user/$UNIT"
[ -e "$UNIT_FILE" ] && die "$UNIT_FILE exists; remove it first"

scratch_home
BIN="$SCRATCH/bin"
swap_bins "$OLD" "$BIN"
mkdir -p "$(dirname "$UNIT_FILE")"
cat > "$UNIT_FILE" <<EOF
[Unit]
Description=K2 ptyd update-swap test daemon (scratch HOME $SCRATCH)

[Service]
Type=simple
ExecStart=$BIN/k2-daemon
Restart=always
RestartSec=1
KillMode=process
NotifyAccess=main
ExecReload=/bin/kill -USR2 \$MAINPID
OOMPolicy=continue
Environment=HOME=$SCRATCH
Environment=TMPDIR=$SCRATCH/tmp
Environment=K2_TEST_AGENT_SHIM_DIR=$(dirname "$AGENT")
Environment=K2_TUNNEL_LEASE=off
Environment=K2_CONNECT_BASE=http://127.0.0.1:1
Environment=K2_CERT_BROKER_URL=http://127.0.0.1:1
Environment=K2_HANDOFF_DROPIN=off
Environment=K2_LLM_ACCOUNTS_LOOP=0
Environment=K2SO_WATCHDOG_DISABLED=1
Environment=K2_HEARTBEAT_NO_SELF_HEAL=1
StandardOutput=append:$SCRATCH/daemon.log
StandardError=append:$SCRATCH/daemon.log
EOF
systemctl --user daemon-reload

cleanup() {
    systemctl --user stop "$UNIT" >/dev/null 2>&1 || true
    rm -f "$UNIT_FILE"; systemctl --user daemon-reload || true
    [ -n "${AGENT_PID:-}" ] && kill -9 "$AGENT_PID" 2>/dev/null || true
    p=$(ptyd_pid 2>/dev/null || true); [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true
    echo "logs kept in $SCRATCH"
}
trap cleanup EXIT

main_pid() { systemctl --user show -p MainPID --value "$UNIT"; }
# wait_new_main <old pid>: the unit's main process changed (restart or hand-off).
wait_new_main() {
    for _ in $(seq 1 120); do
        local m; m=$(main_pid)
        [ -n "$m" ] && [ "$m" != 0 ] && [ "$m" != "$1" ] && return 0
        sleep 0.25
    done
    die "the unit's main process never changed from $1"
}

# 0. The old daemon with the setting on (and booted with it), then an agent.
systemctl --user start "$UNIT"; wait_ready
enable_survive_setting
m=$(main_pid); systemctl --user restart "$UNIT"; wait_new_main "$m"; wait_ready
spawn_agent
PTYD_PID=$(ptyd_pid)
old_version=$(curl -s "http://127.0.0.1:$PORT/boot-status" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')

# 1. The update: in-place swap, then the hand-off reload (old → new binary).
n=$(adoptions_of "$SID"); m=$(main_pid)
swap_bins "$NEW" "$BIN"
systemctl --user reload "$UNIT"; wait_new_main "$m"
after_update "update $old_version → new (swap + systemctl --user reload hand-off)" "$n" replayed

# 2. The next update with a plain restart.
n=$(adoptions_of "$SID"); m=$(main_pid)
swap_bins "$NEW" "$BIN"
systemctl --user restart "$UNIT"; wait_new_main "$m"
after_update "update new → new (swap + systemctl --user restart)" "$n" restored

# 3. The hand-off once more (new → new).
n=$(adoptions_of "$SID"); m=$(main_pid)
systemctl --user reload "$UNIT"; wait_new_main "$m"
after_update "systemctl --user reload (new → new hand-off)" "$n" restored

# 4. A crash.
n=$(adoptions_of "$SID"); m=$(main_pid); kill -9 "$m"; wait_new_main "$m"
after_update "kill -9 (Restart=always)" "$n" restored

# 5. A stop ends it.
systemctl --user stop "$UNIT"
after_stop "systemctl --user stop"
ok "systemd user update: all cases passed"
