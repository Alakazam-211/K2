#!/bin/sh
# The kit's local checks (the node suite, tools/tests/full.sh, runs the tests): formatting of
# the kit's own crates (never vendor/), clippy, the licence check, and the evidence tools: their
# self-test and the check that every count in docs/evidence/*.md cites a run that backs it
# (tools/evidence/README.md), and that no tracked file names an internal build host
# (tools/check-private.sh). Run from anywhere; on a shared machine, through its cargo gate.
set -eu
cd "$(dirname "$0")/.."
cargo fmt -p k2-mpc-base -p k2-mpc-conformance -p k2-mpc-spec-undo -- --check
cargo clippy -p k2-mpc-base -p k2-mpc-conformance -p k2-mpc-spec-undo --all-targets -- -D warnings
cargo clippy -p k2-mpc-spec-undo --all-targets --features mutants -- -D warnings
./scripts/check-licences.sh
sh tools/evidence/test.sh
sh tools/check-private.sh
python3 tools/evidence/check.py
echo "kit checks: ok"
