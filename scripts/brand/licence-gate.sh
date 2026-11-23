#!/usr/bin/env bash
# The licence gate: a partner pipeline runs this on the K5 tag it is about
# to build, before anything is built, and builds only when it passes.
#
#   bash scripts/brand/licence-gate.sh --brand <id> [--tag <tag>] [--report] [--list <file>] [<repo>]
#
# Reads <repo>/Cargo.lock (default: the repo this script is in) and the
# crates a private white-label build can't ship: copyleft-crates.txt beside
# this script, or --list. Each row there is "crate | why | what lifts it",
# or "allowed crate | licence | rule" for bundled C we accept everywhere
# (never refused, named in the report).
#
# When Cargo.lock holds a listed crate, a partner brand (any id but k2 and
# k5) is refused, one line per crate:
#   <brand>: can't build from <tag>: it includes <crate> <version>, which
#   partner builds can't ship. This lifts <what lifts it>.
# and the reason on the next line. Today that is libgit2-sys, until
# gitoxide replaces libgit2. --tag only names the tag in that line (the
# lockfile is always <repo>'s); without it the line says "this tree". Our
# own brands only get the report, and so does --report (CI's brand-xy
# step, which shows the answer without failing on it).
#
# A list row that isn't three fields, or a Cargo.lock that is missing or
# holds no package, fails in every mode. Exit 0 to build (or a report), 1
# to refuse, 2 for a usage error.
set -euo pipefail

usage() { echo "usage: licence-gate.sh --brand <id> [--tag <tag>] [--report] [--list <file>] [<repo>]" >&2; exit 2; }
fail() { echo "licence: REFUSED: $*" >&2; exit 1; }

brand="" tag="this tree" report=no list="" repo=""
while [ $# -gt 0 ]; do
    case "$1" in
        --brand) [ $# -ge 2 ] || usage; brand="$2"; shift 2 ;;
        --tag) [ $# -ge 2 ] && [ -n "$2" ] || usage; tag="$2"; shift 2 ;;
        --report) report=yes; shift ;;
        --list) [ $# -ge 2 ] || usage; list="$2"; shift 2 ;;
        -*) usage ;;
        *) [ -z "$repo" ] || usage; repo="$1"; shift ;;
    esac
done
[[ "$brand" =~ ^[a-z0-9][a-z0-9-]*$ ]] || usage
[ -n "$list" ] || list="$(cd "$(dirname "$0")" && pwd)/copyleft-crates.txt"
[ -n "$repo" ] || repo="$(cd "$(dirname "$0")/../.." && pwd)"
[ -d "$repo" ] || usage

# The list: crate, reason and what lifts it, split by " | ".
[ -f "$list" ] || fail "no list of copyleft crates at $list"
crates=() reasons=() lifts=() allowed=() licences=()
n=0
while IFS= read -r line || [ -n "$line" ]; do
    n=$((n + 1))
    case "$line" in ''|'#'*) continue ;; esac
    kind=refused body="$line"
    case "$line" in "allowed "*) kind=allowed body="${line#allowed }" ;; esac
    c="${body%% | *}" rest="${body#* | }"
    r="${rest%% | *}" l="${rest#* | }"
    if [ "$rest" = "$body" ] || [ "$l" = "$rest" ] || [[ "$l" == *" | "* ]] \
        || ! [[ "$c" =~ ^[A-Za-z0-9_-]+$ ]] || [ -z "$r" ] || [ -z "$l" ]; then
        fail "$list line $n is not \"crate | why | what lifts it\" or \"allowed crate | licence | rule\": $line"
    fi
    for seen in ${crates[@]+"${crates[@]}"} ${allowed[@]+"${allowed[@]}"}; do
        [ "$seen" != "$c" ] || fail "$list names $c twice"
    done
    if [ "$kind" = allowed ]; then
        allowed+=("$c") licences+=("$r")
    else
        crates+=("$c") reasons+=("$r") lifts+=("$l")
    fi
done < "$list"
[ "${#crates[@]}" -gt 0 ] || fail "the list at $list has no refused crate"

# Every "name version" the lockfile holds.
lock="$repo/Cargo.lock"
[ -f "$lock" ] || fail "no Cargo.lock in $repo"
packages="$(awk '
    /^\[\[package\]\]/ { name = "" }
    /^name = "/ { name = $3; gsub(/"/, "", name) }
    /^version = "/ && name != "" { v = $3; gsub(/"/, "", v); print name, v; name = "" }' "$lock")"
[ -n "$packages" ] || fail "$lock holds no package"

refusing=no
case "$brand" in
    k2|k5) prefix="licence: report only (brand $brand is ours): a partner brand" ;;
    *) if [ "$report" = yes ]; then
           prefix="licence: report only (--report): $brand"
       else
           prefix="$brand" refusing=yes
       fi ;;
esac

# versions_of <crate>: its versions in the lockfile, on one line.
versions_of() { printf '%s\n' "$packages" | awk -v c="$1" '$1 == c { print $2 }' | tr '\n' ' ' | sed 's/ $//; s/ /, /g'; }

if [ "${#allowed[@]}" -gt 0 ]; then
    for i in "${!allowed[@]}"; do
        versions="$(versions_of "${allowed[$i]}")"
        [ -z "$versions" ] || echo "licence: allowed: ${allowed[$i]} $versions (${licences[$i]})"
    done
fi

found=0
for i in "${!crates[@]}"; do
    c="${crates[$i]}"
    versions="$(versions_of "$c")"
    [ -n "$versions" ] || continue
    found=$((found + 1))
    echo "$prefix: can't build from $tag: it includes $c $versions, which partner builds can't ship. This lifts ${lifts[$i]}." >&2
    echo "  why: ${reasons[$i]}" >&2
done

if [ "$found" = 0 ]; then
    echo "licence: $lock: none of the ${#crates[@]} refused crate(s) on the list is in it; build it"
    exit 0
fi
[ "$refusing" = no ] || exit 1
echo "licence: $found refused crate(s) in $lock (above); brand $brand is not refused here"
