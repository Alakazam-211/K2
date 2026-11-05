#!/bin/sh
# TEST ONLY. Build K2's Tauri --voice template into a real app (stock Tauri
# 2.12+ from crates.io) and check the webview: WebRTC, the E2EE transform,
# an AudioWorklet with WebAssembly under the README's CSP, IndexedDB that
# survives a restart, call audio that keeps flowing in a hidden window, and
# (with a mic) the mic for the App's origin only.
#
# macOS, on a compute node (no screen, no mic; the mic checks are info):
#   k2 compute run --os macos --sha <commit> --cpus 6 --mem 7G \
#     --cache tauri-voice-check -- sh tests/app-template/tauri-voice-check/run.sh
# On a person's Mac with a mic (Rust installed): K2_CHECK_MIC=1 sh run.sh,
# and click Allow on macOS's microphone prompt within 30 s of run 1 starting.
# The mic checks are then required. To ask again: tccutil reset Microphone.
#
# Steps: plutil-lint the plists; build; sign ad hoc with the hardened runtime
# and the template's entitlements; run twice with backgroundThrottling
# "disabled" (both must pass, and run 2 must read run 1's IndexedDB stamp);
# run once with it left off (the control: shows what the setting changes).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/src-tauri"
export CARGO_TARGET_DIR="${K2_JOB_CACHE:-$here/src-tauri}/target"

if [ "$(uname -s)" = Darwin ]; then
  plutil -lint Info.plist Entitlements.plist
fi
cargo build
echo "tauri: $(cargo tree -i tauri --depth 0 2>/dev/null | head -1)"
echo "wry:   $(cargo tree -i wry --depth 0 2>/dev/null | head -1)"
bin="$CARGO_TARGET_DIR/debug/k2-voice-check"

if [ "$(uname -s)" = Darwin ]; then
  sw_vers
  codesign --force --sign - --options runtime --entitlements Entitlements.plist "$bin"
  codesign -dv "$bin" 2>&1 | grep -E 'flags=.*runtime' || { echo "FAIL: not signed with the hardened runtime"; exit 1; }
  codesign -d --entitlements - "$bin" 2>&1 | grep -E 'audio-input|camera'
  echo "embedded Info.plist:"
  launchctl plist __TEXT,__info_plist "$bin" 2>/dev/null | grep -E 'NSMicrophone|NSCamera|NSAppSleep' || {
    echo "FAIL: no Info.plist embedded in the binary"
    exit 1
  }
fi

export K2_CHECK_PORT="${K2_JOB_PORT_BASE:-47123}"
stamp1="s$(date +%s)a"
stamp2="s$(date +%s)b"
echo "== run 1: backgroundThrottling disabled"
run() { # <log> <env...>: run the app, show its output, stop on a failure
  log=$1; shift
  rc=0
  env "$@" "$bin" > "$log" 2>&1 || rc=$?
  cat "$log"
  [ "$rc" = 0 ] || { echo "FAIL: exit $rc"; exit 1; }
}
run run1.log K2_CHECK_STAMP=$stamp1
echo "== run 2: the same, after a restart"
run run2.log K2_CHECK_STAMP=$stamp2
grep '^K2CHECK ' run2.log | grep -q "\"previous\":\"$stamp1\"" || {
  echo "FAIL: run 2 did not read run 1's IndexedDB stamp ($stamp1)"
  exit 1
}
echo "PASS  IndexedDB kept across a restart ($stamp1)"
echo "== control: backgroundThrottling left off (may fail; informational)"
rc=0
K2_CHECK_THROTTLE=default K2_CHECK_TIMEOUT=120 "$bin" > run3.log 2>&1 || rc=$?
cat run3.log
echo "control run exit $rc"
rm -f run1.log run2.log run3.log
echo "ALL REQUIRED CHECKS PASSED"
