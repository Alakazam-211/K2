#!/usr/bin/env bash
# Required GitHub release asset for the Arch/Omarchy pacman package.
#
# release.sh sources this. An Arch host runs packaging/arch/build-release-pkg.sh
# and the operator copies dist/k2-<version>-x86_64.pkg.tar.zst into $DIST_DIR
# (target/release/daemon-dist) before release.sh. The Mac does not compile
# the GUI. No .sig — pacman is the install path. Absence is FATAL (the linux
# daemon tarballs may be skipped; this file may not). AUR and pkgs.omarchy.org
# are out of scope. Do not add a Linux GUI key to latest.json.

# Print the asset path on stdout (the name release.sh adds to ASSETS).
# On failure, print FATAL to stderr and return 1. Never invokes gh.
k2_require_arch_pkg_asset() {
    local dist_dir="${1:-}"
    local version="${2:-}"
    local name="k2-${version}-x86_64.pkg.tar.zst"
    local path="${dist_dir%/}/${name}"

    if [ -z "$dist_dir" ] || [ -z "$version" ]; then
        echo "FATAL: k2_require_arch_pkg_asset needs DIST_DIR and VERSION" >&2
        return 1
    fi
    case "$version" in
        v*)
            echo "FATAL: Arch package version must not have a v prefix (got ${version}); asset is ${name}" >&2
            return 1
            ;;
    esac
    if [ ! -s "$path" ]; then
        echo "FATAL: missing Arch package ${name} in ${dist_dir} (${path})" >&2
        echo "FATAL: ${name} must be in DIST_DIR before gh release create. No silent skip, no .sig." >&2
        echo "FATAL: build dist/${name} on the Arch host and copy it here. The Mac does not compile the GUI." >&2
        return 1
    fi
    printf '%s\n' "$path"
}
