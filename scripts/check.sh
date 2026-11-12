#!/bin/sh
# Repo checks: formatting, the kit's carrier-row check (a pack that keeps a carrier calls the
# kit's byte-exact judge in a test), the kit's engine-free check (no CRDT engine dependency or
# import), lints (the mutants feature too), tests (read, refusals, round trips, merge, verbs,
# the merge matrix, doc), the mutants, the fuzz targets' build, the licence gate at the locks'
# exact versions, the licence files, and no internal hostnames. Run
# on a build node, with the kit and the package layer checked out beside this repo (README,
# "Tests and checks"). The external checkers are their own step (docs/checkers.md). The fuzz
# crate is checked when the tree has one.
#
# The script's exit is the checks' exit: every step runs under `set -e`, no step's output goes
# through a pipe, and the last line, `repo checks: ok`, prints only when all of them passed.
# The private-names check needs its patterns, which live off the tree (scripts/check-private.sh):
# where none are set (a build node), it is skipped with a line that says so, and must be run
# where they are.
set -eu
cd "$(dirname "$0")/.."
cargo fmt -p k2-mpc-pptx -- --check
if [ -f fuzz/Cargo.toml ]; then cargo fmt --manifest-path fuzz/Cargo.toml -- --check; fi
# The kit's pack-template check (docs/PACK_TEMPLATE.md in k2-mpc-base): this pack keeps a
# carrier, so a test under tests/ must call `test_support::carrier_judge` over its corpus.
sh ../k2-mpc-base/scripts/pack-checks/carrier-row.sh .
# The kit's pack-template rule: a pack never depends on or imports a CRDT engine.
sh ../k2-mpc-base/scripts/pack-checks/engine-free.sh .
cargo clippy -q --locked -p k2-mpc-pptx --all-targets -- -D warnings
cargo clippy -q --locked -p k2-mpc-pptx --features mutants --all-targets -- -D warnings
cargo test -q --locked -p k2-mpc-pptx --all-targets
cargo test -q --locked -p k2-mpc-pptx --doc
if [ -f crates/pptx/tests/mutants.rs ]; then
  # --nocapture: each plant's line and the summary ("every mutant caught: true") are printed;
  # the exit is the test's own
  cargo test -q --locked -p k2-mpc-pptx --features mutants --test mutants -- --nocapture
fi
if [ -f fuzz/Cargo.toml ]; then
  cargo build -q --locked --manifest-path fuzz/Cargo.toml --bins
  echo "fuzz targets: built"
fi
sh scripts/check-licences.sh
sh scripts/oss-check.sh
if [ -n "${MPC_PRIVATE_PATTERNS:-}" ] || [ -n "${MPC_PRIVATE_PATTERNS_FILE:-}" ] \
  || [ -r "${HOME:-/nonexistent}/.config/k2-mpc/private-patterns.txt" ]; then
  sh scripts/check-private.sh
else
  echo "check-private: SKIPPED: no patterns here (they live off the tree); run scripts/check-private.sh where they are"
fi
echo "repo checks: ok"
