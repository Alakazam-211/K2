#!/usr/bin/env bash
# The Linux release build of this Tauri app (k2 app init --template tauri).
# K2 Compute runs it for k2 ci release / k2 ci run; it runs by hand too
# (Linux, Rust, Node and Tauri's system packages installed).
#
# Two nodes must make the same bytes (the signer compares them), so:
# SOURCE_DATE_EPOCH is the commit's time, the checkout, CARGO_HOME and the
# target dir are mapped out of every binary, and nothing builds
# incrementally. No updater signing here: no key ever reaches a node; the
# signer signs the files afterwards.
#
# Writes k2/out/<bin>, k2/out/<bin>.deb and k2/out/<bin>.AppImage
# (<bin> = src-tauri's package name), and prints their sha256.
set -euo pipefail
cd "$(dirname "$0")/../.."
root="$(pwd -P)"

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH="$(git -c safe.directory='*' log -1 --format=%ct HEAD)" || {
        echo "build-linux.sh: no commit time from git: run it in a git checkout, or set SOURCE_DATE_EPOCH" >&2
        exit 1
    }
fi
export SOURCE_DATE_EPOCH
target="${CARGO_TARGET_DIR:-$root/src-tauri/target}"
case "$target" in /*) ;; *) target="$root/$target" ;; esac
mkdir -p "$target"
target="$(cd "$target" && pwd -P)"
export CARGO_TARGET_DIR="$target"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
for p in "$root" "$cargo_home" "$target"; do
    case "$p" in *[[:space:]]*) echo "build-linux.sh: a build path has a space ($p)" >&2; exit 1 ;; esac
done
# The most specific map last (rustc and cc use the last one that matches).
maps="$root=/src $cargo_home=/cargo $target=/target"
rflags="" cflags=""
for m in $maps; do
    rflags="$rflags --remap-path-prefix=$m"
    cflags="$cflags -ffile-prefix-map=$m"
done
export RUSTFLAGS="${RUSTFLAGS:-}$rflags"
export CFLAGS="${CFLAGS:-}$cflags"
export CXXFLAGS="${CXXFLAGS:-}$cflags"
export CARGO_INCREMENTAL=0
export TZ=UTC LC_ALL=C.UTF-8

bin="$(awk '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^name *=/{gsub(/.*= *"|".*/, ""); print; exit}' src-tauri/Cargo.toml)"
[ -n "$bin" ] || { echo "build-linux.sh: no [package] name in src-tauri/Cargo.toml" >&2; exit 1; }
# Two builds must resolve the same crates: a committed Cargo.lock, used as is.
[ -f src-tauri/Cargo.lock ] || [ -f Cargo.lock ] || {
    echo "build-linux.sh: no Cargo.lock: commit it (cargo generate-lockfile in src-tauri), or the two builds may pick different crates" >&2
    exit 1
}

# The web build (Tauri runs beforeBuildCommand) and the Tauri CLI.
if [ -f package-lock.json ]; then
    npm ci --no-audit --no-fund
elif [ -f package.json ]; then
    npm install --no-audit --no-fund
fi
if [ -x node_modules/.bin/tauri ]; then
    tauri=(node_modules/.bin/tauri)
else
    tauri=(npx --yes @tauri-apps/cli@2)
fi
# Bundles on, updater archives off (the signer signs the AppImage itself).
"${tauri[@]}" build --ci --bundles deb,appimage --config '{"bundle":{"active":true,"createUpdaterArtifacts":false}}' -- --locked

out="$root/k2/out"
rm -rf "$out"
mkdir -p "$out"
cp "$target/release/$bin" "$out/$bin"
debs=("$target"/release/bundle/deb/*.deb)
imgs=("$target"/release/bundle/appimage/*.AppImage)
[ "${#debs[@]}" = 1 ] && [ -f "${debs[0]}" ] || { echo "build-linux.sh: want one .deb, found: ${debs[*]}" >&2; exit 1; }
[ "${#imgs[@]}" = 1 ] && [ -f "${imgs[0]}" ] || { echo "build-linux.sh: want one .AppImage, found: ${imgs[*]}" >&2; exit 1; }
cp "${debs[0]}" "$out/$bin.deb"
cp "${imgs[0]}" "$out/$bin.AppImage"
echo "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH"
(cd "$out" && sha256sum -- *)
