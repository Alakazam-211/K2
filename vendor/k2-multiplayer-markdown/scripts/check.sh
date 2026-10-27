#!/bin/sh
# Pack checks: tests, licences, and the engine-free rule from the kit's pack template.
# Expects k2-multiplayer-base checked out beside this repo.
set -eu
cd "$(dirname "$0")/.."
cargo test -q --all-targets
./scripts/check-licences.sh
../k2-multiplayer-base/scripts/pack-checks/engine-free.sh .
echo "pack checks: ok"
