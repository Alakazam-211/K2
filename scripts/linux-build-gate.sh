#!/bin/bash
# linux-build-gate.sh — prove the whole workspace compiles warning-free on
# real Linux. Called by scripts/release.sh (Step 0.6) and scripts/build-app.sh
# (Step 0), and safe to run standalone any time.
#
# K2 ships Linux artifacts from CI at tag time (app-linux.yml +
# daemon-binaries.yml) — but tag time is TOO LATE to discover a Linux build
# break. Per-push CI checks only k2-core/k2-daemon; the k2 (src-tauri) crate
# compiles on Linux ONLY here and at tag time. This gate caught a real break
# on its first shakedown run (mac-only test module in secrets.rs).
#
# Designated box: the Ubuntu ship host 40.160.54.133 (hostname k2-linux,
# user k2-ci). AX41 is becoming the Windows CI machine and is no longer
# this gate. Persistent cargo cache at ${GATE_DIR}-target. This directory
# is not the GitHub runner workdir and not /opt/k2. NEVER point this at
# linux-test.k2.dev.
#
# Env:
#   K2_LINUX_GATE_HOST   override the ssh target (default k2-ci@40.160.54.133)
#   K2_LINUX_GATE_DIR    override the remote tree
#   K2_SKIP_LINUX_GATE=1 skip entirely, with a loud warning (box down etc.)
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
GATE_HOST="${K2_LINUX_GATE_HOST:-k2-ci@40.160.54.133}"
GATE_DIR="${K2_LINUX_GATE_DIR:-/home/k2-ci/k2-release-check}"

if [ "${K2_SKIP_LINUX_GATE:-0}" = "1" ]; then
    echo "⚠⚠ LINUX BUILD GATE SKIPPED (K2_SKIP_LINUX_GATE=1) ⚠⚠"
    echo "  Linux breakage will not surface until CI at tag time."
    exit 0
fi

echo "Linux build gate on ${GATE_HOST} (${GATE_DIR})..."
rsync -az --delete \
    --exclude target --exclude 'target-*' --exclude node_modules --exclude .git \
    --exclude out --exclude dist --exclude dist-windows --exclude .bmr-wt \
    "$PROJECT_DIR/" "${GATE_HOST}:${GATE_DIR}/"
# fetch-frpc.sh auto-detects the Linux triple; the k2 crate's build script
# hard-requires the staged sidecar even for `cargo check`.
ssh "$GATE_HOST" "set -e; export PATH=\"\$HOME/.cargo/bin:\$PATH\"; \
    cd ${GATE_DIR} && ./scripts/fetch-frpc.sh && \
    CARGO_TARGET_DIR=${GATE_DIR}-target RUSTFLAGS='-D warnings' \
    cargo check --workspace --all-targets"
echo "  Linux build gate passed."
