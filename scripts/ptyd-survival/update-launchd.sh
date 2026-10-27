#!/bin/bash
# k2-ptyd: a REAL update (bundle replace + launchd restart) keeps agents
# (PRD prd-k2-ptyd-v1 P59; T2). macOS, a logged-in GUI user, no root. Its
# own job label `dev.k2.daemon.ptydupd` and a scratch HOME: the user's real
# K2 (dev.k2.daemon, ~/.k2, the keychain) is never touched.
#
#   OLD=<dir: v0.46.4 k2-daemon + k2-ptyd> NEW=<dir: tip ones> \
#   AGENT=<k2-ptyd-test-agent> scripts/ptyd-survival/update-launchd.sh
#
# The desktop updater replaces the whole K2.app by rename and the version
# check then runs `launchctl kickstart -k`; the running daemon keeps its old
# image until then. This does the same with a scratch bundle holding the
# daemon and k2-ptyd side by side:
#   0. the OLD daemon, terminal.ptyd turned on (the 0.46.4 Beta toggle), one
#      restart so it boots with it on, then an agent;
#   1. bundle replace OLD → NEW, kickstart -k: same agent, adopted by the new
#      daemon (screen replayed: the old daemon saves no screen), ptyd kept,
#      no failed call;
#   2. bundle replace NEW → NEW, kickstart -k: screen restored (S4);
#   3. kill -9 (KeepAlive respawn): screen restored from the last save;
#   4. POST /cli/daemon/stop: the agent and ptyd end.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=common.sh
. "$here/common.sh"
[ "$(uname -s)" = Darwin ] || die "macOS only"
need_update_bins
LABEL=dev.k2.daemon.ptydupd
DOMAIN="gui/$(id -u)"
launchctl print "$DOMAIN" >/dev/null 2>&1 || die "no $DOMAIN domain: log in to the Mac's GUI as this user"
launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1 && die "$LABEL is already loaded"

scratch_home
APPS="$SCRATCH/Applications"
MACOS="$APPS/K2.app/Contents/MacOS"
DAEMON="$MACOS/k2-daemon"
swap_bins "$OLD" "$MACOS"

# The updater's bundle replace: a complete new bundle beside, then renames.
replace_bundle() {
    local src="$1"
    rm -rf "$APPS/K2.app.new" "$APPS/K2.app.old"
    swap_bins "$src" "$APPS/K2.app.new/Contents/MacOS"
    mv "$APPS/K2.app" "$APPS/K2.app.old"
    mv "$APPS/K2.app.new" "$APPS/K2.app"
    rm -rf "$APPS/K2.app.old"
}

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
    <key>K2_TEST_AGENT_SHIM_DIR</key><string>$(dirname "$AGENT")</string>
    <key>K2_TUNNEL_LEASE</key><string>off</string>
    <key>K2_CONNECT_BASE</key><string>http://127.0.0.1:1</string>
    <key>K2_CERT_BROKER_URL</key><string>http://127.0.0.1:1</string>
    <key>K2_LLM_ACCOUNTS_LOOP</key><string>0</string>
    <key>K2SO_WATCHDOG_DISABLED</key><string>1</string>
    <key>K2_HEARTBEAT_NO_SELF_HEAL</key><string>1</string>
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

kickstart() {
    local old; old=$(daemon_pid)
    launchctl kickstart -k "$DOMAIN/$LABEL"
    for _ in $(seq 1 60); do [ "$(daemon_pid 2>/dev/null)" != "$old" ] && break; sleep 0.25; done
}

# 0. The old daemon with the setting on (and booted with it), then an agent.
launchctl enable "$DOMAIN/$LABEL"; launchctl bootstrap "$DOMAIN" "$PLIST"; wait_ready
enable_survive_setting
kickstart; wait_ready
spawn_agent
PTYD_PID=$(ptyd_pid)
old_version=$(curl -s "http://127.0.0.1:$PORT/boot-status" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')

# 1. The update itself: OLD → NEW bundle, then the version-check restart.
n=$(adoptions_of "$SID"); replace_bundle "$NEW"; kickstart
after_update "update $old_version → new (bundle replace + kickstart -k)" "$n" replayed

# 2. The next update (NEW → NEW): the new daemon saved the screen.
n=$(adoptions_of "$SID"); replace_bundle "$NEW"; kickstart
after_update "update new → new (bundle replace + kickstart -k)" "$n" restored

# 3. A crash right after: the last saved screen is still good.
n=$(adoptions_of "$SID"); old=$(daemon_pid); kill -9 "$old"
for _ in $(seq 1 60); do [ "$(daemon_pid 2>/dev/null)" != "$old" ] && break; sleep 0.25; done
after_update "kill -9 (KeepAlive respawn)" "$n" restored

# 4. A stop ends it.
curl -s -X POST "http://127.0.0.1:$PORT/cli/daemon/stop?token=$TOKEN" | grep -q '"stopping":true' || die "stop route"
after_stop "POST /cli/daemon/stop"
ok "launchd update: all cases passed"
