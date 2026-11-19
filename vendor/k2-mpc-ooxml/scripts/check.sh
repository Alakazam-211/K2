#!/bin/sh
# Repo checks: formatting, lints (the mutants feature too), tests (unit, fixtures, refusals,
# merge rows, doc), the mutants, the licence gate at the locks' exact versions, the licence
# files, and no internal hostnames. Run on a build node. The xlsxwriter corpus test runs only
# with OOXML_XLSXWRITER_CORPUS set (scripts/fetch-corpus.sh fetches it). The fuzz crate is
# checked when the tree has one.
set -eu
cd "$(dirname "$0")/.."
cargo fmt -p k2-mpc-opc -- --check
if [ -f fuzz/Cargo.toml ]; then cargo fmt --manifest-path fuzz/Cargo.toml -- --check; fi
cargo clippy -q --locked -p k2-mpc-opc --all-targets -- -D warnings
cargo clippy -q --locked -p k2-mpc-opc --features mutants --all-targets -- -D warnings
cargo test -q --locked -p k2-mpc-opc --all-targets
cargo test -q --locked -p k2-mpc-opc --doc
cargo test -q --locked -p k2-mpc-opc --features mutants --test mutants
sh scripts/check-licences.sh
sh scripts/oss-check.sh
sh scripts/check-private.sh
sh scripts/check-labels.sh
echo "repo checks: ok"
