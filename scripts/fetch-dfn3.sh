#!/bin/bash
# fetch-dfn3.sh — stage the DeepFilterNet3 noise-removal weights the desktop
# app bundles (Sew 0.10.2 on). tauri-plugin-sew-voice's `dfn3` feature loads
# them from `<app resource dir>/dfn3/dfn3_ll.nnef.tar`; without them noise
# removal's Standard mode stays RNNoise (the plugin logs that once).
#
# The four files land in src-tauri/dfn3/ (gitignored, beside the committed
# README.md: the weights are 39 MB and must never be committed), and
# tauri.conf.json's bundle.resources (`dfn3/*`) ships them as `dfn3/…` in
# every app build:
#   dfn3/dfn3_ll.nnef.tar   the weights (MIT OR Apache-2.0, converted by
#                           Alakazam Labs; see dfn3/NOTICE)
#   dfn3/NOTICE             their notice (also in K2's NOTICE.md)
#   dfn3/LICENSE-MIT        the authors' MIT text
#   dfn3/LICENSE-APACHE     the authors' Apache-2.0 text
#
# Source: the public Sew release asset, checked twice before anything is
# staged: the asset's sha256, then the weights member's sha256 (both as
# crates/k2-root-proto/src/sew/dfn3.rs pins them; a test holds the two
# equal). A mismatch exits 1 and stages nothing.
#
# Idempotent and offline once cached: a staged set whose weights already
# hash right is left alone (no network), and the downloaded asset is kept in
# the cache dir, so a re-stage needs no network either.
#
#   scripts/fetch-dfn3.sh            stage (download only when needed)
#   scripts/fetch-dfn3.sh --check    change nothing; exit 0 if staged and
#                                    right, 1 if not (the build paths' gate)
#
# Every app build path runs it: tauri.conf.json's beforeBuildCommand (any
# `tauri build`, the Linux app leg included), scripts/lib/build-binaries.sh
# (dev-build.sh, build-app.sh, release.sh; it also checks the built bundle),
# scripts/windows-nsis-build.sh (stages here, copies to the Windows box), and
# `tauri dev` (package.json dev:dfn3: there a failed fetch only warns, and
# noise removal stays RNNoise).
#
# Env: DFN3_ASSET_SRC=/path/to/sew-dfn3-<v>.tar.gz  use this file instead of
#        downloading (a box without internet; it is still checked)
#      K2_DFN3_CACHE=<dir>  where the downloaded asset is kept
#        (default ~/.cache/k2-build/dfn3, outside every checkout, so all
#        worktrees share one download)
set -euo pipefail

DFN3_SEW_VERSION="0.10.3"
DFN3_ASSET="sew-dfn3-0.10.3.tar.gz"
DFN3_ASSET_SHA256="153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697"
DFN3_ASSET_BYTES="36353582"
DFN3_WEIGHTS_SHA256="b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af"
DFN3_URL="https://github.com/Alakazam-211/sew-releases/releases/download/v${DFN3_SEW_VERSION}/${DFN3_ASSET}"
# The four members, as the app ships them under its resource dir.
DFN3_FILES="dfn3/dfn3_ll.nnef.tar dfn3/NOTICE dfn3/LICENSE-MIT dfn3/LICENSE-APACHE"

MODE=stage
case "${1:-}" in
    "") ;;
    --check) MODE=check ;;
    *) echo "fetch-dfn3: unknown argument: $1" >&2; exit 2 ;;
esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# Staged as src-tauri/dfn3/<file>: a bundle resource path relative to
# src-tauri keeps its folders, so the app gets <resources>/dfn3/<file>.
STAGE_PARENT="$PROJECT_DIR/src-tauri"
STAGE="$STAGE_PARENT/dfn3"
CACHE="${K2_DFN3_CACHE:-$HOME/.cache/k2-build/dfn3}"
CACHED="$CACHE/$DFN3_ASSET"

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# Staged and right: all four files, and the weights hash to the pin.
staged_ok() {
    local f
    for f in $DFN3_FILES; do
        [ -f "$STAGE_PARENT/$f" ] || return 1
    done
    [ "$(sha256_of "$STAGE/dfn3_ll.nnef.tar")" = "$DFN3_WEIGHTS_SHA256" ]
}

if [ "$MODE" = check ]; then
    if staged_ok; then
        echo "fetch-dfn3: staged (Sew $DFN3_SEW_VERSION, weights sha256 ${DFN3_WEIGHTS_SHA256:0:12}…)"
        exit 0
    fi
    echo "fetch-dfn3: NOT staged in $STAGE (run scripts/fetch-dfn3.sh)" >&2
    exit 1
fi

if staged_ok; then
    echo "fetch-dfn3: already staged (Sew $DFN3_SEW_VERSION)"
    exit 0
fi

# The asset: an explicit file, else the cache, else (stage mode) download.
asset=""
if [ -n "${DFN3_ASSET_SRC:-}" ]; then
    asset="$DFN3_ASSET_SRC"
    [ -f "$asset" ] || { echo "fetch-dfn3: FATAL — DFN3_ASSET_SRC=$asset is not a file" >&2; exit 1; }
elif [ -f "$CACHED" ] && [ "$(sha256_of "$CACHED")" = "$DFN3_ASSET_SHA256" ]; then
    asset="$CACHED"
else
    mkdir -p "$CACHE"
    part="$CACHED.part.$$"
    trap 'rm -f "$part"' EXIT
    echo "fetch-dfn3: downloading $DFN3_URL"
    curl -fL --retry 3 --retry-delay 2 --connect-timeout 20 -o "$part" "$DFN3_URL"
    asset="$part"
fi

# Check 1: the asset itself (size, then sha256).
bytes="$(wc -c < "$asset" | tr -d ' ')"
if [ "$bytes" != "$DFN3_ASSET_BYTES" ]; then
    echo "fetch-dfn3: FATAL — $asset is $bytes bytes, the pin says $DFN3_ASSET_BYTES" >&2
    exit 1
fi
got="$(sha256_of "$asset")"
if [ "$got" != "$DFN3_ASSET_SHA256" ]; then
    echo "fetch-dfn3: FATAL — $DFN3_ASSET sha256 $got, the pin says $DFN3_ASSET_SHA256" >&2
    exit 1
fi
if [ -n "${part:-}" ] && [ "$asset" = "$part" ]; then
    mv -f "$part" "$CACHED"
    asset="$CACHED"
fi

# Check 2: unpack into a fresh dir beside the stage; only the four members,
# each a regular file; then the weights' own sha256. Only then move them in,
# one file at a time (the stage's committed README.md stays).
tmp="$(mktemp -d "$STAGE_PARENT/.dfn3.XXXXXX")"
cleanup_tmp() { rm -rf "$tmp"; }
trap cleanup_tmp EXIT
members="$(tar -tzf "$asset" | sed 's#/$##' | sort | tr '\n' ' ')"
want="$(printf '%s\n' dfn3 $DFN3_FILES | sort | tr '\n' ' ')"
if [ "$members" != "$want" ]; then
    echo "fetch-dfn3: FATAL — $DFN3_ASSET holds [$members], want [$want]" >&2
    exit 1
fi
tar -xzf "$asset" -C "$tmp" $DFN3_FILES
for f in $DFN3_FILES; do
    if [ ! -f "$tmp/$f" ] || [ -L "$tmp/$f" ]; then
        echo "fetch-dfn3: FATAL — $f is not a regular file in $DFN3_ASSET" >&2
        exit 1
    fi
done
got="$(sha256_of "$tmp/dfn3/dfn3_ll.nnef.tar")"
if [ "$got" != "$DFN3_WEIGHTS_SHA256" ]; then
    echo "fetch-dfn3: FATAL — weights sha256 $got, the pin says $DFN3_WEIGHTS_SHA256" >&2
    exit 1
fi
chmod 0644 "$tmp"/dfn3/*
mkdir -p "$STAGE"
for f in $DFN3_FILES; do
    mv -f "$tmp/$f" "$STAGE_PARENT/$f"
done
echo "fetch-dfn3: staged Sew $DFN3_SEW_VERSION's DeepFilterNet3 weights in src-tauri/dfn3/"
