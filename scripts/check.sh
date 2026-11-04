#!/bin/sh
# Pack checks: formatting (this crate), lints, tests, licences, and the engine-free rule from the kit's pack template.
# Expects k2-mpc-base checked out beside this repo.
set -eu
cd "$(dirname "$0")/.."
cargo fmt -p k2-mpc-markdown -- --check   # own crate only: path deps (the kit, its vendored model) keep their own style
cargo clippy -q --locked --all-targets -- -D warnings
cargo test -q --locked --all-targets
./scripts/check-licences.sh
sh scripts/check-private.sh
sh scripts/test-windows-names.sh
sh scripts/check-windows-names.sh
../k2-mpc-base/scripts/pack-checks/engine-free.sh .
echo "pack checks: ok"
