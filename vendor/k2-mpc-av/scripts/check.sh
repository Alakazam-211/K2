#!/bin/sh
# Repo checks: formatting, lints, the docs with warnings denied, tests (the planted bugs too),
# licences at their locked versions, the licence files, no internal hostnames, no internal labels
# in public text.
set -eu
cd "$(dirname "$0")/.."
sh scripts/check-targets.sh
sh scripts/test-check-targets.sh
cargo fmt -p k2-mpc-av -- --check
cargo clippy -q --locked -p k2-mpc-av --all-targets -- -D warnings
cargo clippy -q --locked -p k2-mpc-av --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -q --locked --no-deps -p k2-mpc-av
cargo test -q --locked -p k2-mpc-av --all-targets
cargo test -q --locked -p k2-mpc-av --features mutants --test mutants
sh scripts/check-licences.sh
sh scripts/oss-check.sh
sh scripts/check-private.sh
sh scripts/check-labels.sh
echo "repo checks: ok"
