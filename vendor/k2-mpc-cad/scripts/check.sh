#!/bin/sh
# Repo checks: formatting, lints, the docs with warnings denied, tests (the planted bugs too),
# licences at their locked versions, the licence files, no internal hostnames, no internal labels
# in public text.
set -eu
cd "$(dirname "$0")/.."
# every test target, feature and #[ignore] accounted for, and the scan's self-test (text only;
# CHECK_TARGETS_LANDING=1, the mode for landing a change, refuses pending rows)
sh scripts/check-targets.sh
sh scripts/test-check-targets.sh
# Our package only: `--all` would also format the kit, a path dependency with its own formatting.
cargo fmt -p k2-mpc-cad-model -p k2-mpc-dxf -p k2-mpc-dwg -p k2-mpc-cad-measure -- --check
cargo fmt --manifest-path fuzz/Cargo.toml -- --check
cargo clippy -q --locked -p k2-mpc-cad-model -p k2-mpc-dxf -p k2-mpc-dwg -p k2-mpc-cad-measure --all-targets -- -D warnings
cargo clippy -q --locked -p k2-mpc-cad-model -p k2-mpc-dxf -p k2-mpc-dwg -p k2-mpc-cad-measure --all-targets --all-features -- -D warnings
cargo clippy -q --locked --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -q --locked --no-deps -p k2-mpc-cad-model -p k2-mpc-dxf -p k2-mpc-dwg -p k2-mpc-cad-measure
cargo test -q --locked -p k2-mpc-cad-model -p k2-mpc-dxf -p k2-mpc-dwg -p k2-mpc-cad-measure --all-targets
# the planted bugs, each caught by its row
cargo test -q --locked -p k2-mpc-dxf --features mutants --test mutants
cargo test -q --locked -p k2-mpc-dwg --features mutants --test mutants
sh scripts/check-licences.sh
sh scripts/oss-check.sh
sh scripts/check-private.sh
sh scripts/check-labels.sh
echo "repo checks: ok"
