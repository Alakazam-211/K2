#!/bin/bash
# k2-ptyd survival under REAL launchd (PRD prd-k2-ptyd-v1 §12 item 10).
#
# Needs: a Mac with a logged-in GUI user (a `gui/<uid>` launchd domain; run
# from Terminal/iTerm or an SSH session of that same logged-in user). No
# root. Uses its own job label `dev.k2.daemon.ptydtest` and a scratch HOME,
# so the user's real K2 (dev.k2.daemon, ~/.k2) is never touched.
#
#   TARGET=$PWD/target/debug scripts/ptyd-survival/launchd.sh
#
# Cases: kickstart -k, bootout+bootstrap, kill -9 (KeepAlive respawn) all
# keep the agent (L3, L5); the menu-bar Quit (stop intent + unload -w) and
# POST /cli/daemon/stop end it (L2).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=common.sh
. "$here/common.sh"
[ "$(uname -s)" = Darwin ] || die "macOS only"
need_bins
LABEL=dev.k2.daemon.ptydtest
UID_=$(id -u)
DOMAIN="gui/$UID_"
launchctl print "$DOMAIN" >/dev/null 2>&1 || die "no $DOMAIN domain: log in to the Mac's GUI as this user"

scratch_home
PLIST="$SCRATCH/Library/LaunchAgents/$LABEL.plist"
mkdir -p "$(dirname "$PLIST")"
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>$DAEMON</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>HOME</key><string>$SCRATCH</string>
    <key>TMPDIR</key><string>$SCRATCH/tmp/</string>
    <key>K2_PTYD</key><string>1</string>
    <key>K2_PTYD_BIN</key><string>$PTYD</string>
    <key>K2_TEST_AGENT_SHIM_DIR</key><string>$(dirname "$AGENT")</string>
    <key>K2_TUNNEL_LEASE</key><string>off</string>
    <key>K2_CONNECT_BASE</key><string>http://127.0.0.1:1</string>
    <key>K2_CERT_BROKER_URL</key><string>http://127.0.0.1:1</string>
    <key>K2_LLM_ACCOUNTS_LOOP</key><string>0</string>
    <key>K2SO_WATCHDOG_DISABLED</key><string>1</string>
  </dict>
  <key>KeepAlive</key><true/>
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>$SCRATCH/daemon.log</string>
  <key>StandardErrorPath</key><string>$SCRATCH/daemon.log</string>
</dict></plist>
EOF

cleanup() {
    launchctl bootout "$DOMAIN/$LABEL" >/dev/null 2>&1 || true
    launchctl enable "$DOMAIN/$LABEL" >/dev/null 2>&1 || true
    [ -n "${AGENT_PID:-}" ] && kill -9 "$AGENT_PID" 2>/dev/null || true
    p=$(ptyd_pid 2>/dev/null || true); [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true
    echo "logs kept in $SCRATCH"
}
trap cleanup EXIT

# `unload -w` leaves the label disabled; enable it before each bootstrap.
bootstrap() { launchctl enable "$DOMAIN/$LABEL"; launchctl bootstrap "$DOMAIN" "$PLIST"; wait_ready; }

bootstrap
spawn_agent

old=$(daemon_pid); launchctl kickstart -k "$DOMAIN/$LABEL"
for _ in $(seq 1 40); do [ "$(daemon_pid)" != "$old" ] && break; sleep 0.25; done
after_restart "kickstart -k"

launchctl bootout "$DOMAIN/$LABEL"; sleep 1; bootstrap
after_restart "bootout + bootstrap"

old=$(daemon_pid); kill -9 "$old"
for _ in $(seq 1 60); do [ "$(daemon_pid 2>/dev/null)" != "$old" ] && break; sleep 0.25; done
after_restart "kill -9 (KeepAlive respawn)"

# Menu-bar Quit: stop intent, then unload -w (what k2-menubar does).
write_intent menubar
launchctl unload -w "$PLIST"
after_stop "menu-bar Quit"

# POST /cli/daemon/stop: the daemon ends its agents and unloads its own job.
bootstrap
spawn_agent
curl -s -X POST "http://127.0.0.1:$PORT/cli/daemon/stop?token=$TOKEN" | grep -q '"stopping":true' || die "stop route"
after_stop "POST /cli/daemon/stop"
sleep 3
launchctl print "$DOMAIN/$LABEL" 2>/dev/null | grep -q 'pid = ' && die "launchd respawned the daemon after a stop"
ok "launchd: all cases passed"
