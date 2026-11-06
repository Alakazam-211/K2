#!/bin/sh
# Package kessel-video as crates.io would (`cargo publish --dry-run` builds the
# package), then unpack the .crate outside the repository and run its tests
# and docs there, with every dependency from crates.io: what a user of the
# published crate gets. cargo package checks the git work tree, so run it
# from a clean checkout (or a copy that isn't a git repository).
# Usage, from the repository root:
#   sh crates/kessel-video/scripts/check-package.sh
set -eu
echo "== files in the package"
cargo package -p kessel-video --list
echo "== cargo publish --dry-run"
cargo publish -p kessel-video --dry-run
# publish keeps its .crate elsewhere; write it where `cargo package` does
# (already verified above).
cargo package -p kessel-video --no-verify
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' crates/kessel-video/Cargo.toml | head -n 1)
target=${CARGO_TARGET_DIR:-target}
work=$(mktemp -d)
tar -xzf "$target/package/kessel-video-$version.crate" -C "$work"
cd "$work/kessel-video-$version"
echo "== the packaged crate: cargo test"
cargo test
echo "== the packaged crate: cargo test, API and fake only"
cargo test --no-default-features --features fake
echo "== the packaged crate: cargo doc"
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
