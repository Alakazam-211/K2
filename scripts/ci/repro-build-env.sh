#!/usr/bin/env bash
# repro-build-env.sh — what every Linux release builder sets so GitHub
# (daemon-binaries.yml, app-linux.yml) and K2 Compute (build-linux-bins.sh,
# build-linux-app.sh) build the same bytes (plan-ci-release-on-k2-compute-v1
# G12; prd-ci-release-on-compute-stage1-v1 §6). Sourced; sets no shell
# options.
#
#   k2_repro_build_env <source-dir>
#
# Path mappings, so no builder's own paths end up in a binary:
#   <source-dir>        → /k2
#   $CARGO_HOME         → /cargo   (registry sources, git deps)
#   $CARGO_TARGET_DIR   → /target  (build-script OUT_DIRs; default
#                                   <source-dir>/target)
# Each path is mapped as given and, when it differs, as its physical path
# (a symlinked checkout or home). The most specific mapping comes last
# (rustc, gcc and clang apply the last matching one), so a target dir
# inside the checkout maps to /target.
#
#   rustc: --remap-path-prefix, appended to RUSTFLAGS.
#   C/C++: -ffile-prefix-map, added by small compiler wrappers put first on
#          PATH (cc, c++, gcc, g++, and any bare-named compiler in CC, CXX,
#          CC_<triple>, CXX_<triple>), NOT through CFLAGS. Vendored OpenSSL
#          (git2's vendored-openssl) bakes "compiler: cc <CFLAGS>" into the
#          daemon, so maps in CFLAGS put each job's own folder back into the
#          binary (2026-10-29 shadow: L1 and V1 daemons differed only there
#          and in the date). The wrappers' folder is never recorded: the
#          compiler stays plain "cc".
#   SOURCE_DATE_EPOCH: the checkout's commit time (git; else
#          BUILD_SOURCE_DATE_EPOCH), unless already set. OpenSSL's
#          "built on:" and other build dates use it instead of now.
#
# The same rustc is still needed for equal bytes (rust-toolchain.toml).
# Never reads ~/.k2, never talks to a daemon.

# The real compiler <name> on PATH, skipping the wrappers' own folder.
_k2_repro_real_tool() {
    local name="$1" skip="$2" d
    local IFS=:
    for d in $PATH; do
        [ -n "$d" ] && [ "$d" != "$skip" ] || continue
        if [ -x "$d/$name" ] && [ ! -d "$d/$name" ]; then
            printf '%s\n' "$d/$name"
            return 0
        fi
    done
    return 1
}

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
    RUSTFLAGS="${RUSTFLAGS# }"
    export RUSTFLAGS

    # C/C++: wrappers first on PATH. Their folder is per build (TMPDIR) and
    # appears in no binary.
    local wdir names name real v
    wdir="$(mktemp -d "${TMPDIR:-/tmp}/k2-repro-cc.XXXXXX")" || return 1
    names="cc c++ gcc g++"
    for v in CC CXX $(env | sed -n -E 's/^((CC|CXX)_[A-Za-z0-9_]+)=.*/\1/p'); do
        name="${!v:-}"
        case "$name" in ""|*/*|*[[:space:]]*) continue ;; esac
        names="$names $name"
    done
    for name in $names; do
        [ -e "$wdir/$name" ] && continue
        real="$(_k2_repro_real_tool "$name" "$wdir")" || continue
        # shellcheck disable=SC2086 # cflags is a list of single words (no whitespace, checked above)
        {
            printf '#!/bin/sh\n# k2_repro_build_env: %s with the path maps (scripts/ci/repro-build-env.sh)\n' "$name"
            printf 'exec %s' "'$real'"
            for p in $cflags; do printf " '%s'" "$p"; done
            printf ' "$@"\n'
        } > "$wdir/$name"
        chmod +x "$wdir/$name"
    done
    export PATH="$wdir:$PATH"
    export K2_REPRO_CC_DIR="$wdir"

    if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
        SOURCE_DATE_EPOCH="$(git -c safe.directory='*' -C "$src" log -1 --format=%ct 2>/dev/null || true)"
        [ -n "$SOURCE_DATE_EPOCH" ] || SOURCE_DATE_EPOCH="${BUILD_SOURCE_DATE_EPOCH:-}"
    fi
    if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
        export SOURCE_DATE_EPOCH
    else
        echo "  ⚠ k2_repro_build_env: no commit time (no git, no BUILD_SOURCE_DATE_EPOCH): build dates will differ" >&2
    fi
    echo "  path remap on: $src → /k2, $cargo_home → /cargo, $target → /target (rustc flags; C/C++ via $wdir); SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-unset}"
}
