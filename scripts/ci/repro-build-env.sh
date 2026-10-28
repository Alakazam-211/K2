#!/usr/bin/env bash
# repro-build-env.sh — the path mappings every Linux release builder uses,
# so GitHub (daemon-binaries.yml, app-linux.yml) and K2 Compute
# (build-linux-bins.sh, build-linux-app.sh) can build the same bytes
# (plan-ci-release-on-k2-compute-v1 G12). Sourced; sets no shell options.
#
#   k2_repro_build_env <source-dir>
#
# Exports RUSTFLAGS (appended) with --remap-path-prefix, and CFLAGS /
# CXXFLAGS (appended) with the matching -ffile-prefix-map for the vendored
# C/C++ deps (ggml/llama.cpp, libgit2, sqlite), so no builder's own paths
# end up in a binary:
#   <source-dir>        → /k2
#   $CARGO_HOME         → /cargo   (registry sources, git deps)
#   $CARGO_TARGET_DIR   → /target  (build-script OUT_DIRs; default
#                                   <source-dir>/target)
# The most specific mapping comes last (rustc, gcc and clang all apply the
# last matching one), so a target dir inside the checkout maps to /target.
# Each path is mapped as given and, when it differs, as its physical path
# (a symlinked checkout or home). The same rustc is still needed for equal
# bytes (K2_COMPUTE_RUST pins the compute side).
#
# Never reads ~/.k2, never talks to a daemon.

k2_repro_build_env() {
    local src="${1:?usage: k2_repro_build_env <source-dir>}"
    local cargo_home="${CARGO_HOME:-$HOME/.cargo}"
    local target="${CARGO_TARGET_DIR:-$src/target}"
    case "$target" in /*) ;; *) target="$src/$target" ;; esac
    local p
    for p in "$src" "$cargo_home" "$target"; do
        case "$p" in
            *[[:space:]]*)
                echo "FATAL: k2_repro_build_env: a build path has whitespace ($p); RUSTFLAGS can't carry it" >&2
                return 1
                ;;
        esac
    done
    local rflags="" cflags="" pair from to phys
    for pair in "$src=/k2" "$cargo_home=/cargo" "$target=/target"; do
        from="${pair%=*}" to="${pair##*=}"
        from="${from%/}"
        rflags="$rflags --remap-path-prefix=$from=$to"
        cflags="$cflags -ffile-prefix-map=$from=$to"
        phys="$(cd "$from" 2>/dev/null && pwd -P)" || phys=""
        if [ -n "$phys" ] && [ "$phys" != "$from" ]; then
            rflags="$rflags --remap-path-prefix=$phys=$to"
            cflags="$cflags -ffile-prefix-map=$phys=$to"
        fi
    done
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS}$rflags"
    export CFLAGS="${CFLAGS:+$CFLAGS}$cflags"
    export CXXFLAGS="${CXXFLAGS:+$CXXFLAGS}$cflags"
    RUSTFLAGS="${RUSTFLAGS# }" CFLAGS="${CFLAGS# }" CXXFLAGS="${CXXFLAGS# }"
    export RUSTFLAGS CFLAGS CXXFLAGS
    echo "  path remap on: $src → /k2, $cargo_home → /cargo, $target → /target (rustc + C/C++)"
}
