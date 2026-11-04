#!/bin/sh
# Repo checks: formatting, lints, tests, licences, no internal hostnames, names a Windows checkout
# can hold, no host's private forms, no kit dependency.
set -eu
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy -q --locked --all-targets -- -D warnings
cargo test -q --locked --all-targets
./scripts/check-licences.sh
sh scripts/check-private.sh
sh scripts/test-windows-names.sh
sh scripts/check-windows-names.sh
sh scripts/test-host-forms.sh
sh scripts/check-host-forms.sh
sh scripts/check-no-kit.sh
echo "repo checks: ok"
