#!/bin/sh
# Link kessel-video the way Xcode links a Tauri app's Rust code: as a static
# library (examples/link_check.rs), with no -framework flags of the app's
# own. rustc's #[link] attributes never reach that link (rustc only prints
# them as native-static-libs); only the linker options kessel-video writes
# into its object code do. A framework the crate forgets shows up here as
# undefined symbols, as it does in the app.
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
#   framework among the archive's linker options, and so must AVFoundation
#   and Foundation, whose classes are looked up at run time.
#
# The iOS standard library is fetched into $TMPDIR when rustup lacks it, so
# a read-only rustup is fine.
# Usage, from the repository root, on a Mac:
#   sh crates/kessel-video/scripts/check-apple-link.sh [macos] [ios]
set -eu
[ $# -gt 0 ] || set -- macos ios
target_dir=${CARGO_TARGET_DIR:-target}
work=$(mktemp -d)
cat >"$work/main.c" <<'EOF'
#include <stdio.h>
void *objc_getClass(const char *name);
int kessel_video_link_check(void);
int main(void) {
    /* AVFoundation has no function the crate calls by name: only the
       linker options load it. */
    if (!objc_getClass("AVCaptureDevice")) {
        fprintf(stderr, "AVFoundation is not loaded: no AVCaptureDevice class\n");
        return 1;
    }
    return kessel_video_link_check();
}
EOF

# build <target> <log> [rustflags]: the static library, with rustc's list
# of native libraries in <log>.
build() {
    env RUSTFLAGS="${3:-} ${RUSTFLAGS:-}" cargo rustc --locked -p kessel-video --example link_check --target "$1" \
        --crate-type staticlib -- --print native-static-libs >"$2" 2>&1 ||
        { cat "$2" >&2; exit 1; }
    lib="$target_dir/$1/debug/examples/liblink_check.a"
    [ -f "$lib" ] || { echo "no $lib" >&2; exit 1; }
}

# plain_libs <log>: rustc's native-static-libs without its -framework pairs:
# what an app that never read that list still links (libSystem, libobjc...).
plain_libs() {
    sed -n 's/.*native-static-libs: //p' "$1" | tail -n 1 |
        awk '{ for (i = 1; i <= NF; i++) { if ($i == "-framework") { i++; continue } printf "%s ", $i } }'
}

# archive_check <lib>
archive_check() {
    options=$(otool -l "$1" | awk '/cmd LC_LINKER_OPTION/ { n = 1; next } n && /string #/ { print $3 } /cmd LC_/ && !/LC_LINKER_OPTION/ { n = 0 }' |
        grep -v '^-framework$' | sort -u)
    echo "linker options in the archive: $(echo $options)"
    nm -u -j "$1" | sed -E -n 's/^_k?(VT|CM|CV|CF)[A-Z].*/\1/p' | sort -u >"$work/prefixes"
    missing=
    for prefix in $(cat "$work/prefixes"); do
        case $prefix in
            VT) fw=VideoToolbox ;;
            CM) fw=CoreMedia ;;
            CV) fw=CoreVideo ;;
            CF) fw=CoreFoundation ;;
        esac
        echo "$options" | grep -qx "$fw" || missing="$missing $fw"
    done
    for fw in AVFoundation Foundation; do
        echo "$options" | grep -qx "$fw" || missing="$missing $fw"
    done
    if [ -n "$missing" ]; then
        echo "the archive needs these frameworks but doesn't ask for them:$missing" >&2
        nm -u -j "$1" | grep -E '^_k?(VT|CM|CV|CF)[A-Z]' | sort -u >&2
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
        for fw in VideoToolbox CoreMedia CoreVideo CoreFoundation AVFoundation Foundation; do
            otool -L "$work/link_check" | grep -q "/$fw.framework/" ||
                { echo "the program does not load $fw" >&2; exit 1; }
        done
        echo "linked: the program loads VideoToolbox, CoreMedia, CoreVideo, CoreFoundation, AVFoundation and Foundation"
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
            for fw in VideoToolbox CoreMedia CoreVideo CoreFoundation AVFoundation Foundation; do
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
