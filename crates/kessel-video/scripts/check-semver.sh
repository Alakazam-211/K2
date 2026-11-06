#!/bin/sh
# Compare kessel-video's public API with a release on crates.io
# (cargo-semver-checks): it says whether the next release can be 0.1.x or
# must be 0.2.0. The baseline is the published .crate itself, downloaded and
# unpacked. Both sides' rustdoc JSON is built here, each in its own target
# folder, and handed to cargo-semver-checks: with the same name and version
# on both sides, its own builds can share a cache and compare the current
# code with itself. The API differs by target, so run it on each OS you care
# about (Linux and macOS at least). cargo-semver-checks is used from PATH,
# else its release binary is fetched into a private folder.
# Usage, from the repository root:
#   sh crates/kessel-video/scripts/check-semver.sh [baseline version, default 0.1.0]
set -eu
baseline=${1:-0.1.0}
work=$(mktemp -d)
if ! command -v cargo-semver-checks >/dev/null 2>&1; then
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) asset=x86_64-unknown-linux-gnu ;;
        Linux-aarch64) asset=aarch64-unknown-linux-gnu ;;
        Darwin-arm64) asset=aarch64-apple-darwin ;;
        Darwin-x86_64) asset=x86_64-apple-darwin ;;
        *) echo "no cargo-semver-checks release for $(uname -s)-$(uname -m): cargo install it" >&2; exit 1 ;;
    esac
    mkdir -p "$work/bin"
    curl -sSfL "https://github.com/obi1kenobi/cargo-semver-checks/releases/latest/download/cargo-semver-checks-$asset.tar.gz" | tar -xz -C "$work/bin"
    PATH="$work/bin:$PATH"
    export PATH
fi
cargo semver-checks --version
curl -sSfL -A "kessel-video check-semver.sh" "https://crates.io/api/v1/crates/kessel-video/$baseline/download" -o "$work/baseline.crate"
tar -xzf "$work/baseline.crate" -C "$work"
root="$work/kessel-video-$baseline"
echo "== baseline: kessel-video $baseline from crates.io, built from $(sed -n 's/.*"sha1": "\(.*\)".*/\1/p' "$root/.cargo_vcs_info.json")"
here=$(pwd)

# rustdoc's JSON output is unstable; cargo-semver-checks builds it the same
# way, on the stable toolchain.
json() { # <out> <features flag> <cargo args...>
    out=$1
    shift
    RUSTC_BOOTSTRAP=1 RUSTDOCFLAGS="-Z unstable-options --output-format json" \
        cargo rustdoc --lib "$@"
    cp "$CARGO_TARGET_DIR/doc/kessel_video.json" "$out"
}

status=0
for features in --all-features --default-features; do
    flag=$features
    [ "$flag" = --default-features ] && flag=
    echo "== against $baseline, $features"
    (CARGO_TARGET_DIR="$work/target-current" json "$work/current.json" -p kessel-video $flag)
    (cd "$root" && CARGO_TARGET_DIR="$work/target-baseline" json "$work/baseline.json" $flag)
    cd "$here"
    cargo semver-checks check-release --current-rustdoc "$work/current.json" \
        --baseline-rustdoc "$work/baseline.json" --release-type patch || status=1
done
exit $status
