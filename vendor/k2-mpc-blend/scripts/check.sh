#!/bin/sh
# Repo checks: formatting, lints, the docs (no rustdoc warning), tests (the mutant rows too),
# licences and advisories at their locked versions, the licence files, no internal hostnames.
# The public corpus, saved-file and round-trip rows and the measurements are #[ignore]d here
# (scripts/public-corpus.sh, PROVENANCE.md, scripts/roundtrip.sh and scripts/measure.sh run
# them). When a change lands (CHECK_TARGETS_LANDING=1), the program's headless round trip must
# run too (scripts/roundtrip.sh, the gate's round-trip part), on the build node that holds its
# folder (BLEND_ROUNDTRIP_DIR). The fuzz crate is checked when the tree has one.
set -eu
cd "$(dirname "$0")/.."
# every test target, feature and #[ignore] accounted for, and the scan's self-test (text only;
# CHECK_TARGETS_LANDING=1, the mode for landing a change, refuses pending rows)
sh scripts/check-targets.sh
sh scripts/test-check-targets.sh
# the round-trip part's self-test (text and shell only: a stand-in cargo, the program never runs)
sh scripts/test-roundtrip.sh
# Our package only: `--all` would also format the kit, a path dependency with its own formatting.
cargo fmt -p k2-mpc-blend -- --check
if [ -f fuzz/Cargo.toml ]; then cargo fmt --manifest-path fuzz/Cargo.toml -- --check; fi
cargo clippy -q --locked -p k2-mpc-blend --all-targets -- -D warnings
cargo clippy -q --locked -p k2-mpc-blend --all-targets --all-features -- -D warnings
if [ -f fuzz/Cargo.toml ]; then
  cargo clippy -q --locked --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings
fi
# the docs build with no warning, as a host builds them (default features) and with every
# feature: every rustdoc link resolves to one item, nothing public undocumented
RUSTDOCFLAGS="-D warnings" cargo doc -q --locked --no-deps -p k2-mpc-blend
RUSTDOCFLAGS="-D warnings" cargo doc -q --locked --no-deps -p k2-mpc-blend --all-features
cargo test -q --locked -p k2-mpc-blend --all-targets
# the planted bugs, each caught by its own leg
cargo test -q --locked -p k2-mpc-blend --features mutants --test mutants
sh scripts/check-licences.sh
# advisories: every locked crate against the RustSec database (cargo-deny; deny.toml)
sh scripts/check-advisories.sh
sh scripts/oss-check.sh
sh scripts/check-private.sh
sh scripts/check-labels.sh
# the label check's commit-ref rule, planted in a copy of the tree (text and shell only)
sh scripts/test-check-labels.sh
# the gate's round-trip part: required when a change lands, so a landing can't skip it
if [ "${CHECK_TARGETS_LANDING:-0}" = 1 ]; then
  sh scripts/roundtrip.sh
else
  echo "round trip: not run here (it runs when a change lands: CHECK_TARGETS_LANDING=1, BLEND_ROUNDTRIP_DIR)"
fi
echo "repo checks: ok"
