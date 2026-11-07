#!/usr/bin/env bash
# build-k2-signer.sh — a reproducible release build of k2-signer (macOS
# arm64), so that two machines building one commit get the same sha256
# (install-k2-signer.sh --expect-sha256, security review M6).
#
#   bash scripts/k2sign/build-k2-signer.sh          # prints "<sha256>  <binary>"
#
# Every machine-specific path is remapped for rustc and for the C code
# aws-lc builds: the checkout, CARGO_HOME, RUSTUP_HOME, the target dir and
# $HOME, each as written and as resolved (/var is /private/var on macOS). The
# release profile already strips symbols and uses one codegen unit with LTO.
#
# Two builds match when they use the same Rust (rust-toolchain.toml) and the
# same Xcode Command Line Tools: the binary records the SDK version and the
# linker's version (LC_BUILD_VERSION). This script prints both on stderr;
# build the independent copy on a K2 Compute Mac node that prints the same:
#   k2 compute run --os macos --sha <commit> --needs rust@<pin> -- bash scripts/k2sign/build-k2-signer.sh
set -euo pipefail
TREE="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$TREE"
TD="${CARGO_TARGET_DIR:-$TREE/target}/k2-signer-repro"
mkdir -p "$TD"
RUSTFLAGS="" CFLAGS=""
add() { # <path> <to>: remap the path as written and as resolved
    local p="$1" to="$2" r
    [ -n "$p" ] || return 0
    r="$(cd "$p" 2>/dev/null && pwd -P || echo "$p")"
    for x in "$p" "$r"; do
        RUSTFLAGS="$RUSTFLAGS --remap-path-prefix=$x=$to"
        CFLAGS="$CFLAGS -ffile-prefix-map=$x=$to"
    done
}
# rustc: the last matching --remap-path-prefix wins, so the widest ($HOME) comes first.
add "$HOME" /h
add "$TREE" /k2
add "${CARGO_HOME:-$HOME/.cargo}" /cargo
add "${RUSTUP_HOME:-$HOME/.rustup}" /rustup
add "$TD" /target
export RUSTFLAGS="${RUSTFLAGS# }" CFLAGS="${CFLAGS# }"
export CXXFLAGS="$CFLAGS" ASMFLAGS="$CFLAGS"
export CARGO_INCREMENTAL=0 SOURCE_DATE_EPOCH=0 ZERO_AR_DATE=1
{
    echo "rust: $(rustc -V)"
    echo "sdk: $(xcrun --show-sdk-version 2>/dev/null || echo unknown)"
    echo "ld: $(ld -v 2>&1 | head -1)"
} >&2
cargo build --release --locked -p k2-signer --target-dir "$TD" >&2
shasum -a 256 "$TD/release/k2-signer"
