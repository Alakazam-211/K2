#!/bin/sh
# Type-check kessel-video for Windows from a Linux machine whose toolchain
# can't add targets (read-only rustup): fetch the matching rust-std for
# x86_64-pc-windows-msvc into a private sysroot and point the target build
# at it. Build scripts and proc macros still use the host's own sysroot.
# Usage, from the repository root: sh crates/kessel-video/scripts/check-windows.sh
set -eu
target=x86_64-pc-windows-msvc
version=$(rustc --version | cut -d' ' -f2)
root=${TMPDIR:-/tmp}/kessel-video-sysroot-$version
if [ ! -d "$root/lib/rustlib/$target/lib" ]; then
    work=$(mktemp -d)
    curl -sSfL "https://static.rust-lang.org/dist/rust-std-$version-$target.tar.xz" -o "$work/std.tar.xz"
    tar -xJf "$work/std.tar.xz" -C "$work"
    mkdir -p "$root/lib/rustlib"
    cp -R "$work/rust-std-$version-$target/rust-std-$target/lib/rustlib/$target" "$root/lib/rustlib/"
    rm -rf "$work"
fi
RUSTFLAGS="--sysroot=$root ${RUSTFLAGS:-}" cargo check --locked -p kessel-video --all-targets --target "$target"
