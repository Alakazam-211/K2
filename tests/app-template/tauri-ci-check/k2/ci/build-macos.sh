#!/usr/bin/env bash
# An UNSIGNED macOS build of this Tauri app (k2 app init --template tauri):
# a compile check on a Mac compute node. Release builds for macOS are
# signed and notarized on your own Mac (Developer ID stays there), so this
# makes no updater archive. Writes k2/out/<bin> and k2/out/<bin>.app.zip.
set -euo pipefail
cd "$(dirname "$0")/../.."
root="$(pwd -P)"
target="${CARGO_TARGET_DIR:-$root/src-tauri/target}"
case "$target" in /*) ;; *) target="$root/$target" ;; esac
export CARGO_TARGET_DIR="$target"
bin="$(awk '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^name *=/{gsub(/.*= *"|".*/, ""); print; exit}' src-tauri/Cargo.toml)"
[ -n "$bin" ] || { echo "build-macos.sh: no [package] name in src-tauri/Cargo.toml" >&2; exit 1; }
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
"${tauri[@]}" build --ci --bundles app --config '{"bundle":{"active":true,"createUpdaterArtifacts":false}}'
out="$root/k2/out"
rm -rf "$out"
mkdir -p "$out"
cp "$target/release/$bin" "$out/$bin"
apps=("$target"/release/bundle/macos/*.app)
[ "${#apps[@]}" = 1 ] && [ -d "${apps[0]}" ] || { echo "build-macos.sh: want one .app, found: ${apps[*]}" >&2; exit 1; }
ditto -c -k --keepParent "${apps[0]}" "$out/$bin.app.zip"
(cd "$out" && shasum -a 256 -- *)
