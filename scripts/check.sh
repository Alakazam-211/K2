#!/bin/sh
# Repo checks: formatting (with each crate's own config; the vendored crate keeps upstream's), lints,
# tests, licences, no internal hostnames. The public corpus test runs only with PSD_PUBLIC_CORPUS set.
# The fuzz crate is checked when the tree has one (the main branch; a release tag leaves it out).
set -eu
cd "$(dirname "$0")/.."
# Our packages only: `--all` would also check the kit, a path dependency with its own formatting.
cargo fmt -p k2-mpc-psd -p photocraft-psd -- --check
if [ -f fuzz/Cargo.toml ]; then cargo fmt --manifest-path fuzz/Cargo.toml -- --check; fi
cargo clippy -q --locked -p k2-mpc-psd --all-targets -- -D warnings
cargo clippy -q --locked -p photocraft-psd --all-targets -- -D warnings
cargo test -q --locked --workspace --all-targets
sh scripts/check-licences.sh
sh scripts/oss-check.sh
sh scripts/check-private.sh
echo "repo checks: ok"
