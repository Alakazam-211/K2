#!/usr/bin/env bash
# deskd-sck-live.sh — D5's ScreenCaptureKit live test (prd-remote-desktop-tab-v1),
# run on a Mac as the logged-in console user (over SSH is fine: the app is
# started with `open`, so it runs in the GUI session as its own responsible
# process, the way K2 Screen does).
#
#   1. Build the example on that Mac's compute node and fetch it:
#        k2 compute run k2n1-m4 --sha <c> --os macos --cpus 7 --mem 8G --needs rust@stable \
#            --artifact target/release/examples/sck_live \
#            -- cargo build --release -p k2-deskd --example sck_live --locked
#        k2 compute fetch <job> --out <dir>
#   2. Copy it to the Mac and run this there:
#        deskd-sck-live.sh <sck_live binary> [seconds] [--request]
#
# It bundles the binary as ~/k2-sck-live/K2 Screen Live Test.app (bundle id
# dev.k2.test.screen-live, LSUIElement, ad hoc signed), opens it, waits, and
# prints the results JSON. Exit 3 = Screen Recording isn't allowed for it:
# someone at the Mac turns on "K2 Screen Live Test" in System Settings →
# Privacy & Security → Screen & System Audio Recording (with --request, the
# app shows macOS's prompt first). An ad hoc signature changes with every
# build, so a new binary needs the switch again.
#
# Nothing here touches ~/.k2, a keychain or a running K2.
set -euo pipefail

bin="${1:?usage: $0 <sck_live binary> [seconds] [--request]}"
seconds="${2:-8}"
request="${3:-}"
dir="$HOME/k2-sck-live"
app="$dir/K2 Screen Live Test.app"
out="$dir/result.json"

mkdir -p "$app/Contents/MacOS"
cp "$bin" "$app/Contents/MacOS/sck_live"
chmod 755 "$app/Contents/MacOS/sck_live"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.k2.test.screen-live</string>
  <key>CFBundleName</key><string>K2 Screen Live Test</string>
  <key>CFBundleExecutable</key><string>sck_live</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>LSUIElement</key><true/>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
</dict>
</plist>
PLIST
codesign --force --sign - --identifier dev.k2.test.screen-live "$app"
rm -f "$out"

args=(--out "$out" --seconds "$seconds")
[ "$request" = "--request" ] && args+=(--request)
rc=0
open -n -W --stdout "$dir/stdout.txt" --stderr "$dir/stderr.txt" "$app" --args "${args[@]}" || rc=$?
echo "== open: exit $rc"
cat "$dir/stdout.txt" 2>/dev/null || true
tail -20 "$dir/stderr.txt" 2>/dev/null || true
if [ ! -f "$out" ]; then
    echo "FAIL: no results file (the app didn't run or crashed)"
    exit 1
fi
cat "$out"
state="$(sed -n 's/.*"state": *"\([a-z_]*\)".*/\1/p' "$out" | head -1)"
case "$state" in
    needs_permission) echo "NEEDS PERMISSION: allow \"K2 Screen Live Test\" under Screen & System Audio Recording"; exit 3 ;;
    ran) grep -q '^LIVE PASS' "$dir/stdout.txt" && { echo "deskd-sck-live: PASS"; exit 0; } ;;
esac
echo "deskd-sck-live: FAIL"
exit 1
