#!/usr/bin/env bash
# Build the Arch/Omarchy pacman package on an Arch host and write
# dist/k2-$VERSION-x86_64.pkg.tar.zst.
#
# Build on the Arch host. The tag workflow uploads
# dist/k2-$VERSION-x86_64.pkg.tar.zst. The Mac does not compile the GUI.
# AUR and pkgs.omarchy.org are out of scope.
#
# Does not install (no pacman -U, no makepkg -i). Smoke install is a
# separate human step (`makepkg -si`). No .sig — pacman is the install path.
#
# Usage:
#   packaging/arch/build-release-pkg.sh [version]
# Version: argument, else $VERSION, else src-tauri/tauri.conf.json (the
# file release.sh bumps — no v prefix, never a hardcoded pkgver).

set -euo pipefail

# Print a resolved x.y.z with no v prefix. Argument overrides $VERSION,
# which overrides the repo version files release.sh writes.
k2_arch_resolve_version() {
    local root="$1"
    local arg="${2:-}"
    local env_ver="${VERSION:-}"
    local chosen="" from=""

    if [ -n "$arg" ] && [ -n "$env_ver" ] && [ "$arg" != "$env_ver" ]; then
        echo "FATAL: VERSION argument (${arg}) disagrees with environment (${env_ver})" >&2
        return 1
    fi
    if [ -n "$arg" ]; then
        chosen="$arg"
        from="argument"
    elif [ -n "$env_ver" ]; then
        chosen="$env_ver"
        from="environment"
    else
        local conf="${root}/src-tauri/tauri.conf.json"
        local pkgjson="${root}/package.json"
        local cli="${root}/cli/k2"
        local pkg_ver cli_ver
        if [ ! -f "$conf" ]; then
            echo "FATAL: no VERSION and missing ${conf}" >&2
            return 1
        fi
        chosen="$(sed -nE 's/.*"version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' "$conf" | head -1)"
        pkg_ver="$(sed -nE 's/.*"version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' "$pkgjson" | head -1)"
        cli_ver="$(sed -nE 's/^K2_CLI_VERSION="([^"]+)".*/\1/p' "$cli" | head -1)"
        if [ -z "$pkg_ver" ] || [ "$pkg_ver" != "$chosen" ]; then
            echo "FATAL: package.json version (${pkg_ver:-empty}) != tauri.conf.json (${chosen:-empty})" >&2
            return 1
        fi
        if [ -z "$cli_ver" ] || [ "$cli_ver" != "$chosen" ]; then
            echo "FATAL: cli/k2 K2_CLI_VERSION (${cli_ver:-empty}) != tauri.conf.json (${chosen:-empty})" >&2
            return 1
        fi
        from="src-tauri/tauri.conf.json"
    fi

    case "$chosen" in
        v*)
            echo "FATAL: VERSION must not have a v prefix (got ${chosen})" >&2
            return 1
            ;;
        [0-9]*.[0-9]*.[0-9]*) ;;
        *)
            echo "FATAL: could not resolve VERSION (from ${from}: '${chosen}')" >&2
            return 1
            ;;
    esac
    case "$chosen" in
        *[!0-9.]*)
            echo "FATAL: VERSION has illegal characters (${chosen})" >&2
            return 1
            ;;
    esac
    printf '%s\n' "$chosen"
}

k2_arch_build_release_pkg() {
    local arg="${1:-}"
    local script_dir root arch_dir version envfile pkgbuild got
    local pkg_paths pkg_path n

    if ! command -v pacman >/dev/null 2>&1; then
        echo "FATAL: build-release-pkg.sh must be executed on Arch (pacman not found)." >&2
        echo "FATAL: the Mac does not compile the GUI. Run this on the Arch host." >&2
        return 1
    fi
    if ! command -v makepkg >/dev/null 2>&1; then
        echo "FATAL: makepkg not found. Install base-devel on the Arch host." >&2
        return 1
    fi

    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    root="$(cd "${script_dir}/../.." && pwd)"
    arch_dir="$script_dir"

    version="$(k2_arch_resolve_version "$root" "$arg")" || return 1
    echo "Arch package version: ${version}"

    envfile="${root}/.env"
    if [ -f "$envfile" ]; then
        set -a
        # shellcheck disable=SC1091
        source "$envfile"
        set +a
    fi
    # shellcheck source=../../scripts/require-mail-oauth-build-env.sh
    source "${root}/scripts/require-mail-oauth-build-env.sh"
    require_mail_oauth_build_env

    pkgbuild="${arch_dir}/PKGBUILD"
    if ! grep -qE '^pkgver=' "$pkgbuild"; then
        echo "FATAL: ${pkgbuild} has no pkgver= line" >&2
        return 1
    fi
    # GNU sed (this script already required pacman). Do not leave a stale pkgver.
    sed -i "s/^pkgver=.*/pkgver=${version}/" "$pkgbuild"
    got="$(grep -E '^pkgver=' "$pkgbuild" | head -1 | cut -d= -f2-)"
    if [ "$got" != "$version" ]; then
        echo "FATAL: PKGBUILD pkgver is '${got}' after update; wanted '${version}'" >&2
        return 1
    fi

    pkg_paths="$(cd "$arch_dir" && makepkg --packagelist)" || {
        echo "FATAL: makepkg --packagelist failed" >&2
        return 1
    }
    n=0
    pkg_path=""
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        n=$((n + 1))
        pkg_path="$line"
    done <<EOF
${pkg_paths}
EOF
    if [ "$n" -ne 1 ] || [ -z "$pkg_path" ]; then
        echo "FATAL: expected one makepkg --packagelist entry, found ${n}" >&2
        return 1
    fi
    case "$pkg_path" in
        /*) ;;
        *) pkg_path="${arch_dir}/${pkg_path}" ;;
    esac

    # -f overwrites a previous package. Do not pass -i/--install. Do not pacman -U.
    (cd "$arch_dir" && makepkg -f)

    if [ ! -s "$pkg_path" ]; then
        echo "FATAL: makepkg did not write ${pkg_path}" >&2
        return 1
    fi

    mkdir -p "${root}/dist"
    cp -f "$pkg_path" "${root}/dist/k2-${version}-x86_64.pkg.tar.zst"
    echo "Wrote dist/k2-${version}-x86_64.pkg.tar.zst"
    echo "The k2-arch tag job uploads dist/k2-${version}-x86_64.pkg.tar.zst."
    echo "The Mac does not compile the GUI."
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    k2_arch_build_release_pkg "$@"
fi
