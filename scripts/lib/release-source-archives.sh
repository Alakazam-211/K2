#!/usr/bin/env bash
# release-source-archives.sh — source archives every GitHub release carries
# as assets (release sign-off area).
#
# Earlier K2 releases included libgit2 1.8.1 (resources/THIRD-PARTY-NOTICES.md
# part 3, which also holds the written offer). Each release attaches its
# source: libgit2 v1.8.1's tarball from GitHub, and the libgit2-sys
# 0.17.0+1.8.1 .crate from crates.io (the exact source those releases built;
# its sha256 is the checksum their Cargo.lock pinned). release.sh Step 9a
# downloads both and checks them against the pins below; a mismatch or a
# failed download stops the release before the tag is pushed.
#
#   source scripts/lib/release-source-archives.sh
#   k2_fetch_source_archives <dir>    # prints "<path>#<label>" per archive
#
# tests/packaging/release-source-archives.sh runs it with a stub curl.

# asset file name | URL | sha256 | label on the release page
K2_SOURCE_ARCHIVES=(
    "libgit2-1.8.1.tar.gz|https://github.com/libgit2/libgit2/archive/refs/tags/v1.8.1.tar.gz|8c1eaf0cf07cba0e9021920bfba9502140220786ed5d8a8ec6c7ad9174522f8e|libgit2 1.8.1 source (earlier releases included it)"
    "libgit2-sys-0.17.0-1.8.1.crate|https://static.crates.io/crates/libgit2-sys/libgit2-sys-0.17.0%2B1.8.1.crate|10472326a8a6477c3c20a64547b0059e4b0d086869eee31e6d7da728a8eb7224|libgit2-sys 0.17.0+1.8.1 source, as built (.crate)"
)

k2_sha256_of() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        sha256sum "$1" | awk '{print $1}'
    fi
}

k2_fetch_source_archives() {
    local dir="$1" entry name url want label got
    [ -n "$dir" ] || { echo "k2_fetch_source_archives: no folder given" >&2; return 2; }
    mkdir -p "$dir" || return 1
    for entry in "${K2_SOURCE_ARCHIVES[@]}"; do
        IFS='|' read -r name url want label <<<"$entry"
        rm -f "$dir/$name.part"
        if ! curl -sSfL --retry 3 --retry-delay 5 -o "$dir/$name.part" "$url"; then
            echo "ERROR: could not download $name from $url" >&2
            rm -f "$dir/$name.part"
            return 1
        fi
        got="$(k2_sha256_of "$dir/$name.part")"
        if [ "$got" != "$want" ]; then
            echo "ERROR: $name from $url is sha256 $got; the pin in scripts/lib/release-source-archives.sh says $want" >&2
            rm -f "$dir/$name.part"
            return 1
        fi
        mv "$dir/$name.part" "$dir/$name"
        printf '%s#%s\n' "$dir/$name" "$label"
    done
}
