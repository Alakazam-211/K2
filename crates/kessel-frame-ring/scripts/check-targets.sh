#!/bin/sh
# Type-check kessel-frame-ring for targets the machine can't run (Windows and the
# phones from Linux) on a toolchain that can't add targets (read-only
# rustup): fetch the matching rust-std for each target into a private sysroot
# and point the target build at it. Build scripts and proc macros still use
# the host's own sysroot. This is a type check (cargo check, or cargo clippy
# with CLIPPY=1): nothing is linked, so no Windows SDK, iOS SDK or Android NDK
# is needed. CARGO_FLAGS (optional) is added to the cargo command.
# Off Unix only the types build: the ring itself is Unix only.
# Usage, from the repository root:
#   sh crates/kessel-frame-ring/scripts/check-targets.sh [target...]
# Default targets: x86_64-pc-windows-msvc aarch64-apple-ios aarch64-linux-android.
set -eu
[ $# -gt 0 ] || set -- x86_64-pc-windows-msvc aarch64-apple-ios aarch64-linux-android
version=$(rustc --version | cut -d' ' -f2)
root=${TMPDIR:-/tmp}/kessel-frame-ring-sysroot-$version
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
    echo "== $target ${CARGO_FLAGS:-}"
    # The host's sysroot keeps the host target, so build scripts still run.
    # CARGO_FLAGS is split into words on purpose.
    if [ "${CLIPPY:-0}" = 1 ]; then
        # shellcheck disable=SC2086
        RUSTFLAGS="--sysroot=$root ${RUSTFLAGS:-}" cargo clippy --locked -p kessel-frame-ring --all-targets --target "$target" ${CARGO_FLAGS:-} -- -D warnings
    else
        # shellcheck disable=SC2086
        RUSTFLAGS="--sysroot=$root -D warnings ${RUSTFLAGS:-}" cargo check --locked -p kessel-frame-ring --all-targets --target "$target" ${CARGO_FLAGS:-}
    fi
done
