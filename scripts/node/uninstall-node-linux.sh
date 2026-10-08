#!/usr/bin/env bash
# Stop being a K2 compute node (Linux). Reverses install-node-linux.sh:
# stops the service (running jobs are killed), removes the unit, the user
# k2node and its home (key, ledger, job trees, mirrors, warm slots,
# toolchains), the binary and, unless --keep-config, /etc/k2-node and the
# k2nodectl group. Remove the node on the controller too:
#   k2 compute node remove <name>
set -euo pipefail

NODE_USER="k2node"
HOME_DIR="/var/lib/k2node"
CONFIG_DIR="/etc/k2-node"
BIN="/usr/local/libexec/k2-node"
UNIT="/etc/systemd/system/k2-node.service"
KEEP_CONFIG=0

die() { echo "uninstall-node-linux: $*" >&2; exit 1; }
while [ $# -gt 0 ]; do
    case "$1" in
        --keep-config) KEEP_CONFIG=1; shift ;;
        -h|--help) sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done
[ "$(uname -s)" = "Linux" ] || die "Linux only"
[ "$(id -u)" = "0" ] || die "run as root (sudo)"

# Only ever delete the paths this installer made.
safe_rm() {
    case "$1" in
        /var/lib/k2node|/etc/k2-node|/usr/local/libexec/k2-node) rm -rf -- "$1" ;;
        *) die "refusing to delete $1" ;;
    esac
}

if command -v systemctl >/dev/null; then
    systemctl disable --now k2-node.service 2>/dev/null || true
    rm -f "$UNIT"
    systemctl daemon-reload || true
fi
pkill -KILL -u "$NODE_USER" 2>/dev/null || true
if id -u "$NODE_USER" >/dev/null 2>&1; then
    userdel "$NODE_USER" || die "userdel $NODE_USER failed"
fi
getent group "$NODE_USER" >/dev/null && groupdel "$NODE_USER" 2>/dev/null || true
safe_rm "$HOME_DIR"
safe_rm "$BIN"
if [ "$KEEP_CONFIG" = "0" ]; then
    safe_rm "$CONFIG_DIR"
    getent group k2nodectl >/dev/null && groupdel k2nodectl || true
fi
echo "Removed the compute node. Also run on the controller: k2 compute node remove <name>"
