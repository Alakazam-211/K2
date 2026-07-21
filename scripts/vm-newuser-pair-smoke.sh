#!/bin/bash
# Host orchestrator: clean Tart macOS VM new-user pairing smoke.
#
# Installs the given signed DMG on a fresh Sequoia guest (no prior K2 / no
# ~/.k2*), starts the bundled k2-daemon, and asserts the 0.40.35+ pairing
# contract:
#   - ~/.k2 exists
#   - ~/.k2so is a symlink → ~/.k2  (missing on 0.40.33/34 fresh installs)
#   - daemon.port readable via ~/.k2so (thin-client path)
#   - GET /boot-status on that port returns HTTP 200
#
# This is the check that would have failed on 0.40.33/34 while upgrades and
# Gatekeeper-only tests still looked green.
#
# Usage:
#   scripts/vm-newuser-pair-smoke.sh <dmg-path> [version-label]
#
# Example:
#   scripts/vm-newuser-pair-smoke.sh target/release/bundle/dmg/K2_0.40.54_aarch64.dmg 0.40.54
#
# Requirements (Apple Silicon macOS host):
#   - tart (brew install cirruslabs/cli/tart)
#   - sshpass (brew install sshpass)
#   - First run pulls ghcr.io/cirruslabs/macos-sequoia-base (~25GB compressed)
#
# Escape (loud): K2_SKIP_VM_PAIRING_SMOKE=1
#   Skips the gate. Prefer fixing tart/setup over skipping on release machines.
#
# Env:
#   K2_VM_BASE_IMAGE   — local tart image name or OCI ref
#                        default: k2-newuser-base, else pull sequoia-base
#   K2_VM_SSH_USER     — default admin
#   K2_VM_SSH_PASS     — default admin (cirruslabs image)
#   K2_VM_KEEP=1       — leave the ephemeral VM after a failure (debug)

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
GUEST_SCRIPT="$PROJECT_DIR/scripts/vm-newuser-pair-guest.sh"

if [ "${K2_SKIP_VM_PAIRING_SMOKE:-}" = "1" ]; then
  echo "WARNING: K2_SKIP_VM_PAIRING_SMOKE=1 — skipping clean-VM new-user pairing smoke." >&2
  echo "WARNING: This gate catches fresh-install ~/.k2so pairing regressions (0.40.33/34)." >&2
  echo "WARNING: Only skip when tart is unavailable; do not skip on release cut machines." >&2
  exit 0
fi

DMG_PATH="${1:-}"
LABEL="${2:-}"
if [ -z "$DMG_PATH" ]; then
  echo "Usage: $0 <dmg-path> [version-label]" >&2
  exit 2
fi
if [ ! -f "$DMG_PATH" ]; then
  echo "FATAL: DMG not found: $DMG_PATH" >&2
  exit 1
fi
DMG_PATH="$(cd "$(dirname "$DMG_PATH")" && pwd)/$(basename "$DMG_PATH")"
DMG_NAME="$(basename "$DMG_PATH")"
if [ -z "$LABEL" ]; then
  # K2_0.40.54_aarch64.dmg → 0.40.54 ; K2_0.40.54-test_aarch64.dmg → 0.40.54-test
  LABEL="$(echo "$DMG_NAME" | sed -E 's/^K2_//; s/_aarch64\.dmg$//; s/\.dmg$//')"
fi

if [ ! -f "$GUEST_SCRIPT" ]; then
  echo "FATAL: guest script missing: $GUEST_SCRIPT" >&2
  exit 1
fi

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "FATAL: clean-VM pairing smoke requires Apple Silicon macOS (got $(uname -s)/$(uname -m))." >&2
  echo "       Install tart on an arm64 Mac, or set K2_SKIP_VM_PAIRING_SMOKE=1 knowingly." >&2
  exit 1
fi

export PATH="/opt/homebrew/bin:/usr/local/bin:${PATH:-}"
if ! command -v tart >/dev/null 2>&1; then
  echo "FATAL: tart not found on PATH." >&2
  echo "       brew tap cirruslabs/cli && brew install cirruslabs/cli/tart" >&2
  exit 1
fi
if ! command -v sshpass >/dev/null 2>&1; then
  echo "FATAL: sshpass not found on PATH (needed for guest SSH)." >&2
  echo "       brew install sshpass" >&2
  exit 1
fi

SSH_USER="${K2_VM_SSH_USER:-admin}"
SSH_PASS="${K2_VM_SSH_PASS:-admin}"
BASE_PREFERRED="${K2_VM_BASE_IMAGE:-}"
OCI_DEFAULT="ghcr.io/cirruslabs/macos-sequoia-base:latest"
LOCAL_BASE="k2-newuser-base"
# Ephemeral name — deleted after run
VM_NAME="k2-pair-smoke-$$"
HOST_REPORT="/tmp/k2-newuser-pair-host-${LABEL}-$$.txt"
: > "$HOST_REPORT"

ssh_opts=(
  -o StrictHostKeyChecking=no
  -o UserKnownHostsFile=/dev/null
  -o IdentitiesOnly=yes
  -o PreferredAuthentications=password
  -o PubkeyAuthentication=no
  -o NumberOfPasswordPrompts=1
  -o ConnectTimeout=10
)

ssh_to() {
  sshpass -p "$SSH_PASS" ssh "${ssh_opts[@]}" "${SSH_USER}@${IP}" "$@"
}
scp_to() {
  sshpass -p "$SSH_PASS" scp "${ssh_opts[@]}" "$@"
}

cleanup() {
  local rc=$?
  if [ -n "${IP:-}" ]; then
    ssh_to 'pkill -x k2-daemon 2>/dev/null; pkill -x k2so-daemon 2>/dev/null; true' 2>/dev/null || true
  fi
  if [ "${K2_VM_KEEP:-}" = "1" ] && [ "$rc" -ne 0 ]; then
    echo "K2_VM_KEEP=1 — leaving VM '$VM_NAME' for debug (tart stop/delete when done)." >&2
    return
  fi
  tart stop "$VM_NAME" 2>/dev/null || true
  sleep 1
  tart delete "$VM_NAME" 2>/dev/null || true
}
trap cleanup EXIT

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Clean-VM new-user pairing smoke"
echo "  label=$LABEL"
echo "  dmg=$DMG_PATH"
echo "═══════════════════════════════════════════════════"

# Resolve base image
BASE=""
if [ -n "$BASE_PREFERRED" ]; then
  BASE="$BASE_PREFERRED"
elif tart list 2>/dev/null | awk '{print $2}' | grep -qx "$LOCAL_BASE"; then
  BASE="$LOCAL_BASE"
elif tart list 2>/dev/null | awk '{print $2}' | grep -qx "k2-gk-test"; then
  BASE="k2-gk-test"
else
  BASE="$OCI_DEFAULT"
fi
echo "  base=$BASE"

echo "  cloning ephemeral VM $VM_NAME ..."
# Stop base if it is a local name (not an OCI ref)
case "$BASE" in
  ghcr.io/*|*/*:*) ;; # OCI
  *) tart stop "$BASE" 2>/dev/null || true ;;
esac

if ! tart clone "$BASE" "$VM_NAME" 2>/tmp/tart-clone-err.txt; then
  echo "  clone from $BASE failed — ensuring durable base $LOCAL_BASE from $OCI_DEFAULT ..."
  cat /tmp/tart-clone-err.txt >&2 || true
  tart stop "$LOCAL_BASE" 2>/dev/null || true
  if ! tart list 2>/dev/null | awk '{print $2}' | grep -qx "$LOCAL_BASE"; then
    tart clone "$OCI_DEFAULT" "$LOCAL_BASE"
  fi
  BASE="$LOCAL_BASE"
  tart clone "$BASE" "$VM_NAME"
fi

# Prefer a durable local base for the next run (faster than OCI pull)
if ! tart list 2>/dev/null | awk '{print $2}' | grep -qx "$LOCAL_BASE"; then
  if tart list 2>/dev/null | awk '{print $2}' | grep -qx "$VM_NAME"; then
    echo "  seeding durable base $LOCAL_BASE ..."
    tart clone "$VM_NAME" "$LOCAL_BASE" 2>/dev/null || true
  fi
fi

echo "  starting $VM_NAME (no graphics) ..."
nohup tart run "$VM_NAME" --no-graphics >/tmp/tart-run-${VM_NAME}.log 2>&1 &
sleep 10

IP=""
for _ in $(seq 1 60); do
  IP=$(tart ip "$VM_NAME" 2>/dev/null || true)
  if [ -n "$IP" ]; then break; fi
  sleep 3
done
if [ -z "$IP" ]; then
  echo "FATAL: no IP for VM $VM_NAME (see /tmp/tart-run-${VM_NAME}.log)" >&2
  exit 1
fi
echo "  IP=$IP"

for _ in $(seq 1 40); do
  if ssh_to 'echo SSH_OK' 2>/dev/null | grep -q SSH_OK; then
    echo "  SSH ready"
    break
  fi
  sleep 4
done
if ! ssh_to 'echo SSH_OK' 2>/dev/null | grep -q SSH_OK; then
  echo "FATAL: SSH to guest failed (user=$SSH_USER). Cirrus images use admin/admin." >&2
  exit 1
fi

# Clean slate on guest
ssh_to 'rm -rf ~/k2-gk-testdata ~/.k2 ~/.k2so 2>/dev/null; mkdir -p ~/k2-gk-testdata; true'
ssh_to 'test ! -e /Applications/K2.app || (sudo -n rm -rf /Applications/K2.app 2>/dev/null; rm -rf /Applications/K2.app 2>/dev/null); true'

echo "  copying DMG + guest script ..."
scp_to "$GUEST_SCRIPT" "$DMG_PATH" "${SSH_USER}@${IP}:~/k2-gk-testdata/"

echo "  running guest pairing assertions ..."
set +e
ssh_to "chmod +x ~/k2-gk-testdata/vm-newuser-pair-guest.sh; \
  export K2_LABEL='$LABEL' K2_DMG_NAME='$DMG_NAME' K2_TEST_DIR=\$HOME/k2-gk-testdata; \
  bash ~/k2-gk-testdata/vm-newuser-pair-guest.sh; echo GUEST_EXIT=\$?" \
  2>&1 | tee "$HOST_REPORT"
GUEST_RC=0
if grep -q 'GUEST_EXIT=0' "$HOST_REPORT"; then
  GUEST_RC=0
elif grep -q 'VERDICT_.*=PASS' "$HOST_REPORT"; then
  GUEST_RC=0
else
  GUEST_RC=1
fi
set -e

# Pull guest report if present
scp_to "${SSH_USER}@${IP}:~/k2-newuser-${LABEL}.txt" "/tmp/k2-newuser-guest-${LABEL}.txt" 2>/dev/null || true

if [ "$GUEST_RC" -ne 0 ]; then
  echo "" >&2
  echo "FATAL: clean-VM new-user pairing smoke FAILED for $LABEL" >&2
  echo "       This is the 0.40.33/34 class of regression (fresh install cannot pair)." >&2
  echo "       Host log: $HOST_REPORT" >&2
  if [ -f "/tmp/k2-newuser-guest-${LABEL}.txt" ]; then
    echo "       Guest log: /tmp/k2-newuser-guest-${LABEL}.txt" >&2
    grep -E 'ASSERT_|VERDICT_' "/tmp/k2-newuser-guest-${LABEL}.txt" >&2 || true
  fi
  exit 1
fi

echo "  ✓ clean-VM new-user pairing smoke PASSED for $LABEL"
exit 0
