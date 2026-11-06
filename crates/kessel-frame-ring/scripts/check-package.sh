#!/bin/sh
# Package kessel-frame-ring as crates.io would (`cargo publish --dry-run`
# builds the package), then unpack the .crate outside the repository and run
# its tests and docs there, with every dependency from crates.io: what a user
# of the published crate gets. cargo package checks the git work tree, so run
# it from a clean checkout on a machine where git status works (a Mac).
# Usage, from the repository root:
#   sh crates/kessel-frame-ring/scripts/check-package.sh
set -eu
echo "== files in the package"
cargo package -p kessel-frame-ring --list
echo "== cargo publish --dry-run"
cargo publish -p kessel-frame-ring --dry-run
# publish keeps its .crate elsewhere; write it where `cargo package` does
# (already verified above).
cargo package -p kessel-frame-ring --no-verify
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' crates/kessel-frame-ring/Cargo.toml | head -n 1)
target=${CARGO_TARGET_DIR:-target}
work=$(mktemp -d)
tar -xzf "$target/package/kessel-frame-ring-$version.crate" -C "$work"
cd "$work/kessel-frame-ring-$version"
# A target folder of its own: a shared one can hold another build of a crate
# of the same name and version, and a doctest may link against it.
export CARGO_TARGET_DIR="$work/target"
echo "== the packaged crate: only the code, the README, the changelog and the licences"
unexpected=$(find . -type f ! -path './src/*' ! -path './target/*' \
    ! -name Cargo.toml ! -name Cargo.toml.orig ! -name Cargo.lock \
    ! -name .cargo_vcs_info.json ! -name README.md ! -name CHANGELOG.md \
    ! -name LICENSE-MIT ! -name LICENSE-APACHE)
if [ -n "$unexpected" ]; then
    echo "unexpected files in the package:" >&2
    echo "$unexpected" >&2
    exit 1
fi
echo "== the packaged crate: cargo test"
cargo test
echo "== the packaged crate: cargo test --release"
cargo test --release
echo "== the packaged crate: cargo doc"
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
