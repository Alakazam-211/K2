#!/bin/bash
# windows-nsis-build.sh — build the Windows NSIS installer on the sticky
# Windows box (home LAN `k2-win` for now; later k2-dev-web cloud Windows).
#
# Design (swap-ready):
#   - SSH host is ONLY the alias / env (never a baked-in LAN IP).
#   - On-box layout is fixed: C:\k2\K2 tree + C:\k2\K2-target out dir.
#   - Retarget home → cloud by changing ~/.ssh/config HostName for k2-win
#     (or K2_WINDOWS_SSH_HOST). This script does not need to change.
#
# Called by scripts/release.sh after the GitHub release exists (or
# standalone). Produces:
#   dist-windows/K2_<ver>_x64-setup.exe
#   dist-windows/SHA256SUMS-windows-x86_64.txt
#
# Env:
#   K2_WINDOWS_SSH_HOST     default: k2-win
#   K2_WINDOWS_REMOTE_DIR   default: C:/k2/K2   (scp style)
#   K2_WINDOWS_TARGET_DIR   default: C:/k2/K2-target
#   K2_WINDOWS_VERSION      required unless argv[1] is the version
#   K2_SKIP_WINDOWS_NSIS=1  skip with loud warning (exit 0)
#
# Prerequisites on the sticky box: VS 2022 Build Tools, rustup MSVC, bun,
# CMake, Ninja, LLVM/libclang, NSIS, OpenSSH. See wiki
# "Ops - Windows Build Box".
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-${K2_WINDOWS_VERSION:-}}"
HOST="${K2_WINDOWS_SSH_HOST:-k2-win}"
REMOTE_DIR="${K2_WINDOWS_REMOTE_DIR:-C:/k2/K2}"
TARGET_DIR="${K2_WINDOWS_TARGET_DIR:-C:/k2/K2-target}"
OUT_DIR="${PROJECT_DIR}/dist-windows"
BAT_SRC="${PROJECT_DIR}/scripts/windows/build-nsis-release.bat"

if [ "${K2_SKIP_WINDOWS_NSIS:-0}" = "1" ]; then
    echo "⚠⚠ WINDOWS NSIS BUILD SKIPPED (K2_SKIP_WINDOWS_NSIS=1) ⚠⚠"
    echo "  No K2_*_x64-setup.exe will be attached this run."
    exit 0
fi

if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>" >&2
    echo "  e.g. $0 0.40.94" >&2
    exit 1
fi

if [ ! -f "$BAT_SRC" ]; then
    echo "ERROR: missing $BAT_SRC" >&2
    exit 1
fi

INSTALLER_NAME="K2_${VERSION}_x64-setup.exe"
REMOTE_INSTALLER="${TARGET_DIR}/release/bundle/nsis/${INSTALLER_NAME}"

echo "Windows NSIS build on ${HOST} (tree ${REMOTE_DIR}, target ${TARGET_DIR})..."
echo "  version=${VERSION}  installer=${INSTALLER_NAME}"

if ! ssh -o BatchMode=yes -o ConnectTimeout=15 "$HOST" "echo ok" >/dev/null; then
    echo "ERROR: cannot ssh to ${HOST}. Is the sticky Windows box up?" >&2
    echo "  Host alias contract: only 'k2-win' (or K2_WINDOWS_SSH_HOST) — no LAN IPs in scripts." >&2
    exit 1
fi

STAGE="$(mktemp -d -t k2-win-nsis)"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

echo "  Packaging tree for sync..."
COPYFILE_DISABLE=1 tar -C "$PROJECT_DIR" \
    --exclude='./target' \
    --exclude='./target-*' \
    --exclude='./node_modules' \
    --exclude='./.git' \
    --exclude='./dist' \
    --exclude='./dist-windows' \
    --exclude='./out' \
    --exclude='./.bmr-wt' \
    --exclude='./.k2' \
    -czf "$STAGE/tree.tgz" .

echo "  Uploading tree + build bat..."
# Drop on C:\k2 (always present on the sticky box contract).
scp -o BatchMode=yes -o ConnectTimeout=60 \
    "$STAGE/tree.tgz" \
    "$BAT_SRC" \
    "${HOST}:C:/k2/"

REMOTE_DIR_CMD="$(printf '%s' "$REMOTE_DIR" | tr '/' '\\')"
TARGET_DIR_CMD="$(printf '%s' "$TARGET_DIR" | tr '/' '\\')"

echo "  Extracting on ${HOST}..."
ssh -o BatchMode=yes -o ServerAliveInterval=30 "$HOST" \
    "cmd /c \"if not exist ${REMOTE_DIR_CMD} mkdir ${REMOTE_DIR_CMD} && tar -xzf C:\\k2\\tree.tgz -C ${REMOTE_DIR_CMD} && copy /Y C:\\k2\\build-nsis-release.bat C:\\k2\\build-nsis-release.bat\""

echo "  Building NSIS on ${HOST} (several minutes; ServerAlive keeps SSH up)..."
set +e
ssh -o BatchMode=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=240 "$HOST" \
    "cmd /c \"set CARGO_TARGET_DIR=${TARGET_DIR_CMD}&& set K2_WIN_TREE=${REMOTE_DIR_CMD}&& C:\\k2\\build-nsis-release.bat\"" \
    2>&1 | tee "$STAGE/build.log"
BUILD_RC=${PIPESTATUS[0]}
set -e

if [ "$BUILD_RC" -ne 0 ] || ! grep -q "ALL_OK" "$STAGE/build.log"; then
    echo "ERROR: Windows NSIS build failed (rc=${BUILD_RC}). Tail of log:" >&2
    tail -50 "$STAGE/build.log" >&2 || true
    exit 1
fi

mkdir -p "$OUT_DIR"
echo "  Fetching installer..."
scp -o BatchMode=yes \
    "${HOST}:${REMOTE_INSTALLER}" \
    "${OUT_DIR}/${INSTALLER_NAME}"

if [ ! -f "${OUT_DIR}/${INSTALLER_NAME}" ]; then
    echo "ERROR: installer missing after scp: ${OUT_DIR}/${INSTALLER_NAME}" >&2
    exit 1
fi

(
    cd "$OUT_DIR"
    shasum -a 256 "$INSTALLER_NAME" | tee SHA256SUMS-windows-x86_64.txt
)

echo "  Windows NSIS ready: ${OUT_DIR}/${INSTALLER_NAME}"
