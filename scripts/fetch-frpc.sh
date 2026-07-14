#!/bin/bash
# fetch-frpc.sh — stage the `frpc` tunnel client as a Tauri externalBin
# (sidecar) so `tauri build` bundles it with the app. On macOS the
# bundler signs it with our Developer ID + hardened runtime and
# notarization covers it; a binary shipped INSIDE the notarized app
# (and re-staged by our own process at runtime) runs cleanly on a fresh
# HOST machine with ZERO manual setup — no `brew install frpc`, and no
# Gatekeeper quarantine block (which is what bites a network-downloaded
# binary). On Linux the deb/rpm/AppImage bundles install it next to the
# app binary the same way.
#
# frp (fatedier/frp) is licensed under Apache-2.0:
#   https://github.com/fatedier/frp/blob/master/LICENSE
# We redistribute the unmodified `frpc` client binary under that license.
#
# Tauri's externalBin convention requires the file be suffixed with the
# Rust target triple, e.g. `frpc-aarch64-apple-darwin`. The bundler
# strips the suffix when copying into the app (-> just `frpc`).
#
# Sources, in order of precedence:
#   1. FRPC_SRC=/path/to/frpc          — explicit override, any platform.
#   2. macOS triples: a known-good frpc already staged at ~/.k2/bin/frpc
#      (historical maintainer-box flow, unchanged).
#   3. Linux triples: download the pinned frp release archive straight
#      from GitHub, verify it against the release's sha256 checksum
#      manifest, and extract `frpc`.
#
# The staged binary lands in src-tauri/binaries/ (gitignored — a ~14MB
# binary must NOT bloat the repo).
set -euo pipefail

# Pinned frp release for the download path. MUST match the version of
# the maintainer-staged macOS binary reports 0.61.1 via `frpc -v`) so
# every platform ships the same frp.
FRP_VERSION="0.61.1"

# Resolve the Rust target triple. Explicit FRPC_TARGET_TRIPLE wins;
# otherwise detect the native macOS or Linux host architecture.
TRIPLE="${FRPC_TARGET_TRIPLE:-}"
if [ -z "$TRIPLE" ]; then
    case "$(uname -s)" in
        Darwin)
            case "$(uname -m)" in
                x86_64 | amd64)  TRIPLE="x86_64-apple-darwin" ;;
                aarch64 | arm64) TRIPLE="aarch64-apple-darwin" ;;
                *)
                    echo "fetch-frpc: FATAL — unsupported macOS arch $(uname -m)" >&2
                    echo "  Set FRPC_TARGET_TRIPLE + FRPC_SRC explicitly." >&2
                    exit 1
                    ;;
            esac
            ;;
        Linux)
            case "$(uname -m)" in
                x86_64)          TRIPLE="x86_64-unknown-linux-gnu" ;;
                aarch64 | arm64) TRIPLE="aarch64-unknown-linux-gnu" ;;
                *)
                    echo "fetch-frpc: FATAL — unsupported Linux arch $(uname -m)" >&2
                    echo "  Set FRPC_TARGET_TRIPLE + FRPC_SRC explicitly." >&2
                    exit 1
                    ;;
            esac
            ;;
        *)
            echo "fetch-frpc: FATAL — unsupported OS $(uname -s)" >&2
            echo "  Set FRPC_TARGET_TRIPLE + FRPC_SRC explicitly." >&2
            exit 1
            ;;
    esac
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST_DIR="$PROJECT_DIR/src-tauri/binaries"
DEST="$DEST_DIR/frpc-${TRIPLE}"
mkdir -p "$DEST_DIR"

# ── Path A: copy a local frpc (explicit FRPC_SRC, or the macOS default) ──
stage_from_local() {
    local src="$1"
    if [ ! -x "$src" ]; then
        echo "fetch-frpc: FATAL — no executable frpc at $src" >&2
        echo "  Set FRPC_SRC=/path/to/frpc, or place a working frpc client" >&2
        echo "  (fatedier/frp v0.61+, Apache-2.0) at ~/.k2/bin/frpc." >&2
        exit 1
    fi
    cp "$src" "$DEST"
    chmod +x "$DEST"
    echo "fetch-frpc: staged $src -> $DEST"
}

# ── Path B: download the pinned frp release from GitHub (Linux) ────────
stage_from_github() {
    local frp_arch
    case "$TRIPLE" in
        x86_64-unknown-linux-gnu)  frp_arch="amd64" ;;
        aarch64-unknown-linux-gnu) frp_arch="arm64" ;;
        *)
            echo "fetch-frpc: FATAL — no frp release mapping for triple $TRIPLE" >&2
            echo "  Set FRPC_SRC=/path/to/frpc to stage a local binary instead." >&2
            exit 1
            ;;
    esac

    # Short-circuit: already staged at the pinned version? Skip the
    # network round-trip (beforeBuildCommand runs this on every build).
    if [ -x "$DEST" ]; then
        local have
        have="$("$DEST" -v 2>/dev/null || true)"
        if [ "$have" = "$FRP_VERSION" ]; then
            echo "fetch-frpc: $DEST already staged at frp v$FRP_VERSION — nothing to do"
            return 0
        fi
    fi

    local base="https://github.com/fatedier/frp/releases/download/v${FRP_VERSION}"
    local dirname="frp_${FRP_VERSION}_linux_${frp_arch}"
    local archive="${dirname}.tar.gz"
    local tmp
    tmp="$(mktemp -d)"
    # NB: expand now — `local tmp` is out of scope when the EXIT trap runs.
    # shellcheck disable=SC2064
    trap "rm -rf '$tmp'" EXIT

    echo "fetch-frpc: downloading $base/$archive"
    curl -fsSL --retry 3 -o "$tmp/$archive" "$base/$archive"
    curl -fsSL --retry 3 -o "$tmp/frp_sha256_checksums.txt" "$base/frp_sha256_checksums.txt"

    # Verify the archive against the release's checksum manifest.
    (
        cd "$tmp"
        grep " ${archive}\$" frp_sha256_checksums.txt > checksum.expected
        if command -v sha256sum >/dev/null 2>&1; then
            sha256sum -c checksum.expected
        else
            shasum -a 256 -c checksum.expected # macOS fallback
        fi
    )

    tar -xzf "$tmp/$archive" -C "$tmp" "${dirname}/frpc"

    # Size sanity: a real frpc is ~10-15MB; anything tiny is an error
    # page / truncated download that checksum failure should have
    # caught, but belt-and-suspenders before we bless it as a sidecar.
    local size
    size=$(wc -c < "$tmp/${dirname}/frpc")
    if [ "$size" -lt 4000000 ]; then
        echo "fetch-frpc: FATAL — extracted frpc is only ${size} bytes; refusing" >&2
        exit 1
    fi

    install -m 0755 "$tmp/${dirname}/frpc" "$DEST"
    echo "fetch-frpc: staged frp v$FRP_VERSION (${frp_arch}) -> $DEST (${size} bytes)"
}

if [ -n "${FRPC_SRC:-}" ]; then
    stage_from_local "$FRPC_SRC"
else
    case "$TRIPLE" in
        *-apple-darwin) stage_from_local "$HOME/.k2/bin/frpc" ;;
        *-linux-gnu)    stage_from_github ;;
        *)
            echo "fetch-frpc: FATAL — no source strategy for triple $TRIPLE (set FRPC_SRC)" >&2
            exit 1
            ;;
    esac
fi

"$DEST" --version 2>/dev/null \
    && echo "fetch-frpc: sidecar reports version OK" \
    || echo "fetch-frpc: warning — could not read --version (continuing)"
