#!/bin/sh
# Pack checks: format, lints, tests, licences, and the engine-free rule from the kit's pack
# template. Expects k2-mpc-base checked out beside this repo.
set -eu
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy -q --locked --all-targets -- -D warnings
cargo test -q --locked --all-targets
./scripts/check-licences.sh
../k2-mpc-base/scripts/pack-checks/engine-free.sh .
echo "pack checks: ok"
