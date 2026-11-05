#!/bin/bash
# linux-build-gate.sh — prove the whole workspace compiles warning-free on
# real Linux. Called by scripts/release.sh (Step 0.6) and scripts/build-app.sh
# (Step 0), and safe to run standalone any time.
#
# K2 ships Linux artifacts at tag time (app-linux.yml + daemon-binaries.yml,
# or K2 Compute in K2_LINUX_BUILDS=shadow/compute releases), but tag time is
# TOO LATE to discover a Linux build break. The per-push checks cover only
# k2-core, k2-daemon and their crates; the k2 (src-tauri) crate compiles on
# Linux ONLY here and at tag time. This gate caught a real break on its
# first shakedown run (mac-only test module in secrets.rs).
#
# Where it runs (K2_LINUX_GATE):
#   compute (default)  a K2 Compute job at this checkout's HEAD (plus its
#       uncommitted changes, --dirty), on a Linux x86 node K2 picks among
#       the ones this workspace is granted (never k2-arch: an Arch node has
#       no Debian sys: packages), --tier ci, the pinned Rust
#       (rust-toolchain.toml at HEAD). Needs a grant with --ci on the Linux
#       nodes for the workspace of the checkout you run from (z3mbpZ's
#       release checkout: pod-leader). K2_LINUX_GATE_WORKSPACE sends
#       --workspace; K2_LINUX_GATE_TIER=standard for a workspace without a
#       --ci grant; K2_LINUX_GATE_MAX_WAIT (default 90m).
#   ssh  the old box, while it exists: rsync this tree to
#       ${K2_LINUX_GATE_HOST:-k2-ci@<ship box>}:${K2_LINUX_GATE_DIR} and run
#       cargo check there (persistent cargo cache at ${GATE_DIR}-target).
#       NEVER point this at linux-test.k2.dev.
#
# Env:
#   K2_LINUX_GATE        compute | ssh (default compute)
#   K2_LINUX_GATE_HOST   ssh mode: the ssh target (default k2-ci@40.160.54.133)
#   K2_LINUX_GATE_DIR    ssh mode: the remote tree
#   K2_SKIP_LINUX_GATE=1 skip entirely, with a loud warning (nodes down etc.)
#   K2_BIN               the k2 CLI (tests stub it; default k2)
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${K2_LINUX_GATE:-compute}"

if [ "${K2_SKIP_LINUX_GATE:-0}" = "1" ]; then
    echo "⚠⚠ LINUX BUILD GATE SKIPPED (K2_SKIP_LINUX_GATE=1) ⚠⚠"
    echo "  Linux breakage will not surface until the Linux builds at tag time."
    exit 0
fi

# The check itself, the same in both modes. fetch-frpc.sh auto-detects the
# Linux triple; the k2 crate's build script hard-requires the staged
# sidecar even for `cargo check`.
GATE_CMD="./scripts/fetch-frpc.sh && RUSTFLAGS='-D warnings' cargo check --workspace --all-targets"

case "$MODE" in
    compute)
        # shellcheck source=scripts/lib/rust-pin.sh
        source "$PROJECT_DIR/scripts/lib/rust-pin.sh"
        # shellcheck source=scripts/lib/compute-tier.sh
        source "$PROJECT_DIR/scripts/lib/compute-tier.sh"
        K2="${K2_BIN:-k2}"
        head_sha="$(git -C "$PROJECT_DIR" rev-parse HEAD)"
        rust="$(K2_RUST_PIN_REPO="$PROJECT_DIR" k2_rust_need "$head_sha")"
        # The k2 crate's build scripts find GTK/WebKit (and the voice stack's
        # ALSA) through pkg-config, as the Linux app build does.
        needs="$rust,cmake@3,sys:pkg-config,sys:zlib1g-dev,sys:libssl-dev,sys:libasound2-dev"
        needs="$needs,sys:libwebkit2gtk-4.1-dev,sys:libgtk-3-dev,sys:libayatana-appindicator3-dev,sys:librsvg2-dev,sys:libxdo-dev"
        args=(compute run --any --os linux --arch x86_64 --sha "$head_sha")
        while IFS= read -r t; do [ -n "$t" ] && args+=("$t"); done < <(K2_BIN="$K2" k2_compute_tier_args "${K2_LINUX_GATE_TIER:-ci}")
        while IFS= read -r t; do [ -n "$t" ] && args+=("$t"); done < <(K2_BIN="$K2" k2_compute_avoid_args)
        if [ -n "$(git -C "$PROJECT_DIR" status --porcelain --untracked-files=normal)" ]; then
            args+=(--dirty)
            dirty=" + uncommitted changes"
        else
            dirty=""
        fi
        [ -n "${K2_LINUX_GATE_WORKSPACE:-}" ] && args+=(--workspace "$K2_LINUX_GATE_WORKSPACE")
        args+=(--needs "$needs" --mem 24G --timeout 90m --queue-timeout 1h --retries 1
            --wait --failures --max-wait "${K2_LINUX_GATE_MAX_WAIT:-90m}")
        echo "Linux build gate on K2 Compute: ${head_sha:0:12}${dirty}, $rust (K2 picks a Linux x86 node)..."
        rc=0
        (cd "$PROJECT_DIR" && "$K2" "${args[@]}" -- sh -c "$GATE_CMD") || rc=$?
        if [ "$rc" -ne 0 ]; then
            echo "FATAL: Linux build gate failed on K2 Compute (exit $rc)." >&2
            echo "  A build break: fix it. A node or grant problem (exit 3): k2 compute nodes;" >&2
            echo "  or run it on the old box: K2_LINUX_GATE=ssh, or skip loudly: K2_SKIP_LINUX_GATE=1." >&2
            exit "$rc"
        fi
        ;;
    ssh)
        GATE_HOST="${K2_LINUX_GATE_HOST:-k2-ci@40.160.54.133}"
        GATE_DIR="${K2_LINUX_GATE_DIR:-/home/k2-ci/k2-release-check}"
        echo "Linux build gate on ${GATE_HOST} (${GATE_DIR})..."
        rsync -az --delete \
            --exclude target --exclude 'target-*' --exclude node_modules --exclude .git \
            --exclude out --exclude /dist --exclude dist-windows --exclude .bmr-wt \
            "$PROJECT_DIR/" "${GATE_HOST}:${GATE_DIR}/"
        ssh "$GATE_HOST" "set -e; export PATH=\"\$HOME/.cargo/bin:\$PATH\"; \
            cd ${GATE_DIR} && export CARGO_TARGET_DIR=${GATE_DIR}-target && $GATE_CMD"
        ;;
    *)
        echo "FATAL: K2_LINUX_GATE must be compute or ssh (got $MODE)" >&2
        exit 2
        ;;
esac
echo "  Linux build gate passed."
