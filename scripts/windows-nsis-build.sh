#!/bin/bash
# windows-nsis-build.sh — build the Windows NSIS installer (Alpha) on the
# Windows build box (K2-WIN-AX41, ssh alias `k2-win-ax41`) and bring it back
# here, to the Mac that runs release.sh.
#
# Flow: the release tag's commits go to the box as a git bundle (the box
# has no GitHub credentials and must never get any) → checkout of the tag
# in D:\src\K2 → frpc staged from this Mac (fetch-frpc.sh, pinned +
# checksum-verified) → scripts/windows/build-nsis-release.ps1 builds the
# web UI, a release k2-daemon (externalBin) and the NSIS installer into
# D:\target → scp back to dist-windows/. Nothing is signed on the box: no
# signing keys live on a build machine. release.sh then minisigns the
# installer here for latest.json (scripts/windows-patch-latest-json.sh).
#
# Called by scripts/release.sh Step 9.5 after the tag exists (or
# standalone). Produces:
#   dist-windows/K2_<ver>_x64-setup.exe
#   dist-windows/SHA256SUMS-windows-x86_64.txt
#
# Env:
#   K2_WINDOWS_SSH_HOST     default: k2-win-ax41 (an ~/.ssh/config alias
#                           with the box's host key pinned; no IPs here)
#   K2_WINDOWS_REMOTE_DIR   default: D:\src\K2   (a git checkout)
#   K2_WINDOWS_TARGET_DIR   default: D:\target
#   K2_WINDOWS_REF          default: v<version>  (the release tag)
#   K2_SKIP_WINDOWS_NSIS=1  skip with a loud warning (exit 0)
#
# Prerequisites on the box: Git, VS 2022 Build Tools, Rust (MSVC) under
# D:\cargo + D:\rustup, bun, Node, CMake, Ninja, LLVM, OpenSSH. See wiki
# "Ops - Windows Build Box".
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-${K2_WINDOWS_VERSION:-}}"
HOST="${K2_WINDOWS_SSH_HOST:-k2-win-ax41}"
REMOTE_DIR="${K2_WINDOWS_REMOTE_DIR:-D:\\src\\K2}"
TARGET_DIR="${K2_WINDOWS_TARGET_DIR:-D:\\target}"
OUT_DIR="${PROJECT_DIR}/dist-windows"
TRIPLE="x86_64-pc-windows-msvc"

if [ "${K2_SKIP_WINDOWS_NSIS:-0}" = "1" ]; then
    echo "⚠⚠ WINDOWS NSIS BUILD SKIPPED (K2_SKIP_WINDOWS_NSIS=1) ⚠⚠"
    echo "  No K2_*_x64-setup.exe will be attached this run."
    exit 0
fi

if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>" >&2
    echo "  e.g. $0 0.46.6" >&2
    exit 1
fi
REF="${K2_WINDOWS_REF:-v${VERSION}}"
INSTALLER_NAME="K2_${VERSION}_x64-setup.exe"
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=20 -o ServerAliveInterval=30 -o ServerAliveCountMax=240)
SCP=(scp -q -o BatchMode=yes -o ConnectTimeout=60)

cd "$PROJECT_DIR"
COMMIT="$(git rev-parse --verify --quiet "${REF}^{commit}")" || {
    echo "ERROR: ${REF} is not a commit here (release.sh creates the tag at Step 8.9)." >&2
    exit 1
}
# The tree we ship must be this version (Tauri names the installer from it).
REF_VER="$(git show "${COMMIT}:package.json" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
if [ "$REF_VER" != "$VERSION" ]; then
    echo "ERROR: ${REF} has package.json version ${REF_VER:-?}, expected ${VERSION}." >&2
    exit 1
fi
echo "Windows NSIS build on ${HOST}: ${REF} (${COMMIT:0:9}), tree ${REMOTE_DIR}, target ${TARGET_DIR}"

if ! "${SSH[@]}" "$HOST" "echo ok" >/dev/null; then
    echo "ERROR: cannot ssh to ${HOST}. Is K2-WIN-AX41 up, and is the alias in ~/.ssh/config?" >&2
    exit 1
fi

STAGE="$(mktemp -d -t k2-win-nsis)"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

# ── Code: a git bundle of the release commit ──
# Only what the box lacks: everything since the merge base with the box's
# own HEAD when this Mac has that commit, else the whole history.
BOX_HEAD="$("${SSH[@]}" "$HOST" "git -C ${REMOTE_DIR} rev-parse HEAD" 2>/dev/null | tr -d '\r' || true)"
BASE=""
if [ -n "$BOX_HEAD" ] && git cat-file -e "${BOX_HEAD}^{commit}" 2>/dev/null; then
    BASE="$(git merge-base "$COMMIT" "$BOX_HEAD" 2>/dev/null || true)"
fi
BUNDLE_REF="refs/k2-release/windows"
git update-ref "$BUNDLE_REF" "$COMMIT"
if [ -n "$BASE" ] && [ "$BASE" != "$COMMIT" ]; then
    git bundle create "$STAGE/release.bundle" "$BUNDLE_REF" "^${BASE}" >/dev/null 2>&1
else
    git bundle create "$STAGE/release.bundle" "$BUNDLE_REF" >/dev/null 2>&1
fi
git update-ref -d "$BUNDLE_REF"
echo "  bundle: $(du -h "$STAGE/release.bundle" | cut -f1) (base ${BASE:0:9})"
"${SCP[@]}" "$STAGE/release.bundle" "${HOST}:D:/tmp/k2-release.bundle"
# Detached at the release commit; untracked files go, ignored ones
# (node_modules, src-tauri/binaries) stay so bun and the sidecars are reused.
"${SSH[@]}" "$HOST" "cmd /c \"git -C ${REMOTE_DIR} fetch -q D:\\tmp\\k2-release.bundle +${BUNDLE_REF}:${BUNDLE_REF} && git -C ${REMOTE_DIR} checkout -q -f --detach ${COMMIT} && git -C ${REMOTE_DIR} clean -qfd && del /Q D:\\tmp\\k2-release.bundle\""
BOX_AT="$("${SSH[@]}" "$HOST" "git -C ${REMOTE_DIR} rev-parse HEAD" | tr -d '\r')"
if [ "$BOX_AT" != "$COMMIT" ]; then
    echo "ERROR: the box is at ${BOX_AT}, expected ${COMMIT}." >&2
    exit 1
fi
echo "  box checkout at ${COMMIT:0:9}."

# ── frpc: pinned + checksum-verified, fetched on this Mac ──
FRPC_LOCAL="$PROJECT_DIR/src-tauri/binaries/frpc-${TRIPLE}.exe"
if ! FRPC_TARGET_TRIPLE="$TRIPLE" "$PROJECT_DIR/scripts/fetch-frpc.sh" >"$STAGE/frpc-fetch.log" 2>&1; then
    echo "ERROR: could not stage the Windows frpc (fetch-frpc.sh):" >&2
    tail -20 "$STAGE/frpc-fetch.log" >&2 || true
    exit 1
fi
[ -f "$FRPC_LOCAL" ] || { echo "ERROR: fetch-frpc.sh left no ${FRPC_LOCAL}" >&2; exit 1; }
REMOTE_DIR_SCP="$(printf '%s' "$REMOTE_DIR" | tr '\\' '/')"
"${SCP[@]}" "$FRPC_LOCAL" "${HOST}:${REMOTE_DIR_SCP}/src-tauri/binaries/frpc-${TRIPLE}.exe"
echo "  frpc sidecar staged."

# ── Build ──
echo "  Building on ${HOST} (several minutes)..."
set +e
"${SSH[@]}" "$HOST" \
    "powershell -NoProfile -ExecutionPolicy Bypass -File ${REMOTE_DIR}\\scripts\\windows\\build-nsis-release.ps1 -Version ${VERSION} -Tree ${REMOTE_DIR} -Target ${TARGET_DIR}" \
    2>&1 | tee "$STAGE/build.log"
BUILD_RC=${PIPESTATUS[0]}
set -e
if [ "$BUILD_RC" -ne 0 ] || ! grep -q "ALL_OK" "$STAGE/build.log"; then
    echo "ERROR: Windows NSIS build failed (rc=${BUILD_RC}). Tail of log:" >&2
    tail -40 "$STAGE/build.log" >&2 || true
    exit 1
fi

# ── Bring it back ──
mkdir -p "$OUT_DIR"
rm -f "${OUT_DIR}/${INSTALLER_NAME}"
TARGET_DIR_SCP="$(printf '%s' "$TARGET_DIR" | tr '\\' '/')"
"${SCP[@]}" "${HOST}:${TARGET_DIR_SCP}/release/bundle/nsis/${INSTALLER_NAME}" "${OUT_DIR}/${INSTALLER_NAME}"
[ -s "${OUT_DIR}/${INSTALLER_NAME}" ] || { echo "ERROR: installer missing after scp: ${OUT_DIR}/${INSTALLER_NAME}" >&2; exit 1; }
(
    cd "$OUT_DIR"
    shasum -a 256 "$INSTALLER_NAME" | tee SHA256SUMS-windows-x86_64.txt
)
echo "  Windows NSIS ready (Alpha, unsigned): ${OUT_DIR}/${INSTALLER_NAME}"
