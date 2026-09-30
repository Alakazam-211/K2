#!/bin/bash
# Run the vendored tao unit tests (macOS sendEvent guard, event_fault.rs).
#
# `cargo test -p tao` refuses from the K2 workspace: tao is a path patch, not
# a member, and it has dev-dependencies. This builds a throwaway workspace
# whose only member is a symlink to third_party/tao, with the same objc2
# path patch and K2's Cargo.lock, so the tests compile against what ships.
#
# Usage:
#   ./scripts/test-vendored-tao.sh              # all tao lib tests
#   ./scripts/test-vendored-tao.sh event_fault  # a test-name filter
#
# Build output goes to target/vendored-tao-test (override: CARGO_TARGET_DIR).
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2-vendored-tao.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

ln -s "$PROJECT_DIR/third_party/tao" "$WORK/tao"
cp "$PROJECT_DIR/Cargo.lock" "$WORK/Cargo.lock"
cat >"$WORK/Cargo.toml" <<EOF
[workspace]
resolver = "2"
members = ["tao"]

[profile.dev]
opt-level = 1

[patch.crates-io]
objc2 = { path = "$PROJECT_DIR/third_party/objc2" }
EOF

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_DIR/target/vendored-tao-test}"
cd "$WORK"
cargo test -p tao --lib -- "$@"
