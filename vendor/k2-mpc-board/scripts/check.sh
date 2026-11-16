#!/bin/sh
# Repo checks: every test target accounted for, formatting, lints, tests, the doc examples, the
# fuzz crate's build under the fuzzing cfg, licences, no internal hostnames, names a Windows
# checkout can hold, no host's private forms, no kit dependency, and Board's credit wherever it's
# owed. The bridge (bridge/, its own workspace) is formatted, linted and tested here too: it needs
# the kit's checkout beside this repo's (bridge/README.md).
set -eu
cd "$(dirname "$0")/.."
# every test target, feature and #[ignore] accounted for, and the scan's self-test (text only;
# CHECK_TARGETS_LANDING=1, the mode for landing a change, refuses pending rows)
sh scripts/check-targets.sh
sh scripts/test-check-targets.sh
cargo fmt --all -- --check
cargo fmt --manifest-path fuzz/Cargo.toml -- --check
cargo fmt --manifest-path bridge/Cargo.toml -- --check
cargo clippy -q --locked --all-targets -- -D warnings
cargo test -q --locked --all-targets
cargo test -q --locked --doc   # the doc examples: --all-targets leaves them out
# The fuzz crate's lib and targets under the fuzzing cfg, at its locked dependencies: type errors
# in code built only for fuzzing. A target dir of its own: the flags differ.
RUSTFLAGS="--cfg fuzzing" CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}/fuzz-check" cargo check -q --locked --manifest-path fuzz/Cargo.toml --lib --bins
cargo clippy -q --locked --all-targets --manifest-path bridge/Cargo.toml -- -D warnings
cargo test -q --locked --all-targets --manifest-path bridge/Cargo.toml
./scripts/check-licences.sh
sh scripts/test-licences.sh
sh scripts/check-private.sh
sh scripts/check-labels.sh
sh scripts/test-windows-names.sh
sh scripts/check-windows-names.sh
sh scripts/test-host-forms.sh
sh scripts/check-host-forms.sh
sh scripts/test-no-kit.sh
sh scripts/check-no-kit.sh
sh scripts/test-credit.sh
sh scripts/check-credit.sh
echo "repo checks: ok"
