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
# Designated box: k2-sandbox-01 (Hetzner bare metal, 12c/62GB) — see the
# boxes-inventory memory / git log 894112a for the box prep (gtk/webkit dev
# stack). Persistent cargo cache at ${GATE_DIR}-target keeps warm runs at a
# few seconds of compute. We build in our own directory and touch nothing
# else on the box (it also hosts the sandbox-engine dev env + Dedicated ref
# fixture). NEVER point this at linux-test.k2.dev — that DNS name is the
# LIVE K2 Connect relay.
#
# Env:
#   K2_LINUX_GATE_HOST   override the ssh target (default root@37.27.67.180)
#   K2_SKIP_LINUX_GATE=1 skip entirely, with a loud warning (box down etc.)
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
GATE_HOST="${K2_LINUX_GATE_HOST:-root@37.27.67.180}"
GATE_DIR="/root/k2-release-check"

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
