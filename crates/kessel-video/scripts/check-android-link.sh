#!/bin/sh
# Link kessel-video for Android against the NDK's API 24 stubs: build the
# test binaries (tests/vp8_round_trip.rs reaches every NDK call through
# system()) with the NDK's clang as the linker, so a misspelt symbol, or one
# newer than API 24, fails the link. Then link examples/link_check.rs as a
# shared library, as an app does, with no undefined symbols allowed. Nothing
# runs: there is no device.
# The first run downloads the NDK (about 700 MB) and each target's rust-std
# into $TMPDIR, so a read-only rustup is fine. Linux x86-64 hosts, with curl,
# unzip and tar.
# Usage, from the repository root:
#   sh crates/kessel-video/scripts/check-android-link.sh [target...]
# Default targets: aarch64-linux-android x86_64-linux-android.
set -eu
[ $# -gt 0 ] || set -- aarch64-linux-android x86_64-linux-android
here=$(pwd)
target_dir=${CARGO_TARGET_DIR:-target}
case $target_dir in
/*) ;;
*) target_dir=$here/$target_dir ;;
esac
ndk_version=r27c
api=24
tmp=${TMPDIR:-/tmp}
ndk=$tmp/android-ndk-$ndk_version
if [ ! -d "$ndk" ]; then
    work=$(mktemp -d)
    curl -sSfL "https://dl.google.com/android/repository/android-ndk-$ndk_version-linux.zip" -o "$work/ndk.zip"
    unzip -q "$work/ndk.zip" -d "$work"
    mv "$work/android-ndk-$ndk_version" "$ndk"
    rm -rf "$work"
fi
bin=$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin
version=$(rustc --version | cut -d' ' -f2)
root=$tmp/kessel-video-sysroot-$version
for target in "$@"; do
    if [ ! -d "$root/lib/rustlib/$target/lib" ]; then
        work=$(mktemp -d)
        curl -sSfL "https://static.rust-lang.org/dist/rust-std-$version-$target.tar.xz" -o "$work/std.tar.xz"
        tar -xJf "$work/std.tar.xz" -C "$work"
        mkdir -p "$root/lib/rustlib"
        cp -R "$work/rust-std-$version-$target/rust-std-$target/lib/rustlib/$target" "$root/lib/rustlib/"
        rm -rf "$work"
    fi
done
for target in "$@"; do
    echo "== link $target (API $api, NDK $ndk_version)"
    linker_var=CARGO_TARGET_$(echo "$target" | tr 'a-z-' 'A-Z_')_LINKER
    exe=$(env "$linker_var=$bin/$target$api-clang" RUSTFLAGS="--sysroot=$root ${RUSTFLAGS:-}" \
        cargo test --locked -p kessel-video --target "$target" --no-run --message-format=json |
        sed -n 's/.*"executable":"\([^"]*vp8_round_trip[^"]*\)".*/\1/p' | tail -n 1)
    [ -n "$exe" ] || { echo "no vp8_round_trip binary for $target" >&2; exit 1; }
    # The link is only a check if the NDK libraries are really in it.
    for lib in libcamera2ndk.so libmediandk.so; do
        "$bin/llvm-readelf" -d "$exe" | grep -q "Shared library: \[$lib\]" ||
            { echo "$exe does not link $lib" >&2; exit 1; }
    done
    for sym in ACameraManager_openCamera AImageReader_new AMediaCodec_createEncoderByType; do
        "$bin/llvm-nm" -D --undefined-only "$exe" | grep -qw "$sym" ||
            { echo "$exe does not import $sym" >&2; exit 1; }
    done
    echo "linked: $exe imports the NDK's camera, image reader and codec calls"

    # As an app links it: an app crate (examples/link_check.rs as its
    # library, a Tauri Android app's libapp.so) built as a shared library
    # that depends on kessel-video, outside the workspace (from crates.io)
    # as users get it. It reaches every backend call through system().
    # --no-undefined: a library the crate forgets to ask for fails here, not
    # when Android loads the app.
    echo "== link $target: an app's shared library over kessel-video"
    app=$(mktemp -d)
    mkdir -p "$app/src"
    cp crates/kessel-video/examples/link_check.rs "$app/src/lib.rs"
    cp Cargo.lock "$app/Cargo.lock"
    cat >"$app/Cargo.toml" <<EOF
[package]
name = "link-check-app"
version = "0.0.0"
edition = "2021"
publish = false

[lib]
crate-type = ["cdylib"]

# The example's cfg names kessel-video's backend features.
[features]
default = ["android"]
android = []

[dependencies]
kessel-video = { path = "$here/crates/kessel-video" }

[workspace]
EOF
    (cd "$app" && env "$linker_var=$bin/$target$api-clang" CARGO_TARGET_DIR="$target_dir/kessel-video-link-check" \
        RUSTFLAGS="--sysroot=$root -C link-arg=-Wl,--no-undefined ${RUSTFLAGS:-}" \
        cargo build --lib --target "$target")
    so=$target_dir/kessel-video-link-check/$target/debug/liblink_check_app.so
    [ -f "$so" ] || { echo "no $so" >&2; exit 1; }
    for lib in libcamera2ndk.so libmediandk.so; do
        "$bin/llvm-readelf" -d "$so" | grep -q "Shared library: \[$lib\]" ||
            { echo "$so does not link $lib" >&2; exit 1; }
    done
    for sym in ACameraManager_openCamera AImageReader_new AMediaCodec_createEncoderByType kessel_video_link_check; do
        "$bin/llvm-nm" -D "$so" | grep -qw "$sym" ||
            { echo "$so has no $sym" >&2; exit 1; }
    done
    echo "linked: $so needs libcamera2ndk and libmediandk, with no undefined symbols left"
done
