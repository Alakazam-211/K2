#!/usr/bin/env bash
# Required GitHub release asset for the Arch/Omarchy pacman package.
#
# release.sh sources this only when a caller wants the strict check.
# The tag job .github/workflows/arch-package.yml builds
# dist/k2-<version>-x86_64.pkg.tar.zst on the k2-arch runner and uploads it
# after the tag, the same way daemon-binaries.yml uploads Linux daemons.
# If the file is already in $DIST_DIR, release.sh attaches it. A missing
# file does not stop the release. The Mac does not compile the GUI.
# No .sig. AUR and pkgs.omarchy.org are out of scope. Do not add a Linux
# GUI key to latest.json.

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
