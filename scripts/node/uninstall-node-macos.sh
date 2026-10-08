#!/usr/bin/env bash
# Stop being a K2 compute node (macOS). Reverses install-node-macos.sh:
# boots out the LaunchDaemon (running jobs are killed), removes the plist,
# the hidden user _k2node and its home /var/k2node, the binary and,
# unless --keep-config, /etc/k2-node and the k2nodectl group. The
# human's home stays private (chmod 700) — that's yours to undo.
# Remove the node on the controller too: k2 compute node remove <name>
set -euo pipefail

NODE_USER="_k2node"
HOME_DIR="/var/k2node"
CONFIG_DIR="/etc/k2-node"
BIN_DIR="/Library/Application Support/K2/node"
PLIST="/Library/LaunchDaemons/dev.k2.node.plist"
KEEP_CONFIG=0

die() { echo "uninstall-node-macos: $*" >&2; exit 1; }
while [ $# -gt 0 ]; do
    case "$1" in
        --keep-config) KEEP_CONFIG=1; shift ;;
        -h|--help) sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done
[ "$(uname -s)" = "Darwin" ] || die "macOS only"
[ "$(id -u)" = "0" ] || die "run as root (sudo)"

safe_rm() {
    case "$1" in
        /var/k2node|/etc/k2-node|"/Library/Application Support/K2/node") rm -rf -- "$1" ;;
        *) die "refusing to delete $1" ;;
    esac
}

launchctl bootout system/dev.k2.node 2>/dev/null || true
rm -f "$PLIST"
pkill -KILL -u "$NODE_USER" 2>/dev/null || true
dscl . -read "/Users/$NODE_USER" >/dev/null 2>&1 && dscl . -delete "/Users/$NODE_USER"
dscl . -read "/Groups/$NODE_USER" >/dev/null 2>&1 && dscl . -delete "/Groups/$NODE_USER"
safe_rm "$HOME_DIR"
safe_rm "$BIN_DIR"
if [ "$KEEP_CONFIG" = "0" ]; then
    safe_rm "$CONFIG_DIR"
    dscl . -read /Groups/k2nodectl >/dev/null 2>&1 && dscl . -delete /Groups/k2nodectl
fi
echo "Removed the compute node. Also run on the controller: k2 compute node remove <name>"
