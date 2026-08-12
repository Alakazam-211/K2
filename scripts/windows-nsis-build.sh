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

# Fail early if the tree we're about to ship doesn't match the release tag
# (this is what produced a K2_0.40.93_* setup while release expected 0.40.94).
PKG_VER="$(
    python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['version'])" \
        "$PROJECT_DIR/package.json" 2>/dev/null \
    || node -e "console.log(require(process.argv[1]).version)" \
        "$PROJECT_DIR/package.json" 2>/dev/null \
    || sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$PROJECT_DIR/package.json" | head -1
)"
if [ -z "$PKG_VER" ]; then
    echo "ERROR: could not read package.json version from $PROJECT_DIR" >&2
    exit 1
fi
if [ "$PKG_VER" != "$VERSION" ]; then
    echo "ERROR: package.json version is ${PKG_VER}, but build requested ${VERSION}." >&2
    echo "  release.sh must bump versions before Step 9.5; do not build Windows from a stale tree." >&2
    exit 1
fi
echo "  package.json version matches ${VERSION}."

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
NSIS_DIR_CMD="${TARGET_DIR_CMD}\\release\\bundle\\nsis"

echo "  Extracting on ${HOST}..."
# Wipe the remote tree first so a prior checkout cannot shadow a partial
# extract (0.40.95 manual build produced K2_0.40.94_* because C:\k2\K2
# still had the old Cargo.toml). bat stays at C:\k2\ (uploaded above).
# Prefer cmd `rmdir /S /Q` over PowerShell Remove-Item: on the sticky box
# Remove-Item often left a half-tree (file locks / long paths) so tar merged
# into stale 0.40.95 sources and the version gate failed (0.40.96 cut).
ssh -o BatchMode=yes -o ServerAliveInterval=30 "$HOST" \
    "cmd /c \"taskkill /F /IM cargo.exe /T >nul 2>&1 & taskkill /F /IM rustc.exe /T >nul 2>&1 & taskkill /F /IM bun.exe /T >nul 2>&1 & taskkill /F /IM node.exe /T >nul 2>&1 & rmdir /S /Q ${REMOTE_DIR_CMD} >nul 2>&1 & mkdir ${REMOTE_DIR_CMD} & tar -xzf C:\\k2\\tree.tgz -C ${REMOTE_DIR} & if not exist ${REMOTE_DIR_CMD}\\package.json exit /b 1\""

# Fail loud if the remote tree is still the wrong product version
# (tauri/nsis name the installer from Cargo.toml, not our argv).
# Pull files over scp so we parse on the Mac (avoids ssh/cmd quoting hell).
echo "  Verifying remote package.json + src-tauri/Cargo.toml == ${VERSION}..."
STAGE_V="$(mktemp -d -t k2-win-ver)"
scp -o BatchMode=yes -o ConnectTimeout=20 \
    "${HOST}:${REMOTE_DIR}/package.json" \
    "${HOST}:${REMOTE_DIR}/src-tauri/Cargo.toml" \
    "$STAGE_V/" >/dev/null
REMOTE_PKG="$(python3 -c "import json; print(json.load(open('$STAGE_V/package.json'))['version'])")"
REMOTE_CARGO="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$STAGE_V/Cargo.toml" | head -1)"
rm -rf "$STAGE_V"
if [ "$REMOTE_PKG" != "$VERSION" ] || [ "$REMOTE_CARGO" != "$VERSION" ]; then
    echo "ERROR: after extract, remote versions are package.json=${REMOTE_PKG} Cargo.toml=${REMOTE_CARGO}, expected ${VERSION}." >&2
    echo "  Tree sync failed — refusing to build a mis-versioned installer." >&2
    exit 1
fi
echo "  remote versions OK (package.json + Cargo.toml = ${VERSION})."

# frpc Windows sidecar is gitignored / not in the tarball. Stage from:
#   1) prior release dir on the sticky box, or
#   2) local fetch into a temp path + scp (fetch-frpc.sh).
FRPC_SIDE="src-tauri/binaries/frpc-x86_64-pc-windows-msvc.exe"
echo "  Ensuring Windows frpc sidecar (${FRPC_SIDE})..."
if ssh -o BatchMode=yes -o ConnectTimeout=15 "$HOST" \
    "cmd /c if exist ${REMOTE_DIR_CMD}\\src-tauri\\binaries\\frpc-x86_64-pc-windows-msvc.exe (exit 0) else (exit 1)"; then
    echo "  frpc sidecar already present on remote."
elif ssh -o BatchMode=yes -o ConnectTimeout=15 "$HOST" \
    "cmd /c if exist ${TARGET_DIR_CMD}\\release\\frpc.exe (copy /Y ${TARGET_DIR_CMD}\\release\\frpc.exe ${REMOTE_DIR_CMD}\\src-tauri\\binaries\\frpc-x86_64-pc-windows-msvc.exe >nul & exit 0) else (exit 1)"; then
    echo "  staged frpc from ${TARGET_DIR}/release/frpc.exe"
else
    echo "  fetching frpc for x86_64-pc-windows-msvc..."
    FRPC_TARGET_TRIPLE=x86_64-pc-windows-msvc \
        "$PROJECT_DIR/scripts/fetch-frpc.sh" >>"$STAGE/frpc-fetch.log" 2>&1 || {
        echo "ERROR: could not stage Windows frpc (see fetch-frpc / sticky-box release frpc.exe)." >&2
        tail -20 "$STAGE/frpc-fetch.log" 2>/dev/null || true
        exit 1
    }
    scp -o BatchMode=yes -o ConnectTimeout=60 \
        "$PROJECT_DIR/$FRPC_SIDE" \
        "${HOST}:${REMOTE_DIR}/${FRPC_SIDE}"
    echo "  scp'd frpc sidecar to remote."
fi

# Drop stale NSIS names so a leftover K2_0.40.93_* can't satisfy a loose check.
echo "  Clearing prior NSIS outputs under ${NSIS_DIR_CMD}..."
ssh -o BatchMode=yes -o ConnectTimeout=15 "$HOST" \
    "cmd /c \"if exist ${NSIS_DIR_CMD}\\K2_*_x64-setup.exe del /F /Q ${NSIS_DIR_CMD}\\K2_*_x64-setup.exe\""

echo "  Building NSIS on ${HOST} (several minutes; ServerAlive keeps SSH up)..."
set +e
ssh -o BatchMode=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=240 "$HOST" \
    "cmd /c \"set CARGO_TARGET_DIR=${TARGET_DIR_CMD}&& set K2_WIN_TREE=${REMOTE_DIR_CMD}&& set K2_WIN_VERSION=${VERSION}&& C:\\k2\\build-nsis-release.bat\"" \
    2>&1 | tee "$STAGE/build.log"
BUILD_RC=${PIPESTATUS[0]}
set -e

if [ "$BUILD_RC" -ne 0 ] || ! grep -q "ALL_OK" "$STAGE/build.log"; then
    echo "ERROR: Windows NSIS build failed (rc=${BUILD_RC}). Tail of log:" >&2
    tail -50 "$STAGE/build.log" >&2 || true
    exit 1
fi

mkdir -p "$OUT_DIR"
echo "  Fetching installer ${INSTALLER_NAME}..."
if ! scp -o BatchMode=yes \
    "${HOST}:${REMOTE_INSTALLER}" \
    "${OUT_DIR}/${INSTALLER_NAME}"; then
    echo "ERROR: scp failed for exact name ${INSTALLER_NAME}. Remote NSIS dir:" >&2
    ssh -o BatchMode=yes -o ConnectTimeout=15 "$HOST" \
        "cmd /c dir ${NSIS_DIR_CMD}" >&2 || true
    exit 1
fi

if [ ! -f "${OUT_DIR}/${INSTALLER_NAME}" ]; then
    echo "ERROR: installer missing after scp: ${OUT_DIR}/${INSTALLER_NAME}" >&2
    exit 1
fi

(
    cd "$OUT_DIR"
    shasum -a 256 "$INSTALLER_NAME" | tee SHA256SUMS-windows-x86_64.txt
)

echo "  Windows NSIS ready: ${OUT_DIR}/${INSTALLER_NAME}"
