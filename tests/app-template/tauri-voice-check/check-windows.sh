#!/bin/sh
# TEST ONLY. Type-check the --voice check app for Windows (WebView2) from a
# Linux or macOS node: proves k2_voice.rs and the window settings compile
# against Tauri's Windows backend. It doesn't run anything; the WebView2 run
# is the manual Windows step in k2 study tauri-voice.
#   k2 compute run --os linux --sha <commit> --cpus 6 --mem 8G --needs rust@stable \
#     --cache tauri-voice-check-win -- sh tests/app-template/tauri-voice-check/check-windows.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/src-tauri"
export CARGO_TARGET_DIR="${K2_JOB_CACHE:-$here/src-tauri}/target"
if ! rustup target add x86_64-pc-windows-msvc; then
  # A node's toolchain is read-only: a private copy in the job cache.
  [ -n "${K2_JOB_CACHE:-}" ] || { echo "FAIL: can't add the Windows target"; exit 1; }
  if [ ! -d "$K2_JOB_CACHE/rustup" ]; then
    cp -a "$(rustup show home)" "$K2_JOB_CACHE/rustup"
  fi
  export RUSTUP_HOME="$K2_JOB_CACHE/rustup"
  rustup target add x86_64-pc-windows-msvc
fi
cargo check --target x86_64-pc-windows-msvc
echo "tauri: $(cargo tree --target x86_64-pc-windows-msvc -i tauri --depth 0 2>/dev/null | head -1)"
echo "webview2: $(cargo tree --target x86_64-pc-windows-msvc -i webview2-com --depth 0 2>/dev/null | head -1)"
echo "WINDOWS CHECK OK"
