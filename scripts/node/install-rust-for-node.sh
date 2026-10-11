#!/usr/bin/env bash
# Install a Rust toolchain for the compute node's jobs (CN27). Kept for
# the existing nodes and docs; it is now one recipe of the toolchain
# layer: `k2-node tools install rust@<channel> [cargo-nextest]`, run AS
# the node user with CARGO_HOME / RUSTUP_HOME under its home, so the
# human's ~/.cargo is never touched. Jobs can also just declare what they
# need (`k2 compute run --needs rust@stable,cargo-nextest`, or the repo's
# rust-toolchain.toml) and the node installs it on demand.
#
#   sudo scripts/node/install-rust-for-node.sh [--nextest] [--toolchain stable]
set -euo pipefail
# From / : the node user can't read a 0700 home the installer just made
# private (the 2026-10-10 Mac mini bug).
cd /

NEXTEST=0
TOOLCHAIN="stable"
die() { echo "install-rust-for-node: $*" >&2; exit 1; }
while [ $# -gt 0 ]; do
    case "$1" in
        --nextest) NEXTEST=1; shift ;;
        --toolchain) TOOLCHAIN="${2:-}"; shift 2 ;;
        -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done
[ "$(id -u)" = "0" ] || die "run as root (sudo); it drops to the node user for the install"

case "$(uname -s)" in
    Darwin) NODE_USER="_k2node"; HOME_DIR="/var/k2node"; BIN="/Library/Application Support/K2/node/k2-node" ;;
    Linux) NODE_USER="k2node"; HOME_DIR="/var/lib/k2node"; BIN="/usr/local/libexec/k2-node" ;;
    *) die "unsupported OS" ;;
esac
id -u "$NODE_USER" >/dev/null 2>&1 || die "$NODE_USER doesn't exist; run k2-node setup (install-node-*.sh) first"
[ -x "$BIN" ] || die "$BIN is missing; run k2-node setup first"

needs=("rust@$TOOLCHAIN")
[ "$NEXTEST" = "1" ] && needs+=("cargo-nextest")
if command -v runuser >/dev/null; then
    runuser -u "$NODE_USER" -- env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin "$BIN" tools install --home "$HOME_DIR" "${needs[@]}"
else
    sudo -u "$NODE_USER" -H env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin "$BIN" tools install --home "$HOME_DIR" "${needs[@]}"
fi
echo "Rust is ready for jobs on this node (k2-node re-probes tools within 10 minutes)."
