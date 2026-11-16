#!/bin/sh
# Repo checks: every test target accounted for, formatting (with each crate's own config; the
# vendored crate keeps upstream's), lints, tests, the doc examples, licences, no internal hostnames.
# The public corpus tests are #[ignore]d here (scripts/public-corpus.sh runs them), as is the
# counts-only corpus row (scripts/private-corpus.sh). The fuzz crate is checked when the tree has
# one (the main branch; a release tag leaves it out), and built under the fuzzing cfg.
set -eu
cd "$(dirname "$0")/.."
# every test target, feature and #[ignore] accounted for, and the scan's self-test (text only;
# CHECK_TARGETS_LANDING=1, the mode for landing a change, refuses pending rows)
sh scripts/check-targets.sh
sh scripts/test-check-targets.sh
# Our packages only: `--all` would also check the kit, a path dependency with its own formatting.
cargo fmt -p k2-mpc-psd -p photocraft-psd -- --check
if [ -f fuzz/Cargo.toml ]; then cargo fmt --manifest-path fuzz/Cargo.toml -- --check; fi
cargo clippy -q --locked -p k2-mpc-psd --all-targets -- -D warnings
cargo clippy -q --locked -p photocraft-psd --all-targets -- -D warnings
cargo test -q --locked --workspace --all-targets
cargo test -q --locked --workspace --doc   # the doc examples: --all-targets leaves them out
# The fuzz crate's lib and targets under the fuzzing cfg, at its locked dependencies: type errors
# in code built only for fuzzing. A target dir of its own: the flags differ.
if [ -f fuzz/Cargo.toml ]; then
  RUSTFLAGS="--cfg fuzzing" CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}/fuzz-check" cargo check -q --locked --manifest-path fuzz/Cargo.toml --lib --bins
fi
sh scripts/check-licences.sh
sh scripts/oss-check.sh
sh scripts/check-private.sh
sh scripts/check-labels.sh
echo "repo checks: ok"
