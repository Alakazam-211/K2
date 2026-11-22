#!/usr/bin/env bash
# Browser underlay (0.46.17): compile check of the desktop app on a Mac
# node with warnings as errors on the files the underlay touches, a build
# of the app binary, then the underlay's unit tests. A K2 Compute job only
# (cargo never runs on a person's Mac):
#
#   k2 compute run --sha <sha> --os macos --cpus 7 --mem 8G --stall-timeout off \
#       --needs rust@1.99.1,cmake@3 --cache k2-underlay-mac -- bash scripts/ci/underlay-mac-check.sh
#
# The frontend, frpc and the DeepFilterNet weights are stubbed when absent:
# this checks Rust, it doesn't make a bundle.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
[ "$(uname -s)" = Darwin ] || { echo "underlay-mac-check: macOS only" >&2; exit 2; }
[ -n "${K2_JOB_CPUS:-}" ] || { echo "underlay-mac-check: not in a K2 Compute job" >&2; exit 2; }
mkdir -p out/renderer src-tauri/binaries src-tauri/dfn3
[ -f out/renderer/index.html ] || echo '<!doctype html><title>stub</title>' > out/renderer/index.html
frpc=src-tauri/binaries/frpc-aarch64-apple-darwin
[ -e "$frpc" ] || { printf '#!/bin/sh\n' > "$frpc"; chmod +x "$frpc"; }
ls src-tauri/dfn3/* >/dev/null 2>&1 || echo stub > src-tauri/dfn3/stub.txt
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"
if [ -n "${K2_JOB_CACHE:-}" ]; then export CARGO_TARGET_DIR="$K2_JOB_CACHE/target"; fi
touched='^[^ ]*(third_party/wry/src/(lib\.rs|wkwebview/(mod\.rs|underlay/|class/wry_web_view\.rs))|src-tauri/src/(lib\.rs|commands/(mod\.rs|browser_page_layers\.rs|browser_webviews\.rs))):[0-9]+:[0-9]+: (warning|error)'
rc=0
echo "underlay-mac-check: cargo check -p k2 (lib + tests)"
cargo check -p k2 --locked --lib --tests --message-format short > check.log 2>&1 || rc=$?
grep -E "$touched" check.log > touched.log || true
tail -5 check.log
if [ "$rc" != 0 ]; then
    echo "underlay-mac-check: FAILED: cargo check exit $rc"
    grep -E ': (error|warning)' check.log | head -80
    exit "$rc"
fi
if [ -s touched.log ]; then
    echo "underlay-mac-check: FAILED: warnings in the files the underlay touches:"
    cat touched.log
    exit 1
fi
echo "underlay-mac-check: check clean"
echo "underlay-mac-check: cargo build -p k2 (the app binary)"
if ! cargo build -p k2 --locked --bin k2 --message-format short > build.log 2>&1; then
    tail -40 build.log
    echo "underlay-mac-check: FAILED: build"
    exit 1
fi
tail -2 build.log
echo "underlay-mac-check: tests"
set +e
cargo test -p k2 --locked --lib -- browser_page_layers wry_underlay_logic > test.log 2>&1
trc=$?
set -e
tail -40 test.log
[ "$trc" = 0 ] || { echo "underlay-mac-check: FAILED: tests exit $trc"; exit 1; }
grep -qE '^test result: ok\. [1-9][0-9]* passed' test.log || { echo "underlay-mac-check: FAILED: no tests ran"; exit 1; }
echo "underlay-mac-check: clippy (the touched files)"
cargo clippy -p k2 --locked --lib --tests --message-format short > clippy.log 2>&1 || { tail -30 clippy.log; echo "underlay-mac-check: FAILED: clippy"; exit 1; }
grep -E "$touched" clippy.log > touched-clippy.log || true
if [ -s touched-clippy.log ]; then
    echo "underlay-mac-check: FAILED: clippy warnings in the files the underlay touches:"
    cat touched-clippy.log
    exit 1
fi
echo "underlay-mac-check: OK"
