#!/bin/sh
# Repo checks: formatting, lints, tests, licences, no internal hostnames, names a Windows checkout
# can hold, no host's private forms, no kit dependency, and Board's credit wherever it's owed. The
# bridge (bridge/, its own workspace) is formatted, linted and tested here too: it needs the kit's
# checkout beside this repo's (bridge/README.md).
set -eu
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo fmt --manifest-path fuzz/Cargo.toml -- --check
cargo fmt --manifest-path bridge/Cargo.toml -- --check
cargo clippy -q --locked --all-targets -- -D warnings
cargo test -q --locked --all-targets
cargo clippy -q --locked --all-targets --manifest-path bridge/Cargo.toml -- -D warnings
cargo test -q --locked --all-targets --manifest-path bridge/Cargo.toml
./scripts/check-licences.sh
sh scripts/test-licences.sh
sh scripts/check-private.sh
sh scripts/test-windows-names.sh
sh scripts/check-windows-names.sh
sh scripts/test-host-forms.sh
sh scripts/check-host-forms.sh
sh scripts/test-no-kit.sh
sh scripts/check-no-kit.sh
sh scripts/test-credit.sh
sh scripts/check-credit.sh
echo "repo checks: ok"
