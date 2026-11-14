#!/usr/bin/env bash
# iPhones on this Mac compute node (prd-ios-on-mac-node-0467-v1 §3.3, §3.8).
#
#   sudo k2-node setup --sim [--server <slot|label>] [--private-home]
#   sudo k2-node setup --sim --remove
#
# Run after the node itself is installed (sudo k2-node setup …) and after
# the Mac's owner installed Xcode and an iOS runtime (the Xcode licence is
# yours to accept; K2 never does it):
#   sudo xcode-select -s /Applications/Xcode.app
#   sudo xcodebuild -license accept && sudo xcodebuild -runFirstLaunch
#   xcodebuild -downloadPlatform iOS
#
# Root is needed once. It creates the hidden user `_k2sim` (shell
# /usr/bin/false, home /var/k2sim), the group `k2simctl` whose only member
# is `_k2node`, the LaunchDaemon dev.k2.simhost (`k2-node simhost`, run as
# _k2sim), and turns iPhones on for one of your own servers (default: the
# owner server, slot 0). The Simulator and its apps run as _k2sim, outside
# the job wall, so they never reach the node's key; they can read whatever
# other users can read, so every home folder under /Users must be private
# first (--private-home makes them so).
#
# --remove takes all of that away again; the node stays.
set -euo pipefail
cd /

SIM_USER="_k2sim"
NODE_USER="_k2node"
CTL_GROUP="k2simctl"
ROOT="/var/k2sim"
CONFIG_DIR="/etc/k2-node"
BIN="/Library/Application Support/K2/node/k2-node"
PLIST="/Library/LaunchDaemons/dev.k2.simhost.plist"
BINARY=""
SERVER="0"
PRIVATE_HOME=0
REMOVE=0

die() { echo "install-sim-macos: $*" >&2; exit 1; }
while [ $# -gt 0 ]; do
    case "$1" in
        --binary) BINARY="${2:-}"; shift 2 ;;
        --server) SERVER="${2:-}"; shift 2 ;;
        --private-home) PRIVATE_HOME=1; shift ;;
        --no-private-home) shift ;;
        --remove) REMOVE=1; shift ;;
        --sim) shift ;;
        -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done
[ "$(uname -s)" = "Darwin" ] || die "iPhones run only on a Mac node"
[ "$(id -u)" = "0" ] || die "run as root: sudo k2-node setup --sim"

reload_daemon() {
    local label=$1 plist=$2 i
    launchctl bootout "system/$label" 2>/dev/null || true
    for i in $(seq 1 50); do
        launchctl print "system/$label" >/dev/null 2>&1 || break
        sleep 0.2
    done
    for i in 1 2 3; do
        launchctl bootstrap system "$plist" 2>/dev/null && return 0
        sleep 1
    done
    launchctl bootstrap system "$plist"
}

if [ "$REMOVE" = "1" ]; then
    launchctl bootout system/dev.k2.simhost 2>/dev/null || true
    rm -f "$PLIST"
    pkill -KILL -u "$SIM_USER" 2>/dev/null || true
    dscl . -read "/Users/$SIM_USER" >/dev/null 2>&1 && dscl . -delete "/Users/$SIM_USER"
    dscl . -read "/Groups/$SIM_USER" >/dev/null 2>&1 && dscl . -delete "/Groups/$SIM_USER"
    dscl . -read "/Groups/$CTL_GROUP" >/dev/null 2>&1 && dscl . -delete "/Groups/$CTL_GROUP"
    [ -d "$ROOT" ] && rm -rf -- "$ROOT"
    rm -f "$CONFIG_DIR/sim.toml"
    echo "Removed the iPhone service from this Mac (the compute node stays)."
    exit 0
fi

[ -x "$BIN" ] || die "the compute node isn't installed here ($BIN): sudo k2-node setup … first"
dscl . -read "/Users/$NODE_USER" >/dev/null 2>&1 || die "no $NODE_USER user: install the compute node first (sudo k2-node setup …)"
if [ -n "$BINARY" ] && ! [ "$BINARY" -ef "$BIN" ]; then
    echo "note: using the installed k2-node ($BIN), not $BINARY; install that one first if it's newer (sudo $BINARY setup …)" >&2
fi
"$BIN" sim status --json >/dev/null 2>&1 || die "the installed k2-node is too old for iPhones: install the new one first (sudo <new k2-node> setup …)"

# People's homes: Simulator apps run outside the job wall (review IR-4).
if [ "$PRIVATE_HOME" = "1" ]; then
    "$BIN" admin homes --fix --yes >/dev/null
fi
open_homes="$("$BIN" admin homes --json 2>/dev/null || echo '?')"
[ "$open_homes" = "[]" ] || die "home folders other users can read: $open_homes
  iPhone apps on this Mac run outside the job wall and could read them. Make them private
  (sudo chmod 700 <home>) or run again with --private-home."

free_id() {
    local used id
    used="$( { dscl . -list /Users UniqueID; dscl . -list /Groups PrimaryGroupID; } | awk '{print $2}')"
    for id in $(seq 200 400); do
        if ! printf '%s\n' "$used" | grep -qx "$id"; then
            echo "$id"
            return 0
        fi
    done
    return 1
}

echo "== the hidden user $SIM_USER and the group $CTL_GROUP"
if ! dscl . -read "/Groups/$SIM_USER" >/dev/null 2>&1; then
    gid="$(free_id)" || die "no free id in 200-400"
    dscl . -create "/Groups/$SIM_USER"
    dscl . -create "/Groups/$SIM_USER" PrimaryGroupID "$gid"
    dscl . -create "/Groups/$SIM_USER" RealName "K2 iPhone Simulator"
    dscl . -create "/Groups/$SIM_USER" Password '*'
fi
gid="$(dscl . -read "/Groups/$SIM_USER" PrimaryGroupID | awk '{print $2}')"
if ! dscl . -read "/Users/$SIM_USER" >/dev/null 2>&1; then
    uid="$gid"
    if dscl . -list /Users UniqueID | awk '{print $2}' | grep -qx "$uid"; then
        uid="$(free_id)" || die "no free id in 200-400"
    fi
    dscl . -create "/Users/$SIM_USER"
    dscl . -create "/Users/$SIM_USER" UniqueID "$uid"
    dscl . -create "/Users/$SIM_USER" PrimaryGroupID "$gid"
    dscl . -create "/Users/$SIM_USER" UserShell /usr/bin/false
    dscl . -create "/Users/$SIM_USER" NFSHomeDirectory "$ROOT"
    dscl . -create "/Users/$SIM_USER" RealName "K2 iPhone Simulator"
    dscl . -create "/Users/$SIM_USER" IsHidden 1
    dscl . -create "/Users/$SIM_USER" Password '*'
fi
if ! dscl . -read "/Groups/$CTL_GROUP" >/dev/null 2>&1; then
    cgid="$(free_id)" || die "no free id in 200-400"
    dscl . -create "/Groups/$CTL_GROUP"
    dscl . -create "/Groups/$CTL_GROUP" PrimaryGroupID "$cgid"
    dscl . -create "/Groups/$CTL_GROUP" RealName "K2 compute node: may reach the iPhone service"
    dscl . -create "/Groups/$CTL_GROUP" Password '*'
fi
for g in k2nodectl "$CTL_GROUP"; do
    if dseditgroup -o checkmember -m "$SIM_USER" "$g" >/dev/null 2>&1; then
        die "$SIM_USER must not be in $g"
    fi
done
NEW_MEMBER=0
if ! dseditgroup -o checkmember -m "$NODE_USER" "$CTL_GROUP" >/dev/null 2>&1; then
    dseditgroup -o edit -a "$NODE_USER" -t user "$CTL_GROUP"
    NEW_MEMBER=1
fi

echo "== $ROOT"
# Only the group's search bit on the home, so _k2node reaches run/ and
# nothing else; run/ is the socket's folder.
install -d -m 0710 -o "$SIM_USER" -g "$CTL_GROUP" "$ROOT"
install -d -m 0750 -o "$SIM_USER" -g "$CTL_GROUP" "$ROOT/run"
for d in sets tmp uploads bin cache cache/tmp log; do
    install -d -m 0700 -o "$SIM_USER" -g "$SIM_USER" "$ROOT/$d"
done

echo "== iPhones for server $SERVER"
"$BIN" sim enable --server "$SERVER" --config-dir "$CONFIG_DIR"

echo "== LaunchDaemon dev.k2.simhost"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"$BIN" sim install-files --binary "$BIN" --root "$ROOT" --out "$tmp" >/dev/null
install -m 0644 -o root -g wheel "$tmp/dev.k2.simhost.plist" "$PLIST"
reload_daemon dev.k2.simhost "$PLIST" || die "dev.k2.simhost didn't load (System Settings → General → Login Items may block background items)"
for i in $(seq 1 50); do
    [ -S "$ROOT/run/simhost.sock" ] && break
    sleep 0.2
done
[ -S "$ROOT/run/simhost.sock" ] || die "dev.k2.simhost didn't start: see $ROOT/log/simhost.log"

# A running k2-node only gets its new group at its next start.
NODE_STATUS="k2-node already reaches the service"
if [ "$NEW_MEMBER" = "1" ]; then
    launchctl kickstart -k system/dev.k2.node >/dev/null 2>&1 || true
    NODE_STATUS="k2-node restarted to join $CTL_GROUP (running jobs keep running)"
fi
sleep 2
STATUS="$(sudo -u "$NODE_USER" -H "$BIN" sim status 2>&1 || true)"

cat <<EOF

Done.
  user:     $SIM_USER (hidden; home $ROOT)
  service:  dev.k2.simhost (k2-node simhost as $SIM_USER; log $ROOT/log/simhost.log)
  node:     $NODE_STATUS
$(printf '%s\n' "$STATUS" | sed 's/^/  /')
  turn on for another of your servers: sudo "$BIN" sim enable --server <slot|label>
  remove:   sudo "$BIN" setup --sim --remove
EOF
