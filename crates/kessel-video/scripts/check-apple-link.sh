#!/bin/sh
# Link kessel-video the way Xcode links a Tauri app's Rust code: an app
# crate (examples/link_check.rs as its library) built as a static library
# that depends on kessel-video, linked with no -framework flags of the
# app's own. rustc's #[link] attributes never reach that link (rustc only
# prints them as native-static-libs); only the linker options kessel-video
# writes into its object code do. A framework the crate forgets shows up
# here as undefined symbols, as it does in the app. The app crate is built
# outside the workspace (from crates.io, needs the network), as users get
# kessel-video's dependencies.
#
# macos: link the library with cc and only the plain libraries rustc names
#   (its -framework flags dropped), check the program loads every framework
#   the backend needs, and run it: VideoToolbox H.264 and VP8, one frame
#   each through an encoder and a decoder.
# ios: build the library for aarch64-apple-ios. With an iPhoneOS SDK (full
#   Xcode; DEVELOPER_DIR may point at one), link it for arm64 iOS the same
#   way (it can't run here). Without one (Command Line Tools only), only the
#   archive check below runs, and the script says so.
# Both: the archive check. Every framework symbol the archive leaves
#   undefined (VT*, CM*, CV*, CF*, and their k constants) must have that
#   framework among the archive's linker options (AV* is AVFoundation), and
#   so must Foundation, whose classes are looked up at run time. A linker
#   keeps an auto-linked framework only when it takes a symbol from it, so
#   the program check also wants AVFoundation in the program's load list.
#
# The iOS standard library is fetched into $TMPDIR when rustup lacks it, so
# a read-only rustup is fine.
# Usage, from the repository root, on a Mac:
#   sh crates/kessel-video/scripts/check-apple-link.sh [macos] [ios]
set -eu
[ $# -gt 0 ] || set -- macos ios
here=$(pwd)
target_dir=${CARGO_TARGET_DIR:-target}
case $target_dir in
/*) ;;
*) target_dir=$here/$target_dir ;;
esac
work=$(mktemp -d)
cat >"$work/main.c" <<'EOF'
#include <stdio.h>
void *objc_getClass(const char *name);
int kessel_video_link_check(void);
int main(void) {
    /* The crate looks AVFoundation's and Foundation's classes up at run
       time: the frameworks must be loaded. */
    if (!objc_getClass("AVCaptureDevice") || !objc_getClass("NSString")) {
        fprintf(stderr, "AVFoundation or Foundation is not loaded\n");
        return 1;
    }
    return kessel_video_link_check();
}
EOF

# The app: a crate of its own whose library is examples/link_check.rs, a
# static library that depends on kessel-video, as a Tauri app's crate does.
# It starts from the workspace's Cargo.lock and is outside the workspace,
# so it gets kessel-video's dependencies as a user of the crate does.
# A fresh copy each run, so rustc runs and prints the native libraries.
mkdir -p "$work/app/src"
cp crates/kessel-video/examples/link_check.rs "$work/app/src/lib.rs"
cp Cargo.lock "$work/app/Cargo.lock"
cat >"$work/app/Cargo.toml" <<EOF
[package]
name = "link-check-app"
version = "0.0.0"
edition = "2021"
publish = false

[lib]
crate-type = ["staticlib"]

# The example's cfg names kessel-video's backend features.
[features]
default = ["apple"]
apple = []

[dependencies]
kessel-video = { path = "$here/crates/kessel-video" }

[workspace]
EOF

# build <target> <log> [rustflags]: the app's static library, with rustc's
# list of native libraries in <log>.
build() {
    (cd "$work/app" && env CARGO_TARGET_DIR="$target_dir/kessel-video-link-check" \
        RUSTFLAGS="${3:-} ${RUSTFLAGS:-}" cargo rustc --lib --target "$1" \
        -- --print native-static-libs) >"$2" 2>&1 ||
        { cat "$2" >&2; exit 1; }
    lib="$target_dir/kessel-video-link-check/$1/debug/liblink_check_app.a"
    [ -f "$lib" ] || { echo "no $lib" >&2; exit 1; }
}

# plain_libs <log>: rustc's native-static-libs without its -framework pairs:
# what an app that never read that list still links (libSystem, libobjc...).
plain_libs() {
    grep -q 'native-static-libs: ' "$1" || { echo "rustc printed no native-static-libs" >&2; cat "$1" >&2; exit 1; }
    sed -n 's/.*native-static-libs: //p' "$1" | tail -n 1 |
        awk '{ for (i = 1; i <= NF; i++) { if ($i == "-framework") { i++; continue } printf "%s ", $i } }'
}

# archive_check <lib>
archive_check() {
    options=$(otool -l "$1" | awk '/cmd LC_LINKER_OPTION/ { n = 1; next } n && /string #/ { print $3 } /cmd LC_/ && !/LC_LINKER_OPTION/ { n = 0 }' |
        grep -v '^-framework$' | sort -u)
    echo "linker options in the archive: $(echo $options)"
    # --no-llvm-bc: the members also carry Rust's LLVM bitcode, which an
    # older LLVM in Apple's nm can't read; the machine code is what links.
    # nm says "no symbols" for empty members; anything else fails the check.
    nm --no-llvm-bc -u -j "$1" >"$work/undefined" 2>"$work/nm.err" || true
    if grep -v -e 'no symbols$' -e '^$' "$work/nm.err" | grep -q .; then
        cat "$work/nm.err" >&2
        exit 1
    fi
    sed -E -n -e 's/^_k?(VT|CM|CV|CF)[A-Z].*/\1/p' -e 's/^_(AV)[A-Z].*/\1/p' "$work/undefined" |
        sort -u >"$work/prefixes"
    missing=
    for prefix in $(cat "$work/prefixes"); do
        case $prefix in
            VT) fw=VideoToolbox ;;
            CM) fw=CoreMedia ;;
            CV) fw=CoreVideo ;;
            CF) fw=CoreFoundation ;;
            AV) fw=AVFoundation ;;
        esac
        echo "$options" | grep -qx "$fw" || missing="$missing $fw"
    done
    for fw in AVFoundation Foundation; do
        echo "$options" | grep -qx "$fw" || missing="$missing $fw"
    done
    if [ -n "$missing" ]; then
        echo "the archive needs these frameworks but doesn't ask for them:$missing" >&2
        grep -E '^_(k?(VT|CM|CV|CF)|AV)[A-Z]' "$work/undefined" | sort -u >&2
        exit 1
    fi
    echo "archive check: every framework symbol it leaves undefined has its linker option"
}

for os in "$@"; do
    case $os in
    macos)
        target=$(rustc -vV | sed -n 's/^host: //p')
        echo "== macos ($target): static library, linked with cc and no -framework flags"
        build "$target" "$work/macos.log"
        # shellcheck disable=SC2046
        cc "$work/main.c" "$lib" $(plain_libs "$work/macos.log") -o "$work/link_check"
        archive_check "$lib"
        for fw in VideoToolbox CoreMedia CoreVideo CoreFoundation AVFoundation; do
            otool -L "$work/link_check" | grep -q "/$fw.framework/" ||
                { echo "the program does not load $fw" >&2; exit 1; }
        done
        echo "linked: the program loads VideoToolbox, CoreMedia, CoreVideo, CoreFoundation and AVFoundation"
        "$work/link_check"
        ;;
    ios)
        target=aarch64-apple-ios
        echo "== ios ($target): static library"
        flags=
        if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
            version=$(rustc --version | cut -d' ' -f2)
            root=${TMPDIR:-/tmp}/kessel-video-sysroot-$version
            if [ ! -d "$root/lib/rustlib/$target/lib" ]; then
                fetch=$(mktemp -d)
                curl -sSfL "https://static.rust-lang.org/dist/rust-std-$version-$target.tar.xz" -o "$fetch/std.tar.xz"
                tar -xJf "$fetch/std.tar.xz" -C "$fetch"
                mkdir -p "$root/lib/rustlib"
                cp -R "$fetch/rust-std-$version-$target/rust-std-$target/lib/rustlib/$target" "$root/lib/rustlib/"
                rm -rf "$fetch"
            fi
            flags="--sysroot=$root"
        fi
        build "$target" "$work/ios.log" "$flags"
        archive_check "$lib"
        if sdk=$(xcrun --sdk iphoneos --show-sdk-path 2>/dev/null); then
            # shellcheck disable=SC2046
            xcrun --sdk iphoneos clang -target arm64-apple-ios13.0 -isysroot "$sdk" \
                "$work/main.c" "$lib" $(plain_libs "$work/ios.log") -o "$work/link_check-ios"
            for fw in VideoToolbox CoreMedia CoreVideo CoreFoundation AVFoundation; do
                otool -L "$work/link_check-ios" | grep -q "/$fw.framework/" ||
                    { echo "the iOS program does not load $fw" >&2; exit 1; }
            done
            echo "linked for iOS with $sdk (not run: no device)"
        else
            echo "no iPhoneOS SDK here (xcrun --sdk iphoneos): the iOS link was not run, only the archive check"
        fi
        ;;
    *)
        echo "unknown OS $os: macos or ios" >&2
        exit 2
        ;;
    esac
done
