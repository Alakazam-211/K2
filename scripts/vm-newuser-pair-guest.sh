#!/bin/bash
# Guest-side new-user install + daemon pairing smoke.
#
# Catches the 0.40.33/0.40.34 class of bug: fresh install creates ~/.k2 but
# omits the ~/.k2so → ~/.k2 compat symlink, so the thin client (hardcoded
# ~/.k2so/daemon.port) never pairs with the daemon.
#
# Invoked by scripts/vm-newuser-pair-smoke.sh over SSH. Env:
#   K2_LABEL     — version label for reports (e.g. 0.40.54)
#   K2_DMG_NAME  — basename of the DMG under ~/k2-gk-testdata/
#
# Exit 0 = PASS, non-zero = FAIL (pairing or install).

set +e
LABEL="${K2_LABEL:?K2_LABEL required}"
DMG_NAME="${K2_DMG_NAME:?K2_DMG_NAME required}"
DIR="${K2_TEST_DIR:-$HOME/k2-gk-testdata}"
DMG="$DIR/$DMG_NAME"
REPORT="$HOME/k2-newuser-${LABEL}.txt"
: > "$REPORT"
log() { echo "$@" | tee -a "$REPORT"; }

log "=== NEW-USER PAIRING smoke: $LABEL ==="
log "utc: $(date -u)"
log "os: $(sw_vers -productVersion) $(uname -m)"
log "home=$HOME"

# --- preconditions: truly clean home state ---
# shellcheck disable=SC2009
pgrep -x k2-daemon >/dev/null 2>&1 && kill "$(pgrep -x k2-daemon)" 2>/dev/null
pgrep -x k2so-daemon >/dev/null 2>&1 && kill "$(pgrep -x k2so-daemon)" 2>/dev/null
launchctl bootout "gui/$(id -u)/dev.k2.daemon" 2>/dev/null
launchctl bootout "gui/$(id -u)/com.k2so.daemon" 2>/dev/null
rm -rf "$HOME/.k2" "$HOME/.k2so" 2>/dev/null
sleep 1

if [ -e "$HOME/.k2" ] || [ -e "$HOME/.k2so" ]; then
  log "WARN: could not fully clear home state"
  ls -la "$HOME/.k2" "$HOME/.k2so" 2>&1 | tee -a "$REPORT"
fi
log "precheck: home state cleared (or best-effort)"

if [ -d /Applications/K2.app ]; then
  if sudo -n true 2>/dev/null; then
    sudo rm -rf /Applications/K2.app
  else
    rm -rf /Applications/K2.app 2>/dev/null
  fi
fi

if [ ! -f "$DMG" ]; then
  log "VERDICT_${LABEL}=FAIL_MISSING_DMG"
  log "missing: $DMG"
  exit 1
fi
log "dmg_sha=$(shasum -a 256 "$DMG" | awk '{print $1}')"

# --- install from DMG ---
hdiutil detach /Volumes/K2 -force >/dev/null 2>&1
hdiutil attach -nobrowse -readonly "$DMG" >/tmp/k2-pair-attach.txt 2>&1
log "attach_rc=$?"
if [ ! -d /Volumes/K2/K2.app ]; then
  log "VERDICT_${LABEL}=FAIL_MOUNT"
  cat /tmp/k2-pair-attach.txt | tee -a "$REPORT"
  exit 1
fi
if ! ditto /Volumes/K2/K2.app /Applications/K2.app 2>/tmp/k2-pair-ditto.err; then
  if sudo -n true 2>/dev/null; then
    sudo ditto /Volumes/K2/K2.app /Applications/K2.app 2>>/tmp/k2-pair-ditto.err
  fi
fi
hdiutil detach /Volumes/K2 -force >/dev/null 2>&1
if [ ! -d /Applications/K2.app ]; then
  log "VERDICT_${LABEL}=FAIL_INSTALL"
  cat /tmp/k2-pair-ditto.err 2>/dev/null | tee -a "$REPORT"
  exit 1
fi
log "installed=/Applications/K2.app"
/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
  /Applications/K2.app/Contents/Info.plist 2>/dev/null | tee -a "$REPORT"

DAEMON=""
for c in \
  /Applications/K2.app/Contents/MacOS/k2-daemon \
  /Applications/K2.app/Contents/MacOS/k2so-daemon
do
  if [ -x "$c" ]; then DAEMON="$c"; break; fi
done
log "daemon_bin=$DAEMON"
if [ -z "$DAEMON" ]; then
  log "VERDICT_${LABEL}=FAIL_NO_DAEMON_BIN"
  exit 1
fi

# --- start bundled daemon (migrate_home runs at boot; no GUI app) ---
"$DAEMON" >/tmp/k2-daemon-stdout.log 2>/tmp/k2-daemon-stderr.log &
DPID=$!
log "daemon_pid=$DPID"
sleep 4

PORT_FILE=""
for _ in $(seq 1 20); do
  for f in \
    "$HOME/.k2/daemon.port" \
    "$HOME/.k2/heartbeat.port" \
    "$HOME/.k2so/daemon.port" \
    "$HOME/.k2so/heartbeat.port"
  do
    if [ -f "$f" ] && [ -s "$f" ]; then
      PORT_FILE="$f"
      break 2
    fi
  done
  sleep 1
done
log "port_file_found=$PORT_FILE"
log "home_layout:"
ls -la "$HOME/.k2" 2>&1 | head -25 | tee -a "$REPORT"
ls -la "$HOME/.k2so" 2>&1 | head -10 | tee -a "$REPORT"

FAIL=0

if [ ! -d "$HOME/.k2" ]; then
  log "ASSERT_FAIL: ~/.k2 missing after daemon start"
  FAIL=1
else
  log "ASSERT_OK: ~/.k2 exists"
fi

# Compat symlink — missing on 0.40.33/34 fresh installs
if [ -L "$HOME/.k2so" ]; then
  target=$(readlink "$HOME/.k2so")
  log "ASSERT_OK: ~/.k2so is symlink -> $target"
  if [ ! -d "$HOME/.k2so" ]; then
    log "ASSERT_FAIL: ~/.k2so symlink is broken"
    FAIL=1
  fi
else
  if [ -e "$HOME/.k2so" ]; then
    log "ASSERT_FAIL: ~/.k2so exists but is NOT a symlink ($(ls -ld "$HOME/.k2so"))"
  else
    log "ASSERT_FAIL: ~/.k2so missing entirely (0.40.33/34 fresh-install regression class)"
  fi
  FAIL=1
fi

# Thin-client path: port via ~/.k2so (what the app historically resolved)
APP_PORT=""
for f in "$HOME/.k2so/daemon.port" "$HOME/.k2so/heartbeat.port"; do
  if [ -f "$f" ] && [ -s "$f" ]; then
    APP_PORT=$(tr -d ' \n\r' < "$f")
    log "ASSERT_OK: app can read $f -> $APP_PORT"
    break
  fi
done
if [ -z "$APP_PORT" ]; then
  log "ASSERT_FAIL: no daemon/heartbeat port readable via ~/.k2so/* (client pairing path)"
  for f in "$HOME/.k2/daemon.port" "$HOME/.k2/heartbeat.port"; do
    if [ -f "$f" ]; then
      log "  note: exists under canonical $f = $(tr -d ' \n\r' < "$f")"
    fi
  done
  FAIL=1
fi

if [ -n "$APP_PORT" ]; then
  CODE=$(curl -sS -m 5 -o /tmp/k2-bs.json -w "%{http_code}" \
    "http://127.0.0.1:${APP_PORT}/boot-status" 2>/dev/null)
  BODY=$(head -c 400 /tmp/k2-bs.json 2>/dev/null)
  log "boot_status_http=$CODE body=$BODY"
  if [ "$CODE" = "200" ]; then
    log "ASSERT_OK: /boot-status HTTP 200 via app pairing port"
    if echo "$BODY" | grep -qiE '"phase"[[:space:]]*:[[:space:]]*"ready"|ready'; then
      log "ASSERT_OK: boot-status reports ready (or compatible)"
    fi
  else
    log "ASSERT_FAIL: /boot-status not healthy via app port (http=$CODE)"
    FAIL=1
  fi
fi

if kill -0 "$DPID" 2>/dev/null; then
  log "ASSERT_OK: daemon process still running"
else
  log "ASSERT_FAIL: daemon exited early"
  log "stderr: $(tail -40 /tmp/k2-daemon-stderr.log 2>/dev/null)"
  log "stdout: $(tail -40 /tmp/k2-daemon-stdout.log 2>/dev/null)"
  FAIL=1
fi

kill "$DPID" 2>/dev/null
sleep 1
kill -9 "$DPID" 2>/dev/null

if [ "$FAIL" -eq 0 ]; then
  log "VERDICT_${LABEL}=PASS"
  exit 0
fi
log "VERDICT_${LABEL}=FAIL_PAIRING"
exit 1
