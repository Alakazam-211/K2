#!/bin/bash
# Dry test for release.sh's k2-ptyd checks (prd-k2-ptyd-v1 P2/P59):
# k2_daemon_helper_verify_tarball_ptyd passes on an update bundle that
# carries an executable nested k2-ptyd and fails loudly on one without it
# (or with it not executable), and release.sh calls both checks. Runs
# anywhere (no codesign/notarytool needed).
set -euo pipefail
here="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../../scripts/macos-daemon-helper-app.sh
. "$here/scripts/macos-daemon-helper-app.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mac="$work/ok/K2.app/Contents/Helpers/$K2_DAEMON_HELPER_APP/Contents/MacOS"
mkdir -p "$mac"
printf 'x' > "$mac/k2-daemon"; printf 'x' > "$mac/$K2_DAEMON_HELPER_PTYD"
chmod 0755 "$mac/k2-daemon" "$mac/$K2_DAEMON_HELPER_PTYD"
tar -czf "$work/ok.tar.gz" -C "$work/ok" K2.app
k2_daemon_helper_verify_tarball_ptyd "$work/ok.tar.gz" >/dev/null || { echo "FAIL: a good bundle was refused"; exit 1; }

chmod 0644 "$mac/$K2_DAEMON_HELPER_PTYD"
tar -czf "$work/noexec.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_ptyd "$work/noexec.tar.gz" 2>/dev/null; then echo "FAIL: a non-executable k2-ptyd passed"; exit 1; fi

rm "$mac/$K2_DAEMON_HELPER_PTYD"
tar -czf "$work/missing.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_ptyd "$work/missing.tar.gz" 2>/dev/null; then echo "FAIL: a bundle without k2-ptyd passed"; exit 1; fi

grep -q '^k2_daemon_helper_verify_shipped_ptyd "K2.app"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the notarized app's k2-ptyd"; exit 1; }
grep -q '^k2_daemon_helper_verify_tarball_ptyd "target/release/bundle/macos/K2.app.tar.gz"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the update bundle's k2-ptyd"; exit 1; }
echo "ok: release k2-ptyd checks"
