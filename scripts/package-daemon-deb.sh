#!/bin/bash
# package-daemon-deb.sh — build the standalone k2-daemon and package it
# as a Debian .deb for Linux hosts (headless servers, the K2 Cloud
# fleet, and Linux desktops that want the daemon without the app).
#
# Layout:
#   /usr/bin/k2-daemon                          — the daemon binary
#   /usr/lib/systemd/user/k2-daemon.service     — systemd USER unit
#
# The unit mirrors the macOS launchd agent (dev.k2.daemon, see
# crates/k2-core/src/wake.rs `DaemonPlist::canonical`): plain
# `k2-daemon` with no args, start on login (WantedBy=default.target ≙
# RunAtLoad), restart on crash (Restart=always ≙ KeepAlive), stdout /
# stderr appended to ~/.k2/daemon.stdout.log / daemon.stderr.log.
#
# Deliberately NOT auto-enabled on install — the user (or provisioning
# script) opts in:
#   systemctl --user daemon-reload
#   systemctl --user enable --now k2-daemon
# (On headless boxes run `loginctl enable-linger <user>` first so the
# user manager exists without an interactive login.)
#
# Run from any repo checkout on a Linux box:
#   scripts/package-daemon-deb.sh
# Output: dist/k2-daemon_<version>_<arch>.deb
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

if [ "$(uname -s)" != "Linux" ]; then
    echo "package-daemon-deb: FATAL — .deb packaging must run on a Linux host" >&2
    exit 1
fi
for tool in cargo dpkg-deb dpkg; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "package-daemon-deb: FATAL — missing tool: $tool" >&2
        exit 1
    }
done

# Version from Cargo metadata (no jq dependency: cargo pkgid prints
# `path+file:///…#k2-daemon@0.40.30`). Arch from dpkg itself.
VERSION="$(cargo pkgid -p k2-daemon | sed 's/.*[@#]//')"
ARCH="$(dpkg --print-architecture)"
case "$VERSION" in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *)
        echo "package-daemon-deb: FATAL — could not parse k2-daemon version (got '$VERSION')" >&2
        exit 1
        ;;
esac

# Pre-packaged Gmail OAuth must be baked at compile time (option_env!).
# shellcheck source=scripts/require-mail-oauth-build-env.sh
. "$(cd "$(dirname "$0")" && pwd)/require-mail-oauth-build-env.sh"
# Source repo .env if present (same keys as release.sh).
_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ -f "$_ROOT/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    . "$_ROOT/.env"
    set +a
fi
require_mail_oauth_build_env

echo "package-daemon-deb: building k2-daemon v$VERSION ($ARCH, release)"
cargo build --release -p k2-daemon --bin k2-daemon
BIN="${CARGO_TARGET_DIR:-target}/release/k2-daemon"
[ -x "$BIN" ] || { echo "package-daemon-deb: FATAL — $BIN missing after build" >&2; exit 1; }
assert_daemon_oauth_not_placeholder "$BIN"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
PKGROOT="$STAGE/k2-daemon_${VERSION}_${ARCH}"

install -D -m 0755 "$BIN" "$PKGROOT/usr/bin/k2-daemon"
install -d "$PKGROOT/usr/lib/systemd/user"
cat > "$PKGROOT/usr/lib/systemd/user/k2-daemon.service" <<'UNIT'
# k2-daemon systemd USER unit — Linux twin of the macOS launchd agent
# dev.k2.daemon (RunAtLoad=true, KeepAlive=true, logs under ~/.k2/).
#
# Enable per-user, after installing the k2-daemon package:
#   systemctl --user daemon-reload
#   systemctl --user enable --now k2-daemon
# Headless boxes need a lingering user manager first:
#   loginctl enable-linger <user>
[Unit]
Description=K2 daemon — AI workspace orchestration backend
Documentation=https://k2.dev
After=network.target

[Service]
Type=simple
ExecStartPre=/bin/sh -c 'mkdir -p "%h/.k2"'
ExecStart=/usr/bin/k2-daemon
Restart=always
RestartSec=2
StandardOutput=append:%h/.k2/daemon.stdout.log
StandardError=append:%h/.k2/daemon.stderr.log

[Install]
WantedBy=default.target
UNIT
chmod 0644 "$PKGROOT/usr/lib/systemd/user/k2-daemon.service"

INSTALLED_SIZE=$(( ($(du -sb "$PKGROOT" | cut -f1) + 1023) / 1024 ))
install -d "$PKGROOT/DEBIAN"
cat > "$PKGROOT/DEBIAN/control" <<CONTROL
Package: k2-daemon
Version: $VERSION
Section: devel
Priority: optional
Architecture: $ARCH
Maintainer: Alakazam Labs <support@k2.dev>
Homepage: https://k2.dev
Installed-Size: $INSTALLED_SIZE
Description: K2 daemon - AI workspace orchestration backend
 Headless backend for K2 by Alakazam Labs. Owns workspaces, terminal
 PTYs, agents, and the local HTTP/WebSocket API that the K2 app, CLI,
 and mobile companion connect to.
 .
 Not started automatically. Enable the bundled systemd user unit:
   systemctl --user daemon-reload
   systemctl --user enable --now k2-daemon
 On headless machines run 'loginctl enable-linger <user>' first.
CONTROL

OUT_DIR="$PROJECT_DIR/dist"
mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/k2-daemon_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKGROOT" "$OUT"
echo "package-daemon-deb: built $OUT"
dpkg-deb --info "$OUT"
