#!/usr/bin/env bash
# linux-sums.sh — keep a release's SHA256SUMS-linux-x86_64.txt complete.
#
# Two workflows write Linux packages onto the same GitHub release after
# the tag, on different runners and in either order:
#   app-linux.yml    (k2-linux) K2_<v>_amd64.deb, K2-<v>-1.x86_64.rpm,
#                               K2_<v>_amd64.AppImage
#   arch-package.yml (k2-arch)  k2-<v>-x86_64.pkg.tar.zst
# Both merge their own lines into the one SUMS file with `merge`: download
# the release's file, replace or append their lines, keep every other
# line, re-upload with --clobber, read it back. The merge jobs share the
# GitHub concurrency group k2-linux-sums-<tag>, so two read-modify-writes
# never overlap. app-linux may create the file (--create-if-missing).
# arch-package must find it (it `wait`s for app-linux first) and fails
# loudly when it never appears.
#
# The desktop updater (src-tauri linux package install) refuses a package
# whose name has no line here, so a missing line blocks in-app updates.
#
# Usage:
#   linux-sums.sh merge-text <existing-file|-> <new-lines-file>
#       Pure merge to stdout (no gh). '-' = no existing file.
#   linux-sums.sh line <file>
#       Print "<sha256>  <basename>" for a local file.
#   linux-sums.sh wait <tag> [minutes]
#       Wait until the release has the SUMS asset (default 180 minutes).
#   linux-sums.sh merge <tag> <new-lines-file> [--create-if-missing]
#       Merge new lines into the release's SUMS file and re-upload.
#   linux-sums.sh check <version> [--wait-min N]
#       Post-publish check (release.sh): the four Linux packages exist,
#       every Linux package asset has a SUMS line, and each line matches
#       GitHub's own asset digest. Waits up to N minutes (default 0) for
#       the workflows to finish.
#
# Environment: K2_RELEASE_REPO (default Alakazam-211/K2),
#              K2_SUMS_POLL_SECS (default 30; tests set 0).
set -euo pipefail

REPO="${K2_RELEASE_REPO:-Alakazam-211/K2}"
SUMS_NAME="SHA256SUMS-linux-x86_64.txt"
POLL="${K2_SUMS_POLL_SECS:-30}"
# One "waiting for …" line, then dots, a status line every few minutes.
# shellcheck source=lib/wait-progress.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/wait-progress.sh"
WORK_DIRS=()
cleanup() {
    local d
    for d in ${WORK_DIRS[@]+"${WORK_DIRS[@]}"}; do rm -rf "$d"; done
}
trap cleanup EXIT

log() { printf '[linux-sums] %s\n' "$*" >&2; }
die() { printf '[linux-sums] FATAL: %s\n' "$*" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "need sha256sum or shasum"
    fi
}

# A SUMS line is "<64 lowercase hex><two spaces or space-star><name>" with a
# plain file name: no '/', no leading '.', nothing but [A-Za-z0-9._+-].
# Prints the normalized "<hash>  <name>" or fails.
normalize_line() {
    local line="$1" hash name
    if [[ "$line" =~ ^([0-9a-f]{64})(\ \ |\ \*)([A-Za-z0-9_+][A-Za-z0-9._+-]*)$ ]]; then
        hash="${BASH_REMATCH[1]}"
        name="${BASH_REMATCH[3]}"
        printf '%s  %s\n' "$hash" "$name"
        return 0
    fi
    return 1
}

# merge_text <existing|-> <new> → merged text on stdout. Existing lines keep
# their order; a new line replaces the existing line with the same name in
# place; new names are appended. Any malformed or duplicate line fails.
# Portable to macOS bash 3.2 (no associative arrays): validate in bash,
# merge in awk.
merge_text() {
    local existing="$1" new="$2" raw norm tmpd
    tmpd="$(mktemp -d)"
    WORK_DIRS+=("$tmpd")
    : > "$tmpd/old"
    : > "$tmpd/new"
    if [ "$existing" != "-" ]; then
        [ -f "$existing" ] || die "existing SUMS file missing: $existing"
        while IFS= read -r raw || [ -n "$raw" ]; do
            [ -n "$raw" ] || continue
            norm="$(normalize_line "$raw")" || die "malformed line in existing ${SUMS_NAME}: '$raw'"
            printf '%s\n' "$norm" >> "$tmpd/old"
        done < "$existing"
    fi
    [ -f "$new" ] || die "new lines file missing: $new"
    while IFS= read -r raw || [ -n "$raw" ]; do
        [ -n "$raw" ] || continue
        norm="$(normalize_line "$raw")" || die "malformed new line: '$raw'"
        printf '%s\n' "$norm" >> "$tmpd/new"
    done < "$new"
    [ -s "$tmpd/new" ] || die "no new lines to merge"
    local dup
    dup="$(awk '{print $2}' "$tmpd/old" | sort | uniq -d | head -1)"
    [ -z "$dup" ] || die "duplicate name in existing ${SUMS_NAME}: $dup"
    dup="$(awk '{print $2}' "$tmpd/new" | sort | uniq -d | head -1)"
    [ -z "$dup" ] || die "duplicate name in new lines: $dup"
    awk '
        FNR == NR { repl[$2] = $0; norder[++n] = $2; next }
        { if ($2 in repl) { print repl[$2]; used[$2] = 1 } else { print } }
        END { for (i = 1; i <= n; i++) if (!(norder[i] in used)) print repl[norder[i]] }
    ' "$tmpd/new" "$tmpd/old"
}

# Download the release's SUMS file into dir $2. Returns 1 when the release
# has no such asset; any other gh failure is fatal.
download_sums() {
    local tag="$1" dir="$2" err
    if gh release view "$tag" --repo "$REPO" --json assets --jq '.assets[].name' \
        | grep -qxF "$SUMS_NAME"; then
        err="$(gh release download "$tag" --repo "$REPO" --pattern "$SUMS_NAME" \
            --dir "$dir" --clobber 2>&1)" || die "gh release download ${SUMS_NAME} from ${tag} failed: ${err}"
        [ -f "$dir/$SUMS_NAME" ] || die "gh release download reported success but ${dir}/${SUMS_NAME} is missing"
        return 0
    fi
    return 1
}

cmd_merge() {
    local tag="${1:-}" new="${2:-}" create=0
    [ -n "$tag" ] && [ -n "$new" ] || die "usage: linux-sums.sh merge <tag> <new-lines-file> [--create-if-missing]"
    [ "${3:-}" = "--create-if-missing" ] && create=1
    [ -s "$new" ] || die "new lines file is missing or empty: $new"
    local work
    work="$(mktemp -d)"
    WORK_DIRS+=("$work")
    mkdir -p "$work/in" "$work/out"
    local existing="-"
    if download_sums "$tag" "$work/in"; then
        existing="$work/in/$SUMS_NAME"
        log "merging into ${tag}/${SUMS_NAME} ($(grep -c . "$existing" || true) existing lines)"
    elif [ "$create" = 1 ]; then
        log "${tag} has no ${SUMS_NAME} yet; creating it"
    else
        die "${tag} has no ${SUMS_NAME}. app-linux.yml writes it; run that first (or re-run this job after it). Not creating a partial file."
    fi
    merge_text "$existing" "$new" > "$work/out/$SUMS_NAME"
    gh release upload "$tag" "$work/out/$SUMS_NAME" --repo "$REPO" --clobber
    # Read it back: a clobber from a writer outside the concurrency group
    # (a hand upload) would drop our lines. Say so instead of passing.
    rm -rf "$work/in" && mkdir -p "$work/in"
    download_sums "$tag" "$work/in" || die "${SUMS_NAME} vanished from ${tag} right after upload"
    local raw norm
    while IFS= read -r raw || [ -n "$raw" ]; do
        [ -n "$raw" ] || continue
        norm="$(normalize_line "$raw")"
        grep -qxF "$norm" "$work/in/$SUMS_NAME" \
            || die "after upload, ${tag}/${SUMS_NAME} lacks: ${norm}"
    done < "$new"
    log "ok: ${tag}/${SUMS_NAME} now has $(grep -c . "$work/in/$SUMS_NAME") lines"
    cat "$work/in/$SUMS_NAME"
}

cmd_wait() {
    local tag="${1:-}" minutes="${2:-180}"
    [ -n "$tag" ] || die "usage: linux-sums.sh wait <tag> [minutes]"
    local deadline=$(( $(date +%s) + minutes * 60 ))
    while :; do
        if gh release view "$tag" --repo "$REPO" --json assets --jq '.assets[].name' \
            | grep -qxF "$SUMS_NAME"; then
            k2_wait_done
            log "${tag} has ${SUMS_NAME}"
            return 0
        fi
        if [ "$(date +%s)" -ge "$deadline" ]; then
            k2_wait_done
            die "${tag} still has no ${SUMS_NAME} after ${minutes} min. app-linux.yml did not finish; re-run it, then re-run this job."
        fi
        k2_wait_tick "[linux-sums] waiting for app-linux.yml to upload ${SUMS_NAME} to ${tag}..."
        sleep "$POLL"
    done
}

# Linux package assets for <version>, by exact name.
expected_assets() {
    local v="$1"
    printf '%s\n' "K2_${v}_amd64.deb" "K2-${v}-1.x86_64.rpm" \
        "K2_${v}_amd64.AppImage" "k2-${v}-x86_64.pkg.tar.zst"
}

is_linux_package_name() {
    case "$1" in
        *.deb|*.rpm|*.AppImage|*.pkg.tar.zst) return 0 ;;
    esac
    return 1
}

# One pass of the check. Prints problems to stderr. Returns 0 (complete and
# correct), 1 (wrong: a line is missing for a present asset, or a hash
# disagrees) or 2 (incomplete: an expected asset or the SUMS file is not
# up yet; worth waiting for).
check_once() {
    local version="$1" work="$2" listing="$work/assets.tsv"
    gh api "repos/${REPO}/releases/tags/v${version}" \
        --jq '.assets[] | [.name, (.digest // "")] | @tsv' > "$listing" \
        || die "gh api releases/tags/v${version} failed"
    local missing=0 wrong=0 name digest line hash
    for name in $(expected_assets "$version"); do
        if ! cut -f1 "$listing" | grep -qxF "$name"; then
            log "not on the release yet: ${name}"
            missing=1
        fi
    done
    rm -f "$work/$SUMS_NAME"
    if ! cut -f1 "$listing" | grep -qxF "$SUMS_NAME"; then
        log "not on the release yet: ${SUMS_NAME}"
        return 2
    fi
    download_sums "v${version}" "$work" || return 2
    while IFS= read -r line || [ -n "$line" ]; do
        [ -n "$line" ] || continue
        normalize_line "$line" >/dev/null || { log "malformed SUMS line: '$line'"; wrong=1; }
    done < "$work/$SUMS_NAME"
    while IFS=$'\t' read -r name digest; do
        is_linux_package_name "$name" || continue
        line="$(awk -v n="$name" '{ f=$2; sub(/^\*/, "", f); if (f == n) print $1 }' "$work/$SUMS_NAME")"
        if [ -z "$line" ]; then
            log "no ${SUMS_NAME} line for ${name}"
            # The writer may still be merging; app-linux and arch both
            # upload the asset before their SUMS merge.
            missing=1
            continue
        fi
        if [ "$(printf '%s\n' "$line" | wc -l | tr -d ' ')" != 1 ]; then
            log "more than one ${SUMS_NAME} line for ${name}"
            wrong=1
            continue
        fi
        hash="$line"
        if [ -n "$digest" ]; then
            if [ "sha256:${hash}" != "$digest" ]; then
                log "${name}: SUMS says ${hash}, GitHub digest says ${digest#sha256:}"
                wrong=1
            fi
        else
            log "WARNING: GitHub reports no digest for ${name}; line present, hash not cross-checked"
        fi
    done < "$listing"
    [ "$wrong" = 0 ] || return 1
    [ "$missing" = 0 ] || return 2
    return 0
}

cmd_check() {
    local version="${1:-}" wait_min=0
    [ -n "$version" ] || die "usage: linux-sums.sh check <version> [--wait-min N]"
    version="${version#v}"
    if [ "${2:-}" = "--wait-min" ]; then
        wait_min="${3:?--wait-min needs a number}"
    fi
    local work rc deadline
    work="$(mktemp -d)"
    WORK_DIRS+=("$work")
    deadline=$(( $(date +%s) + wait_min * 60 ))
    # Each pass's findings go to a file: shown on the first and every status
    # line of the wait, and at the end; a dot for the passes in between.
    local pass_err="$work/pass.err"
    while :; do
        rc=0
        check_once "$version" "$work" 2>"$pass_err" || rc=$?
        if [ "$rc" = 0 ]; then
            k2_wait_done
            cat "$pass_err" >&2
            log "ok: every Linux package on v${version} has a matching ${SUMS_NAME} line"
            return 0
        fi
        if [ "$rc" = 1 ]; then
            k2_wait_done
            cat "$pass_err" >&2
            die "v${version} ${SUMS_NAME} is wrong (see above)"
        fi
        if [ "$(date +%s)" -ge "$deadline" ]; then
            k2_wait_done
            cat "$pass_err" >&2
            die "v${version} Linux packages or SUMS lines still missing after ${wait_min} min (see above). Re-run: bash scripts/linux-sums.sh check ${version} --wait-min 60"
        fi
        k2_wait_tick "[linux-sums] waiting for app-linux.yml / arch-package.yml (v${version})..."
        if [ "$K2_WAIT_SAID" = 1 ]; then
            sed 's/^/    /' "$pass_err" >&2
        fi
        sleep "$POLL"
    done
}

cmd_line() {
    local f="${1:-}"
    [ -s "$f" ] || die "line: file missing or empty: $f"
    local name
    name="$(basename "$f")"
    normalize_line "$(sha256_of "$f")  ${name}" || die "line: unusable file name '${name}'"
}

sub="${1:-}"
shift || true
case "$sub" in
    merge-text) merge_text "${1:?existing}" "${2:?new}" ;;
    line) cmd_line "$@" ;;
    wait) cmd_wait "$@" ;;
    merge) cmd_merge "$@" ;;
    check) cmd_check "$@" ;;
    *) die "usage: linux-sums.sh {merge-text|line|wait|merge|check} ..." ;;
esac
