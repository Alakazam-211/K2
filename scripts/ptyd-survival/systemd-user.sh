#!/bin/bash
# k2-ptyd survival under a REAL systemd user manager (PRD prd-k2-ptyd-v1
# §12 item 8).
#
# Needs: a Linux box where this user has a running user manager
# (`systemctl --user` works: a login session, or `loginctl enable-linger`).
# No root. Uses its own unit `k2-ptydtest.service` and a scratch HOME, so a
# real K2 daemon unit and ~/.k2 are never touched. Not inside the K2 compute
# job wall (it has no user manager).
#
#   TARGET=$PWD/target/debug scripts/ptyd-survival/systemd-user.sh
#
# The unit keeps the user manager's own XDG_RUNTIME_DIR (/run/user/<uid>):
# the daemon runs `systemctl --user list-jobs` (is this SIGTERM a stop?)
# and `systemctl --user stop` (POST /cli/daemon/stop), which need the
# user bus there. Running as root via sudo: pass XDG_RUNTIME_DIR too, e.g.
#   sudo -u k2test XDG_RUNTIME_DIR=/run/user/$(id -u k2test) TARGET=… bash systemd-user.sh
#
# Cases: systemctl --user restart, kill -9 (Restart=always) keep the agent
# (L3, L5); systemctl --user stop and POST /cli/daemon/stop end it (L2,
# P55); a KillMode=control-group unit keeps new sessions in-process
# (ptyd: off (killmode_control-group), P46).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=common.sh
. "$here/common.sh"
[ "$(uname -s)" = Linux ] || die "Linux only"
need_bins
systemctl --user show-environment >/dev/null 2>&1 || die "no systemd user manager for $(id -un)"
UNIT=k2-ptydtest.service
UNIT_FILE="$HOME/.config/systemd/user/$UNIT"
[ -e "$UNIT_FILE" ] && die "$UNIT_FILE exists; remove it first"

scratch_home
write_unit() {
    mkdir -p "$(dirname "$UNIT_FILE")"
    cat > "$UNIT_FILE" <<EOF
[Unit]
Description=K2 ptyd survival test daemon (scratch HOME $SCRATCH)

[Service]
Type=simple
ExecStart=$DAEMON
Restart=always
RestartSec=1
KillMode=$1
Environment=HOME=$SCRATCH
Environment=TMPDIR=$SCRATCH/tmp
Environment=K2_PTYD=1
Environment=K2_PTYD_BIN=$PTYD
Environment=K2_TEST_AGENT_SHIM_DIR=$(dirname "$AGENT")
Environment=K2_TUNNEL_LEASE=off
Environment=K2_CONNECT_BASE=http://127.0.0.1:1
Environment=K2_CERT_BROKER_URL=http://127.0.0.1:1
Environment=K2_HANDOFF_DROPIN=off
Environment=K2_LLM_ACCOUNTS_LOOP=0
Environment=K2SO_WATCHDOG_DISABLED=1
StandardOutput=append:$SCRATCH/daemon.log
StandardError=append:$SCRATCH/daemon.log
EOF
    systemctl --user daemon-reload
}

cleanup() {
    systemctl --user stop "$UNIT" >/dev/null 2>&1 || true
    rm -f "$UNIT_FILE"; systemctl --user daemon-reload || true
    [ -n "${AGENT_PID:-}" ] && kill -9 "$AGENT_PID" 2>/dev/null || true
    p=$(ptyd_pid 2>/dev/null || true); [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true
    echo "logs kept in $SCRATCH"
}
trap cleanup EXIT

write_unit process
systemctl --user start "$UNIT"; wait_ready
spawn_agent

systemctl --user restart "$UNIT"
after_restart "systemctl --user restart"

old=$(daemon_pid); kill -9 "$old"
for _ in $(seq 1 60); do [ "$(daemon_pid 2>/dev/null)" != "$old" ] && break; sleep 0.25; done
after_restart "kill -9 (Restart=always)"

systemctl --user stop "$UNIT"
after_stop "systemctl --user stop"

systemctl --user start "$UNIT"; wait_ready
spawn_agent
curl -s -X POST "http://127.0.0.1:$PORT/cli/daemon/stop?token=$TOKEN" | grep -q '"stopping":true' || die "stop route"
after_stop "POST /cli/daemon/stop"
sleep 3
[ "$(systemctl --user is-active "$UNIT" || true)" = active ] && die "systemd restarted the daemon after a stop"

# KillMode=control-group: a restart would kill ptyd with the cgroup, so new
# survive sessions must stay in-process and /boot-status must say why.
write_unit control-group
systemctl --user start "$UNIT"; wait_ready
curl -s -X POST "http://127.0.0.1:$PORT/cli/sessions/v2/spawn?token=$TOKEN" -H 'Content-Type: application/json' \
    --data "{\"agent_name\":\"tab-cg\",\"cwd\":\"$SCRATCH/ws\",\"command\":\"k2-ptyd-test-agent\",\"args\":[\"sleep\"]}" >/dev/null
sleep 1
curl -s "http://127.0.0.1:$PORT/boot-status?token=$TOKEN" | grep -q 'killmode_control-group' \
    || die "control-group unit: /boot-status does not report ptyd off (killmode_control-group)"
ok "systemd user: all cases passed"
