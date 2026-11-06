#!/bin/sh
# The publish gates for kessel-frame-ring and kessel-video on this machine:
# clippy (-D warnings) and the tests of both crates. On a Mac, also both
# package checks (cargo package reads git status, which works there).
# Usage, from the repository root:
#   sh crates/kessel-frame-ring/scripts/check-all.sh
set -eu
echo "== clippy"
cargo clippy --locked -p kessel-frame-ring -p kessel-video --all-targets -- -D warnings
echo "== tests"
cargo test --locked -p kessel-frame-ring -p kessel-video
if [ "$(uname -s)" = Darwin ]; then
    echo "== package: kessel-frame-ring"
    sh crates/kessel-frame-ring/scripts/check-package.sh
    echo "== package: kessel-video"
    sh crates/kessel-video/scripts/check-package.sh
fi
echo "== all gates pass"
