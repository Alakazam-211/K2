#!/bin/sh
# Type-check kessel-audio for targets the machine can't run (the phones, or
# Windows from Linux) on a toolchain that can't add targets (read-only
# rustup): fetch the matching rust-std for each target into a private sysroot
# and point the target build at it. Build scripts and proc macros still use
# the host's own sysroot. This is a type check (cargo check, or cargo clippy
# with CLIPPY=1): nothing is linked, so no iOS SDK or Android NDK is needed.
# Usage, from the repository root:
#   sh crates/kessel-audio/scripts/check-targets.sh [target...]
# Default targets: aarch64-apple-ios aarch64-linux-android.
set -eu
[ $# -gt 0 ] || set -- aarch64-apple-ios aarch64-linux-android
version=$(rustc --version | cut -d' ' -f2)
root=${TMPDIR:-/tmp}/kessel-audio-sysroot-$version
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
    echo "== $target"
    # The host's sysroot keeps the host target, so build scripts still run.
    if [ "${CLIPPY:-0}" = 1 ]; then
        RUSTFLAGS="--sysroot=$root ${RUSTFLAGS:-}" cargo clippy --locked -p kessel-audio --all-targets --target "$target" -- -D warnings
    else
        RUSTFLAGS="--sysroot=$root -D warnings ${RUSTFLAGS:-}" cargo check --locked -p kessel-audio --all-targets --target "$target"
    fi
done
