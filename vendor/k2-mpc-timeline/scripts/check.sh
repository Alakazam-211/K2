#!/bin/sh
# Repo checks: formatting, lints, tests (the planted bugs too), licences at their locked
# versions, the licence files, no internal hostnames, no internal labels in public text. The
# public corpus tests are #[ignore]d here: scripts/public-corpus.sh fetches the files and runs
# them. The fuzz crate is checked when the tree has one.
set -eu
cd "$(dirname "$0")/.."
# every test target, feature and #[ignore] accounted for, and the scan's self-test (text only;
# CHECK_TARGETS_LANDING=1, the mode for landing a change, refuses pending rows)
sh scripts/check-targets.sh
sh scripts/test-check-targets.sh
# Our package only: `--all` would also format the kit, a path dependency with its own formatting.
cargo fmt -p k2-mpc-timeline -- --check
if [ -f fuzz/Cargo.toml ]; then cargo fmt --manifest-path fuzz/Cargo.toml -- --check; fi
cargo clippy -q --locked -p k2-mpc-timeline --all-targets -- -D warnings
cargo clippy -q --locked -p k2-mpc-timeline --all-targets --all-features -- -D warnings
if [ -f fuzz/Cargo.toml ]; then
  cargo clippy -q --locked --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings
fi
cargo test -q --locked -p k2-mpc-timeline --all-targets
# the planted bugs, each caught by its row
cargo test -q --locked -p k2-mpc-timeline --features mutants --test mutants
sh scripts/check-licences.sh
sh scripts/oss-check.sh
sh scripts/check-private.sh
sh scripts/check-labels.sh
echo "repo checks: ok"
