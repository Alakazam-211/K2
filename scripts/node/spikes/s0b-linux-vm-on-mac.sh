#!/usr/bin/env bash
# S0(b) spike — prd-k2-compute-nodes-v1 §9.5 / §21.4.
#
# On an Apple Silicon Mac mini: start an aarch64 Ubuntu VM with Lima
# (Virtualization.framework, Rosetta on), build and test K2's Rust crates
# inside it, and report how long it takes and what fails. This is the
# "a Linux VM on a Mac is its own node" path (labels os=linux,
# arch=aarch64, vm=true). x86_64-only failures still need z13flow.
#
# Run ON the mini as the human (no sudo):
#   scp scripts/node/spikes/s0b-linux-vm-on-mac.sh <user>@<mini>:
#   ssh <user>@<mini> bash s0b-linux-vm-on-mac.sh --sha <commit>
# Options:
#   --sha REV          commit of github.com/Alakazam-211/K2 (required)
#   --cpus N --mem GiB VM size (default: half the cores, 16 GiB)
#   --packages "…"     cargo -p list (default: k2-node-proto k2-node k2-core)
#   --install-lima     `brew install lima` when limactl is missing
#   --keep             keep the VM (named k2s0) afterwards
#   --report PATH      default /tmp/k2-s0b-report.txt

set -euo pipefail

SHA=""
CPUS=""
MEM=16
PACKAGES="k2-node-proto k2-node k2-core"
INSTALL=0
KEEP=0
REPORT="/tmp/k2-s0b-report.txt"
VM="k2s0"
while [ $# -gt 0 ]; do
    case "$1" in
        --sha) SHA="${2:-}"; shift 2 ;;
        --cpus) CPUS="${2:-}"; shift 2 ;;
        --mem) MEM="${2:-}"; shift 2 ;;
        --packages) PACKAGES="${2:-}"; shift 2 ;;
        --install-lima) INSTALL=1; shift ;;
        --keep) KEEP=1; shift ;;
        --report) REPORT="${2:-}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ] || { echo "this spike is only for Apple Silicon Macs" >&2; exit 2; }
[ "$(id -u)" != "0" ] || { echo "run as the human, not root (Lima VMs belong to a user)" >&2; exit 2; }
[ -n "$SHA" ] || { echo "--sha <commit> is required" >&2; exit 2; }
case "$SHA" in *[!0-9a-fA-F]*) echo "--sha must be a hex commit" >&2; exit 2 ;; esac
[ -n "$CPUS" ] || CPUS=$(( $(sysctl -n hw.ncpu) / 2 ))

if ! command -v limactl >/dev/null 2>&1; then
    if [ "$INSTALL" = "1" ]; then
        brew install lima
    else
        echo "limactl is missing: install Lima (brew install lima) or pass --install-lima" >&2
        exit 2
    fi
fi

cleanup() {
    local rc=$?
    if [ "$KEEP" = "0" ]; then
        limactl stop "$VM" >/dev/null 2>&1 || true
        limactl delete --force "$VM" >/dev/null 2>&1 || true
    fi
    exit "$rc"
}
trap cleanup EXIT

t0=$(date +%s)
if ! limactl list -q 2>/dev/null | grep -qx "$VM"; then
    limactl create --name="$VM" --vm-type=vz --rosetta --cpus="$CPUS" --memory="$MEM" --disk=120 \
        --mount-none --tty=false template://ubuntu-lts
fi
limactl start --tty=false "$VM"
t_boot=$(( $(date +%s) - t0 ))

PFLAGS=""
for p in $PACKAGES; do PFLAGS="$PFLAGS -p $p"; done
t1=$(date +%s)
set +e
limactl shell "$VM" -- bash -lc "
set -e
sudo apt-get update -qq
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential cmake clang pkg-config libssl-dev git curl >/dev/null
[ -x \"\$HOME/.cargo/bin/cargo\" ] || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
. \"\$HOME/.cargo/env\"
cargo nextest --version >/dev/null 2>&1 || cargo install cargo-nextest --locked >/dev/null
rm -rf K2 && git init -q K2 && cd K2
git fetch -q --depth 1 https://github.com/Alakazam-211/K2.git $SHA && git checkout -q FETCH_HEAD
uname -m
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo nextest run $PFLAGS --no-fail-fast 2>&1
" > /tmp/k2-s0b.log 2>&1
rc=$?
set -e
t_test=$(( $(date +%s) - t1 ))

{
    echo "S0(b) — K2 tests in an aarch64 Linux VM on $(hostname)"
    echo "mac: $(sw_vers -productVersion) $(sysctl -n machdep.cpu.brand_string)  vm: ${CPUS} cpus ${MEM} GiB"
    echo "commit: $SHA  packages: $PACKAGES"
    echo "VM create+boot: ${t_boot}s  provision+build+test: ${t_test}s  rc: $rc"
    echo "guest arch: $(grep -m1 -E '^(aarch64|x86_64)$' /tmp/k2-s0b.log || echo '?')"
    echo
    grep -E "Summary|tests run" /tmp/k2-s0b.log | tail -3
    echo
    echo "failures:"
    grep -E "^\s+FAIL \[" /tmp/k2-s0b.log | sed -E 's/^\s+FAIL \[[^]]*\]\s*/  /' | sort -u || true
} > "$REPORT"
cat "$REPORT"
echo "(full log: /tmp/k2-s0b.log)"
