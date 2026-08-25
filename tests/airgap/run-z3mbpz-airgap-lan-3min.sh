#!/usr/bin/env bash
# Live air-gap + LAN probe for a running macOS launchd daemon (z3mbpZ).
#
# What it proves, inside the Connect lease window (3 minutes):
#   1. K2_AIRGAP=1 + K2_LISTEN=lan on THAT launchd daemon (env only — not
#      persisted settings, not tunnel disable/unpair).
#   2. Control plane is dark: no k2 frpc, no ESTABLISHED to k2e-01,
#      update/check and tunnel/start refuse with the teaching error.
#   3. LAN HTTP still answers: /boot-status on 127.0.0.1 AND the box's
#      RFC1918 address, listener bound 0.0.0.0.
#   4. Restore: strip launchd env, respawn, air-gap off, tunnel autostart
#      comes back. tunnel.json / keychain / federation files are never
#      deleted.
#
# Usage (on the daemon box, or ssh z3mbpZ bash this):
#   NEW_DAEMON=/path/to/aa5dd0e9/k2-daemon ./tests/airgap/run-z3mbpz-airgap-lan-3min.sh
#
# Does NOT: persist AppSettings.airgap, write enabled:false, unpair, or
# leave K2_AIRGAP on. Hard deadline: restore begins by T+150s.

set -euo pipefail

LEASE_SECS=180
RESTORE_BY=150
LABEL="dev.k2.daemon"
HOME_K2="${HOME}/.k2"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
UID_NUM="$(id -u)"
DOMAIN="gui/${UID_NUM}"
TARGET="${DOMAIN}/${LABEL}"
K2E01="178.156.232.105"
PORT_FILE="${HOME_K2}/daemon.port"
TOKEN_FILE="${HOME_K2}/daemon.token"
STAMP="$(date +%Y%m%dT%H%M%S)"
BACKUP_DIR="${TMPDIR:-/tmp}/k2-airgap-lan-test-${STAMP}"
LOG="${BACKUP_DIR}/test.log"
T0=0
RESTORED=0

die() { echo "FAIL: $*" >&2; exit 1; }
log() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$LOG"; }
secs_left() { echo $((RESTORE_BY - ($(date +%s) - T0))); }

port_now() {
  [[ -f "$PORT_FILE" ]] || die "missing $PORT_FILE"
  tr -d '[:space:]' <"$PORT_FILE"
}

token_now() {
  [[ -f "$TOKEN_FILE" ]] || die "missing $TOKEN_FILE"
  tr -d '[:space:]' <"$TOKEN_FILE"
}

http() {
  # http METHOD PATH [body]
  local method="$1" path="$2" body="${3:-}" port token
  port="$(port_now)"
  token="$(token_now)"
  if [[ -n "$body" ]]; then
    curl -sS -m 5 -X "$method" \
      -H "Content-Type: application/json" \
      --data-binary "$body" \
      "http://127.0.0.1:${port}${path}?token=${token}"
  else
    curl -sS -m 5 -X "$method" \
      "http://127.0.0.1:${port}${path}?token=${token}"
  fi
}

http_code() {
  local url="$1"
  curl -sS -m 5 -o /dev/null -w "%{http_code}" "$url" || true
}

wait_boot() {
  local want_airgap="$1" deadline=$(( $(date +%s) + 25 )) code body
  while (( $(date +%s) < deadline )); do
    if body="$(curl -sS -m 2 "http://127.0.0.1:$(port_now)/boot-status" 2>/dev/null)"; then
      if echo "$body" | python3 -c 'import json,sys
d=json.load(sys.stdin)
assert d.get("phase")=="ready"
ag=bool((d.get("airgap") or {}).get("enabled"))
want=sys.argv[1]=="1"
sys.exit(0 if ag==want else 1)
' "$want_airgap" 2>/dev/null; then
        echo "$body"
        return 0
      fi
    fi
    sleep 0.4
  done
  die "daemon did not reach phase=ready airgap=${want_airgap} (last=$(curl -sS -m 2 http://127.0.0.1:$(port_now)/boot-status 2>/dev/null || echo none))"
}

lan_ipv4() {
  ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null || true
}

k2_frpc_pids() {
  pgrep -f '/.k2/frpc.toml' || true
}

# Only THIS daemon's frpc (~/.k2/frpc.toml). Postal (and anything else)
# may also dial k2e-01; that is not K2 Connect going dark.
plane_estab() {
  local pids
  pids="$(k2_frpc_pids | tr '\n' ',' | sed 's/,$//')"
  [[ -n "$pids" ]] || return 0
  # lsof selections are OR unless -a ANDs them — without -a this matches
  # every process dialing k2e-01 (Postal's frpc shares that relay).
  lsof -nP -a -p "$pids" -iTCP@"${K2E01}" -sTCP:ESTABLISHED 2>/dev/null || true
}

reload_launchd() {
  launchctl bootout "$TARGET" >/dev/null 2>&1 || true
  sleep 0.4
  launchctl bootstrap "$DOMAIN" "$PLIST" || die "launchctl bootstrap failed"
}

restore() {
  if [[ "$RESTORED" == "1" ]]; then
    return 0
  fi
  RESTORED=1
  log "RESTORE begin (elapsed=$(( $(date +%s) - T0 ))s)"
  if [[ -f "${BACKUP_DIR}/dev.k2.daemon.plist" ]]; then
    cp "${BACKUP_DIR}/dev.k2.daemon.plist" "$PLIST"
  fi
  # Keep the new (air-gap-capable) binary in the bundle — default-off is
  # today's behavior. Only roll the binary back if boot fails after env strip.
  reload_launchd
  local body
  if ! body="$(wait_boot 0)"; then
    log "new binary failed default-off boot — rolling back original k2-daemon"
    if [[ -f "${BACKUP_DIR}/k2-daemon.prev" ]]; then
      cp "${BACKUP_DIR}/k2-daemon.prev" "${BACKUP_DIR}/bundle-daemon-path.txt" >/dev/null 2>&1 || true
      local dest
      dest="$(cat "${BACKUP_DIR}/bundle-path")"
      cp "${BACKUP_DIR}/k2-daemon.prev" "$dest"
      codesign --force --sign - "$dest" >/dev/null 2>&1 || true
      reload_launchd
      body="$(wait_boot 0)"
    fi
  fi
  log "restored boot-status: $body"
  local deadline=$(( $(date +%s) + 40 ))
  while (( $(date +%s) < deadline )); do
    if [[ -n "$(k2_frpc_pids)" ]]; then
      log "k2 frpc is back pid=$(k2_frpc_pids | tr '\n' ' ')"
      return 0
    fi
    sleep 0.5
  done
  log "WARN: frpc did not return within 40s — tunnel.json untouched; try: k2 tunnel start"
}

on_exit() {
  local rc=$?
  restore || true
  if (( rc != 0 )); then
    log "exiting rc=$rc elapsed=$(( $(date +%s) - T0 ))s — see $LOG"
  fi
  exit "$rc"
}

NEW_DAEMON="${NEW_DAEMON:-}"
if [[ -z "$NEW_DAEMON" ]]; then
  die "set NEW_DAEMON to an aa5dd0e9+ k2-daemon binary (air-gap + LAN listen)"
fi
[[ -x "$NEW_DAEMON" ]] || die "NEW_DAEMON not executable: $NEW_DAEMON"
[[ -f "$PLIST" ]] || die "missing launchd plist $PLIST"

mkdir -p "$BACKUP_DIR"
touch "$LOG"
T0="$(date +%s)"
trap on_exit EXIT INT TERM

log "backup dir $BACKUP_DIR"
cp "$PLIST" "${BACKUP_DIR}/dev.k2.daemon.plist"
cp "$HOME_K2/tunnel.json" "${BACKUP_DIR}/tunnel.json" 2>/dev/null || true
DEST="$(python3 - <<'PY'
import plistlib, pathlib, sys
p = pathlib.Path.home() / "Library/LaunchAgents/dev.k2.daemon.plist"
d = plistlib.loads(p.read_bytes())
args = d.get("ProgramArguments") or []
if not args:
    sys.exit("plist has no ProgramArguments")
print(args[0])
PY
)"
[[ -x "$DEST" ]] || die "plist binary missing: $DEST"
echo "$DEST" >"${BACKUP_DIR}/bundle-path"
cp "$DEST" "${BACKUP_DIR}/k2-daemon.prev"
log "live binary $DEST"
log "preflight boot-status $(curl -sS -m 3 http://127.0.0.1:$(port_now)/boot-status)"
log "preflight k2 frpc $(k2_frpc_pids | tr '\n' ' ' || echo none)"
log "preflight plane estab: $(plane_estab | head -2 || echo none)"
LAN_IP="$(lan_ipv4)"
[[ -n "$LAN_IP" ]] || die "no en0/en1 IPv4"
log "LAN_IP=$LAN_IP port=$(port_now)"

# ── install new binary + launchd env (the 3-min clock starts at respawn) ──
log "installing $(basename "$NEW_DAEMON") + K2_AIRGAP=1 K2_LISTEN=lan"
cp "$NEW_DAEMON" "$DEST"
chmod 755 "$DEST"
codesign --force --sign - "$DEST" >/dev/null
/usr/libexec/PlistBuddy -c "Delete :EnvironmentVariables" "$PLIST" >/dev/null 2>&1 || true
/usr/libexec/PlistBuddy -c "Add :EnvironmentVariables dict" "$PLIST"
/usr/libexec/PlistBuddy -c "Add :EnvironmentVariables:K2_AIRGAP string 1" "$PLIST"
/usr/libexec/PlistBuddy -c "Add :EnvironmentVariables:K2_LISTEN string lan" "$PLIST"
T0="$(date +%s)"
reload_launchd
BODY="$(wait_boot 1)"
log "air-gap boot-status $BODY"
echo "$BODY" | python3 -c 'import json,sys
d=json.load(sys.stdin)
ag=d.get("airgap") or {}
ln=d.get("listen") or {}
assert ag.get("enabled") is True, d
assert ln.get("lan") is True, "listen.lan must be true (0.0.0.0); got %r" % ln
print("boot-status airgap.enabled=true listen.lan=true")'

# Listener really 0.0.0.0?
python3 - <<PY
import os, subprocess, sys
port = open(os.path.expanduser("~/.k2/daemon.port")).read().strip()
out = subprocess.check_output(["lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN"], text=True)
print(out)
if "*:{p}".format(p=port) not in out and "0.0.0.0:{p}".format(p=port) not in out:
    # macOS lsof shows *:port for IPv4 unspecified
    if f":{port}" not in out:
        sys.exit("listener not found on port %s" % port)
    if "127.0.0.1" in out and "*" not in out and "0.0.0.0" not in out:
        sys.exit("still loopback-only:\n" + out)
print("LISTEN ok")
PY

# LAN answers (loopback + RFC1918)
PORT="$(port_now)"
CODE_LO="$(http_code "http://127.0.0.1:${PORT}/boot-status")"
CODE_LAN="$(http_code "http://${LAN_IP}:${PORT}/boot-status")"
log "GET /boot-status loopback=$CODE_LO lan=$LAN_IP:$PORT -> $CODE_LAN"
[[ "$CODE_LO" == "200" ]] || die "loopback /boot-status HTTP $CODE_LO"
[[ "$CODE_LAN" == "200" ]] || die "LAN /boot-status HTTP $CODE_LAN (bind/firewall?)"

# Plane dark
sleep 1
FRPC="$(k2_frpc_pids)"
[[ -z "$FRPC" ]] || die "k2 frpc still running after air-gap: $FRPC"
EST="$(plane_estab)"
[[ -z "$EST" ]] || die "k2 frpc still ESTABLISHED to k2e-01:\n$EST"
log "no k2 frpc, no k2 ESTABLISHED to $K2E01"

CHK="$(http POST /cli/daemon/update/check)"
log "update/check $CHK"
echo "$CHK" | python3 -c 'import json,sys
d=json.load(sys.stdin)
err=d.get("error") or ""
assert "K2_AIRGAP" in err, d
print("update/check refused")'
START="$(http POST /cli/tunnel/start)"
log "tunnel/start $START"
echo "$START" | python3 -c 'import json,sys
d=json.load(sys.stdin)
err=d.get("error") or ""
assert "K2_AIRGAP" in err, d
print("tunnel/start refused")'

# Must still be inside the lease window
ELAPSED=$(( $(date +%s) - T0 ))
log "air-gap assertions passed in ${ELAPSED}s (restore-by ${RESTORE_BY}s, lease ${LEASE_SECS}s)"
(( ELAPSED < RESTORE_BY )) || die "took ${ELAPSED}s — too close to lease TTL"

# Restore is the EXIT trap — also call it now so we can assert recovery
restore
RESTORED=1
trap - EXIT INT TERM

# Post-restore: air-gap off, tunnel.json unchanged, frpc back
python3 - <<PY
import json, pathlib
a = json.loads(pathlib.Path("$HOME_K2/tunnel.json").read_text())
b = json.loads(pathlib.Path("${BACKUP_DIR}/tunnel.json").read_text())
assert a.get("subdomain") == b.get("subdomain"), (a.get("subdomain"), b.get("subdomain"))
assert a.get("token") == b.get("token"), "tunnel token mutated"
print("tunnel.json subdomain+token unchanged")
PY
BODY2="$(curl -sS -m 3 http://127.0.0.1:$(port_now)/boot-status)"
log "post-restore boot-status $BODY2"
echo "$BODY2" | python3 -c 'import json,sys
d=json.load(sys.stdin)
assert d.get("phase")=="ready"
ag=(d.get("airgap") or {}).get("enabled")
assert ag in (False, None), d
print("airgap off after restore")'
if [[ -z "$(k2_frpc_pids)" ]]; then
  die "k2 frpc did not return after restore"
fi
log "PASS elapsed=$(( $(date +%s) - T0 ))s backup=$BACKUP_DIR"
