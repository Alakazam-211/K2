#!/bin/bash
# fetch-frpc.sh — stage the `frpc` tunnel client as a Tauri externalBin
# (sidecar) so `tauri build` signs it with our Developer ID + hardened
# runtime and notarization covers it. A binary shipped INSIDE the
# notarized app (and re-staged by our own process at runtime) runs
# cleanly on a fresh HOST machine with ZERO manual setup — no
# `brew install frpc`, and no Gatekeeper quarantine block (which is what
# bites a network-downloaded binary).
#
# frp (fatedier/frp) is licensed under Apache-2.0:
#   https://github.com/fatedier/frp/blob/master/LICENSE
# We redistribute the unmodified `frpc` client binary under that license.
#
# Tauri's externalBin convention requires the file be suffixed with the
# Rust target triple, e.g. `frpc-aarch64-apple-darwin`. The bundler
# strips the suffix when copying into Contents/MacOS/ (-> just `frpc`).
#
# Source: a known-good frpc binary already present at ~/.k2so/bin/frpc.
# We COPY it into src-tauri/binaries/ (which is gitignored — a ~14MB
# binary must NOT bloat the repo). On a fresh build machine, drop a
# working frpc at ~/.k2so/bin/frpc (or set FRPC_SRC) before building.
set -euo pipefail

# Resolve the Rust host target triple. Default to aarch64-apple-darwin
# (Apple Silicon, what we ship for). Allow override for x86_64 builds.
TRIPLE="${FRPC_TARGET_TRIPLE:-aarch64-apple-darwin}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST_DIR="$PROJECT_DIR/src-tauri/binaries"
DEST="$DEST_DIR/frpc-${TRIPLE}"

# Source binary: explicit override, else the staged copy under ~/.k2so.
FRPC_SRC="${FRPC_SRC:-$HOME/.k2so/bin/frpc}"

if [ ! -x "$FRPC_SRC" ]; then
    echo "fetch-frpc: FATAL — no executable frpc at $FRPC_SRC" >&2
    echo "  Set FRPC_SRC=/path/to/frpc, or place a working frpc client" >&2
    echo "  (fatedier/frp v0.61+, Apache-2.0) at ~/.k2so/bin/frpc." >&2
    exit 1
fi

mkdir -p "$DEST_DIR"
cp "$FRPC_SRC" "$DEST"
chmod +x "$DEST"

echo "fetch-frpc: staged $FRPC_SRC -> $DEST"
"$DEST" --version 2>/dev/null \
    && echo "fetch-frpc: sidecar reports version OK" \
    || echo "fetch-frpc: warning — could not read --version (continuing)"
