#!/bin/sh
# Build kessel-frame-ring's static library (what an app extension links) and
# check that it holds nothing but this crate, the libc crate's declarations
# and Rust's own standard library:
# - every archive member is this crate's, the libc crate's, or one of the
#   toolchain's own standard library members (found in the target's sysroot);
# - the only C-named (unmangled) symbols this crate defines are the kfr_*
#   functions of the C surface in the README, all of them;
# - the libc crate's members define no C-named symbol (it vendors no C);
# - this crate's members call no C function outside a short list of the
#   system's own.
# It builds with the release profile but without LTO, as a crates.io user's
# default profile would. A target other than the host is built against its
# rust-std, fetched into a private sysroot as in kessel-video's
# check-targets.sh (no SDK is needed: a static library isn't linked).
# Symbols are read with the toolchain's llvm-nm when it is installed (rustup
# component llvm-tools; it reads Rust's own LLVM objects), else with nm.
# Usage, from the repository root:
#   sh crates/kessel-frame-ring/scripts/check-staticlib.sh [target...]
# Default: the host. On a Mac: sh ... aarch64-apple-darwin aarch64-apple-ios
set -eu
host=$(rustc -vV | sed -n 's/^host: //p')
[ $# -gt 0 ] || set -- "$host"

# The C surface: exactly these, defined by this crate.
c_surface="kfr_monotonic_ns kfr_producer_attach kfr_producer_finish
kfr_producer_push_nv12 kfr_producer_start kfr_producer_stop_requested
kfr_producer_wants_frames"
# C functions this crate's own code may call: the system's.
allowed_imports="mmap munmap flock clock_gettime memcpy memmove memset memcmp
bcmp _Unwind_Resume rust_eh_personality __rust_probestack __error
__errno_location __errno __stack_chk_fail __stack_chk_guard bzero close strlen"
# Compiler-made symbols that may sit beside the C surface: the ELF reference
# to the panic personality routine (weak and hidden).
compiler_defs="DW.ref.rust_eh_personality"

version=$(rustc --version | cut -d' ' -f2)
host_sysroot=$(rustc --print sysroot)
root=${TMPDIR:-/tmp}/kessel-frame-ring-sysroot-$version
nm_bin=$host_sysroot/lib/rustlib/$host/bin/llvm-nm
[ -x "$nm_bin" ] || nm_bin=nm
echo "== nm: $nm_bin"
export CARGO_PROFILE_RELEASE_LTO=false
status=0
fail() {
    echo "FAIL: $*" >&2
    status=1
}
in_list() {
    for w in $2; do
        [ "$w" = "$1" ] && return 0
    done
    return 1
}

for target in "$@"; do
    echo "== $target"
    if [ "$target" = "$host" ]; then
        cargo build --locked --release -p kessel-frame-ring --lib
        lib=${CARGO_TARGET_DIR:-target}/release/libkessel_frame_ring.a
        stdlib=$host_sysroot/lib/rustlib/$target/lib
    else
        if [ ! -d "$root/lib/rustlib/$target/lib" ]; then
            work=$(mktemp -d)
            curl -sSfL "https://static.rust-lang.org/dist/rust-std-$version-$target.tar.xz" -o "$work/std.tar.xz"
            tar -xJf "$work/std.tar.xz" -C "$work"
            mkdir -p "$root/lib/rustlib"
            cp -R "$work/rust-std-$version-$target/rust-std-$target/lib/rustlib/$target" "$root/lib/rustlib/"
            rm -rf "$work"
        fi
        RUSTFLAGS="--sysroot=$root ${RUSTFLAGS:-}" cargo build --locked --release -p kessel-frame-ring --lib --target "$target"
        lib=${CARGO_TARGET_DIR:-target}/$target/release/libkessel_frame_ring.a
        stdlib=$root/lib/rustlib/$target/lib
    fi
    ls -l "$lib"
    case "$target" in
        *apple*) prefix=_ ;;
        *) prefix= ;;
    esac

    # Every member of the standard library's rlibs for this target.
    std_members=$(for r in "$stdlib"/*.rlib; do ar t "$r"; done | sort -u)
    [ -n "$std_members" ] || fail "$target: no standard library in $stdlib"
    # Each archive member: this crate's, the libc crate's, or the standard
    # library's (by its name, or by its name after a 16-hex-digit prefix
    # rustc adds to native objects it bundles).
    members=$(ar t "$lib" | grep -v '^__\.SYMDEF' | grep -v '^/' || true)
    ours=0
    std=0
    other=0
    for m in $members; do
        case "$m" in
            kessel_frame_ring-*) ours=$((ours + 1)) ;;
            *)
                bare=$(echo "$m" | sed 's/^[0-9a-f]\{16\}-//')
                if echo "$std_members" | grep -qxF "$m" || echo "$std_members" | grep -qxF "$bare"; then
                    std=$((std + 1))
                else
                    case "$m" in
                        libc-*) other=$((other + 1)) ;;
                        *) fail "$target: $m is not this crate's, libc's or the standard library's" ;;
                    esac
                fi
                ;;
        esac
    done
    echo "-- members: $ours this crate's, $other the libc crate's, $std the standard library's"
    [ "$ours" -gt 0 ] || fail "$target: no member from this crate"

    # Global symbols, POSIX format: "lib[member]: name type ...". Every
    # member must be readable.
    errors=$(mktemp)
    syms=$("$nm_bin" -g -P -A "$lib" 2>"$errors" | sed -n 's/^[^[]*\[\([^]]*\)\]: \([^ ]*\) \([A-Za-z]\).*/\1 \2 \3/p')
    # A member without global symbols is fine ("no symbols"); anything else
    # nm says is a member it couldn't read.
    if grep -v ": no symbols$" "$errors" | grep -q .; then
        fail "$target: nm could not read every member: $(grep -v ": no symbols$" "$errors" | head -n 3)"
    fi
    rm -f "$errors"
    [ -n "$syms" ] || fail "$target: nm listed no symbols"
    # One line per symbol: crate, name without the platform's prefix, type.
    table=$(echo "$syms" | while read -r member name type; do
        name=${name#"$prefix"}
        echo "${member%%-*} $name $type"
    done)
    # C-named means not Rust-mangled (_ZN legacy, _R v0).
    c_named() {
        grep -v ' _ZN' | grep -v ' _R' || true
    }
    # Defined: not U (undefined) or w/v (weak undefined).
    defined_c=$(echo "$table" | grep -v ' [Uwv]$' | c_named)
    defs=$(echo "$defined_c" | awk '$1 == "kessel_frame_ring" { print $2 }' | sort -u)
    echo "-- C symbols this crate defines: $(echo "$defs" | tr '\n' ' ')"
    for s in $defs; do
        in_list "$s" "$c_surface $compiler_defs" || fail "$target: this crate defines $s, not in the C surface"
    done
    for s in $c_surface; do
        echo "$defs" | grep -qx "$s" || fail "$target: $s is missing"
    done
    from_libc=$(echo "$defined_c" | awk '$1 == "libc" { print $2 }' | sort -u)
    [ -z "$from_libc" ] || fail "$target: the libc crate defines C symbols: $(echo "$from_libc" | tr '\n' ' ')"
    imports=$(echo "$table" | awk '$1 == "kessel_frame_ring" && $3 ~ /^[Uwv]$/ { print $2 }' | grep -v '^_ZN' | grep -v '^_R' | sort -u || true)
    echo "-- C functions this crate calls: $(echo "$imports" | tr '\n' ' ')"
    for s in $imports; do
        in_list "$s" "$allowed_imports" || fail "$target: this crate calls $s"
    done
done
[ $status = 0 ] && echo "== the static library holds only this crate, libc's declarations and Rust's standard library"
exit $status
