#!/usr/bin/env bash
# Install a Rust toolchain for the compute node's jobs (CN27): rustup runs
# AS the node user with CARGO_HOME / RUSTUP_HOME under its home, so the
# human's ~/.cargo is never touched. Jobs find it on PATH
# (<home>/toolchains/cargo/bin comes first).
#
#   sudo scripts/node/install-rust-for-node.sh [--nextest] [--toolchain stable]
set -euo pipefail

NEXTEST=0
TOOLCHAIN="stable"
die() { echo "install-rust-for-node: $*" >&2; exit 1; }
while [ $# -gt 0 ]; do
    case "$1" in
        --nextest) NEXTEST=1; shift ;;
        --toolchain) TOOLCHAIN="${2:-}"; shift 2 ;;
        -h|--help) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done
[ "$(id -u)" = "0" ] || die "run as root (sudo)"

case "$(uname -s)" in
    Darwin) NODE_USER="_k2node"; HOME_DIR="/var/k2node" ;;
    Linux) NODE_USER="k2node"; HOME_DIR="/var/lib/k2node" ;;
    *) die "unsupported OS" ;;
esac
id -u "$NODE_USER" >/dev/null 2>&1 || die "$NODE_USER doesn't exist; run install-node-*.sh first"

TC="$HOME_DIR/toolchains"
install -d -m 0755 -o "$NODE_USER" "$TC" "$TC/cargo" "$TC/rustup"
# /tmp, not $TMPDIR: the node user must be able to read the script.
tmp="$(mktemp -d /tmp/k2-node-rust.XXXXXX)"
trap 'rm -rf "$tmp"' EXIT
chmod 0755 "$tmp"
curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$tmp/rustup-init.sh"
chmod 0644 "$tmp/rustup-init.sh"

as_node() {
    local env=(env -i HOME="$HOME_DIR" CARGO_HOME="$TC/cargo" RUSTUP_HOME="$TC/rustup"
        PATH="$TC/cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
    if command -v runuser >/dev/null; then
        runuser -u "$NODE_USER" -- "${env[@]}" "$@"
    else
        sudo -u "$NODE_USER" -H "${env[@]}" "$@"
    fi
}

as_node sh "$tmp/rustup-init.sh" -y --no-modify-path --profile minimal --default-toolchain "$TOOLCHAIN"
as_node "$TC/cargo/bin/rustc" --version
if [ "$NEXTEST" = "1" ]; then
    as_node "$TC/cargo/bin/cargo" install cargo-nextest --locked
    as_node "$TC/cargo/bin/cargo" nextest --version
fi
echo "Rust is ready for jobs on this node (k2-node re-probes tools within 10 minutes)."
