#!/usr/bin/env bash
# release-source-archives.sh — source archives every GitHub release carries
# as assets (release sign-off area).
#
# Earlier K2 releases included libgit2 1.8.1 (resources/THIRD-PARTY-NOTICES.md
# part 3, which also holds the written offer). Each release attaches its
# source:
#   - libgit2-1.8.1-source.tar.gz: an archive of libgit2's v1.8.1 tag, made
#     here with `git archive` from a fresh fetch of the tag. The tag must
#     point at the pinned commit, and the uncompressed `git archive
#     --format=tar` output (deterministic for a commit) must have the pinned
#     sha256; it is then gzipped with no name or time (gzip -n).
#   - libgit2-sys-0.17.0-1.8.1.crate: the exact source those releases built,
#     from crates.io (immutable); its sha256 is the checksum their Cargo.lock
#     pinned.
# release.sh Step 9a fetches both; a failure or a pin mismatch stops the
# release before the tag is pushed.
#
#   source scripts/lib/release-source-archives.sh
#   k2_fetch_source_archives <dir>    # prints "<path>#<label>" per archive
#
# tests/packaging/release-source-archives.sh runs it with a stub curl and a
# local stand-in for the libgit2 repository.

# libgit2: repository, tag, the tag's commit, sha256 of the uncompressed
# `git archive --format=tar --prefix=libgit2-1.8.1/ <commit>`.
K2_LIBGIT2_REPO="https://github.com/libgit2/libgit2.git"
K2_LIBGIT2_TAG="v1.8.1"
K2_LIBGIT2_COMMIT="36f7e21ad757a3dacc58cf7944329da6bc1d6e96"
K2_LIBGIT2_PREFIX="libgit2-1.8.1/"
K2_LIBGIT2_TAR_SHA256="aa9f5c286afe49640c823b5903fa0703f498beda6d3fc49352667988cd903cc4"
K2_LIBGIT2_ASSET="libgit2-1.8.1-source.tar.gz"
K2_LIBGIT2_LABEL="libgit2 1.8.1 source, an archive of the v1.8.1 tag (earlier releases included it)"

# Downloaded archives: asset file name | URL | sha256 | label on the release page
K2_SOURCE_ARCHIVES=(
    "libgit2-sys-0.17.0-1.8.1.crate|https://static.crates.io/crates/libgit2-sys/libgit2-sys-0.17.0%2B1.8.1.crate|10472326a8a6477c3c20a64547b0059e4b0d086869eee31e6d7da728a8eb7224|libgit2-sys 0.17.0+1.8.1 source, as built (.crate)"
)

k2_sha256_of() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        sha256sum "$1" | awk '{print $1}'
    fi
}

# k2_build_libgit2_archive <dir>: fetch the tag, check the commit and the
# tar's sha256, write <dir>/$K2_LIBGIT2_ASSET.
k2_build_libgit2_archive() {
    local dir="$1" repo got
    repo="$(mktemp -d)"
    if ! git -C "$repo" init -q \
        || ! git -C "$repo" fetch -q --depth 1 "$K2_LIBGIT2_REPO" "refs/tags/$K2_LIBGIT2_TAG"; then
        echo "ERROR: could not fetch $K2_LIBGIT2_TAG from $K2_LIBGIT2_REPO" >&2
        rm -rf "$repo"
        return 1
    fi
    got="$(git -C "$repo" rev-parse 'FETCH_HEAD^{commit}')"
    if [ "$got" != "$K2_LIBGIT2_COMMIT" ]; then
        echo "ERROR: $K2_LIBGIT2_TAG in $K2_LIBGIT2_REPO is commit $got; the pin in scripts/lib/release-source-archives.sh says $K2_LIBGIT2_COMMIT" >&2
        rm -rf "$repo"
        return 1
    fi
    git -C "$repo" archive --format=tar --prefix="$K2_LIBGIT2_PREFIX" "$K2_LIBGIT2_COMMIT" > "$dir/$K2_LIBGIT2_ASSET.tar"
    rm -rf "$repo"
    got="$(k2_sha256_of "$dir/$K2_LIBGIT2_ASSET.tar")"
    if [ "$got" != "$K2_LIBGIT2_TAR_SHA256" ]; then
        echo "ERROR: git archive of $K2_LIBGIT2_COMMIT is sha256 $got (uncompressed tar); the pin in scripts/lib/release-source-archives.sh says $K2_LIBGIT2_TAR_SHA256" >&2
        rm -f "$dir/$K2_LIBGIT2_ASSET.tar"
        return 1
    fi
    gzip -n -9 < "$dir/$K2_LIBGIT2_ASSET.tar" > "$dir/$K2_LIBGIT2_ASSET"
    rm -f "$dir/$K2_LIBGIT2_ASSET.tar"
}

k2_fetch_source_archives() {
    local dir="$1" entry name url want label got
    [ -n "$dir" ] || { echo "k2_fetch_source_archives: no folder given" >&2; return 2; }
    mkdir -p "$dir" || return 1
    k2_build_libgit2_archive "$dir" || return 1
    printf '%s#%s\n' "$dir/$K2_LIBGIT2_ASSET" "$K2_LIBGIT2_LABEL"
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
