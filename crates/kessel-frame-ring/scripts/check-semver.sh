#!/bin/sh
# Check kessel-frame-ring's public API against a published version with
# cargo-semver-checks: a patch release must report no breaking change.
# Uses cargo-semver-checks from PATH, else fetches its release binary for
# this machine into a temporary folder.
# Usage, from the repository root:
#   sh crates/kessel-frame-ring/scripts/check-semver.sh [baseline] [release-type]
# Defaults: baseline 0.1.0, release type patch.
set -eu
baseline=${1:-0.1.0}
release=${2:-patch}
if ! command -v cargo-semver-checks >/dev/null 2>&1; then
    host=$(rustc -vV | sed -n 's/^host: //p')
    bin=$(mktemp -d)
    curl -sSfL "https://github.com/obi1kenobi/cargo-semver-checks/releases/latest/download/cargo-semver-checks-$host.tar.gz" | tar -xz -C "$bin"
    PATH="$bin:$PATH"
    export PATH
fi
cargo semver-checks --version
cargo semver-checks -p kessel-frame-ring --baseline-version "$baseline" --release-type "$release"
