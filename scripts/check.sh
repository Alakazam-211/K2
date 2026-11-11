#!/bin/sh
# The kit's local checks (the node suite, tools/tests/full.sh, runs the tests): formatting of
# the kit's own crates (never vendor/), clippy, the licence check, and the evidence tools: their
# self-test and the check that every count in docs/evidence/*.md cites a run that backs it
# (tools/evidence/README.md; not in a public cut, which has no run records), and that no tracked file names an internal build host
# (tools/check-private.sh), that every path can be checked out on Windows
# (tools/check-windows-names.sh and its self-test), that the engine's test-only features reach
# no build (not in a public cut), and that the repo carries its licence, MIT OR Apache-2.0 (K-OSS-3:
# tools/oss-check.sh and its self-test). Run from anywhere; on a shared machine, through its cargo gate.
set -eu
cd "$(dirname "$0")/.."
cargo fmt -p k2-mpc-base -p k2-mpc-spec-undo -- --check
cargo clippy -p k2-mpc-base -p k2-mpc-spec-undo --all-targets -- -D warnings
cargo clippy -p k2-mpc-spec-undo --all-targets --features mutants -- -D warnings
./scripts/check-licences.sh
sh tools/evidence/test.sh
sh tools/test-check-private.sh
sh tools/check-private.sh
sh tools/test-windows-names.sh
sh tools/check-windows-names.sh
sh tools/test-oss-check.sh
sh tools/oss-check.sh
echo "kit checks: ok"
