#!/usr/bin/env bash
# cross-toolchain.sh — the hash-pinned toolchain for the Linux aarch64
# release build cross-compiled on an x86_64 node (scripts/ci/build-linux-
# bins.sh). Sourced; sets no shell options. Every function returns non-zero
# and says why on failure; nothing here is trusted unless its sha256 is the
# one scripts/ci/cross-pins.sh pins (Steward, 11-11: a build tool that
# produces something we sign is hash-pinned).
#
#   k2_cross_fetch <url> <sha256> <out>     download, then refuse other bytes
#   k2_cross_toolchain <host-arch> <dir>    zig from its release tarball and
#                                           cargo-zigbuild built from its
#                                           crates.io source (--locked), both
#                                           checked against the pins; puts
#                                           both first on PATH and sets
#                                           K2_CROSS_ZIG_TARBALL_SHA256,
#                                           K2_CROSS_ZIGBUILD_CRATE_SHA256
#                                           (what build-info.json records)
#
# Needs curl, tar with xz, and cargo (the job's pinned Rust).

_k2_cross_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi
}

k2_cross_fetch() {
    local url="$1" want="$2" out="$3" got
    case "$want" in
        [0-9a-f][0-9a-f]*) [ "${#want}" = 64 ] || { echo "FATAL: k2_cross_fetch: the pinned sha256 for $url is not 64 hex" >&2; return 1; } ;;
        *) echo "FATAL: k2_cross_fetch: no pinned sha256 for $url" >&2; return 1 ;;
    esac
    rm -f "$out"
    curl -fsSL --retry 3 --retry-delay 5 -o "$out" "$url" || { echo "FATAL: could not download $url" >&2; rm -f "$out"; return 1; }
    got="$(_k2_cross_sha256 "$out")"
    if [ "$got" != "$want" ]; then
        echo "FATAL: $url has sha256 $got, scripts/ci/cross-pins.sh pins $want: not used" >&2
        rm -f "$out"
        return 1
    fi
    echo "  verified $(basename "$out"): sha256 $got (pinned)"
}

k2_cross_toolchain() {
    local host="$1" dir="$2" zsha zurl zdir crate src
    case "$host" in
        x86_64) zsha="${K2_CROSS_ZIG_SHA256_X86_64:-}" ;;
        aarch64) zsha="${K2_CROSS_ZIG_SHA256_AARCH64:-}" ;;
        *) echo "FATAL: k2_cross_toolchain: no zig pin for a $host host" >&2; return 1 ;;
    esac
    [ -n "${K2_CROSS_ZIG:-}" ] && [ -n "${K2_CROSS_ZIGBUILD:-}" ] && [ -n "${K2_CROSS_ZIGBUILD_CRATE_SHA256:-}" ] \
        || { echo "FATAL: k2_cross_toolchain: scripts/ci/cross-pins.sh not sourced (no versions or hashes)" >&2; return 1; }
    command -v curl >/dev/null 2>&1 || { echo "FATAL: the aarch64 cross build needs curl" >&2; return 1; }
    command -v xz >/dev/null 2>&1 || { echo "FATAL: the aarch64 cross build needs xz (zig's tarball is .tar.xz)" >&2; return 1; }
    mkdir -p "$dir" || return 1

    # zig: the release tarball, pinned (its hash was checked against
    # ziglang.org's minisign signature when it was pinned).
    zurl="https://ziglang.org/download/$K2_CROSS_ZIG/zig-$host-linux-$K2_CROSS_ZIG.tar.xz"
    k2_cross_fetch "$zurl" "$zsha" "$dir/zig.tar.xz" || return 1
    tar -xJf "$dir/zig.tar.xz" -C "$dir" || { echo "FATAL: could not unpack zig" >&2; return 1; }
    zdir="$dir/zig-$host-linux-$K2_CROSS_ZIG"
    [ -x "$zdir/zig" ] || { echo "FATAL: no zig in $zdir" >&2; return 1; }
    [ "$("$zdir/zig" version)" = "$K2_CROSS_ZIG" ] || { echo "FATAL: the pinned tarball's zig says $("$zdir/zig" version), not $K2_CROSS_ZIG" >&2; return 1; }

    # cargo-zigbuild: its crates.io source, pinned, built with its own
    # Cargo.lock (--locked; cargo checks every dependency's checksum
    # against the index). No prebuilt binary, no binstall.
    crate="$dir/cargo-zigbuild-$K2_CROSS_ZIGBUILD.crate"
    k2_cross_fetch "https://static.crates.io/crates/cargo-zigbuild/cargo-zigbuild-$K2_CROSS_ZIGBUILD.crate" \
        "$K2_CROSS_ZIGBUILD_CRATE_SHA256" "$crate" || return 1
    tar -xzf "$crate" -C "$dir" || { echo "FATAL: could not unpack the cargo-zigbuild crate" >&2; return 1; }
    src="$dir/cargo-zigbuild-$K2_CROSS_ZIGBUILD"
    [ -f "$src/Cargo.lock" ] || { echo "FATAL: the cargo-zigbuild crate has no Cargo.lock (--locked needs it)" >&2; return 1; }
    echo "  building cargo-zigbuild $K2_CROSS_ZIGBUILD from its pinned source (--locked)..."
    env -u RUSTFLAGS CARGO_TARGET_DIR="$dir/zb-target" \
        cargo install --path "$src" --locked --root "$dir/zb" --quiet \
        || { echo "FATAL: building cargo-zigbuild from source failed" >&2; return 1; }
    [ "$("$dir/zb/bin/cargo-zigbuild" --version | awk '{print $2}')" = "$K2_CROSS_ZIGBUILD" ] \
        || { echo "FATAL: the built cargo-zigbuild doesn't say $K2_CROSS_ZIGBUILD" >&2; return 1; }

    export PATH="$zdir:$dir/zb/bin:$PATH"
    K2_CROSS_ZIG_TARBALL_SHA256="$zsha"
    export K2_CROSS_ZIG_TARBALL_SHA256 K2_CROSS_ZIGBUILD_CRATE_SHA256
}
