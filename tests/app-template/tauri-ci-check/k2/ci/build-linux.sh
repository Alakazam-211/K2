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

# Tauri's .deb keeps each file's build-time mtime and the folder's read
# order, so two builds of one commit differ only there: repack it with the
# commit's time, names sorted, root as owner, and no gzip name or time.
repack_deb() {
    local deb="$1" w
    w="$(mktemp -d)"
    (cd "$w" && ar x "$deb" && mkdir c d && tar -xpzf control.tar.gz -C c && tar -xpzf data.tar.gz -C d)
    local t=(--sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner --format=gnu)
    (cd "$w/c" && tar "${t[@]}" -cf - . | gzip -9n > ../control.tar.gz)
    (cd "$w/d" && tar "${t[@]}" -cf - . | gzip -9n > ../data.tar.gz)
    rm -f "$deb"
    (cd "$w" && ar -rcD "$deb" debian-binary control.tar.gz data.tar.gz)
    rm -rf "$w"
}

# The AppImage is a runtime (an ELF) with a squashfs after it. The squashfs
# keeps each file's build-time mtime, and the runtime's .digest_md5 section
# is the MD5 of the whole image, so both differ between builds. Rebuild
# the squashfs from the same files with the commit's time, root as owner
# and no xattrs (mksquashfs sorts each folder), splice it back onto the
# runtime, and zero .digest_md5 (an AppImage with no digest is valid; Tauri's
# updater and the runtime never read it, and the signer's .sig covers the
# whole file). Needs python3 and
# mksquashfs 4.4+: on PATH, else the one inside Tauri's cached appimage
# plugin. The AppImage also carries the build machine's own libraries, so
# two builds match only on the same distro release and packages.
normalise_appimage() {
    local img="$1" w mks plugin
    w="$(mktemp -d)"
    mks="$(command -v mksquashfs || true)"
    if [ -z "$mks" ]; then
        plugin="$(ls "$HOME"/.cache/tauri/*plugin-appimage*.AppImage 2>/dev/null | head -1)"
        [ -n "$plugin" ] || { echo "build-linux.sh: no mksquashfs (and no Tauri appimage plugin to take one from)" >&2; exit 1; }
        (cd "$w" && "$plugin" --appimage-extract >/dev/null && mv squashfs-root plugin)
        mks="$(find "$w/plugin" -type f -name mksquashfs | head -1)"
        [ -x "$mks" ] || { echo "build-linux.sh: Tauri's appimage plugin holds no mksquashfs" >&2; exit 1; }
    fi
    command -v python3 >/dev/null || { echo "build-linux.sh: python3 is needed to rebuild the AppImage" >&2; exit 1; }
    local off
    off="$(python3 -c 'import struct,sys; b=open(sys.argv[1],"rb").read(64); o,=struct.unpack_from("<Q",b,0x28); e,n=struct.unpack_from("<HH",b,0x3A); print(o+e*n)' "$img")"
    head -c "$off" "$img" > "$w/image"
    (cd "$w" && "$img" --appimage-extract >/dev/null)
    # mksquashfs refuses SOURCE_DATE_EPOCH next to -mkfs-time/-all-time, so
    # pass the commit time as flags and unset it for this one call.
    env -u SOURCE_DATE_EPOCH "$mks" "$w/squashfs-root" "$w/fs" -noappend -quiet -no-progress -all-root -no-xattrs \
        -comp zstd -b 131072 -mkfs-time "$SOURCE_DATE_EPOCH" -all-time "$SOURCE_DATE_EPOCH" >/dev/null
    cat "$w/fs" >> "$w/image"
    python3 - "$w/image" <<'PY'
import struct, sys
p = sys.argv[1]
b = bytearray(open(p, "rb").read())
shoff, = struct.unpack_from("<Q", b, 0x28)
entsize, num, strndx = struct.unpack_from("<HHH", b, 0x3A)
secs = []
for i in range(num):
    name, _t, _f, _a, off, size = struct.unpack_from("<IIQQQQ", b, shoff + i * entsize)
    secs.append((name, off, size))
stroff = secs[strndx][1]
named = {bytes(b[stroff + n:b.index(0, stroff + n)]).decode(): (off, size) for n, off, size in secs}
off, size = named[".digest_md5"]
b[off:off + size] = bytes(size)
open(p, "wb").write(b)
PY
    chmod 0755 "$w/image"
    mv "$w/image" "$img"
    rm -rf "$w"
}

out="$root/k2/out"
rm -rf "$out"
mkdir -p "$out"
cp "$target/release/$bin" "$out/$bin"
debs=("$target"/release/bundle/deb/*.deb)
imgs=("$target"/release/bundle/appimage/*.AppImage)
[ "${#debs[@]}" = 1 ] && [ -f "${debs[0]}" ] || { echo "build-linux.sh: want one .deb, found: ${debs[*]}" >&2; exit 1; }
[ "${#imgs[@]}" = 1 ] && [ -f "${imgs[0]}" ] || { echo "build-linux.sh: want one .AppImage, found: ${imgs[*]}" >&2; exit 1; }
cp "${debs[0]}" "$out/$bin.deb"
repack_deb "$out/$bin.deb"
cp "${imgs[0]}" "$out/$bin.AppImage"
normalise_appimage "$out/$bin.AppImage"
echo "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH"
(cd "$out" && sha256sum -- *)
