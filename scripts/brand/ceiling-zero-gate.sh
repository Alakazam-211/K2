#!/usr/bin/env bash
# The ceiling-zero gate: a partner pipeline runs this on the K2 tag it is
# about to build, and builds only when it passes.
#
#   bash scripts/brand/ceiling-zero-gate.sh --tag <tag> [<repo>]
#   bash scripts/brand/ceiling-zero-gate.sh --tree [<repo>]
#
# A brand other than ours ships the k5 binary only, never a brand build of
# the shell CLI (step C7 is skipped, on this condition). So every command
# line must be answered by the binary itself: none may fall through to the
# shell CLI it carries, which still prints our names. The native bar's
# ceiling (crates/k5-cli/tests/data/native_bar_ceiling.tsv, written by
# crates/k5-cli/tests/native_bar.rs, whose test fails when the ceiling and
# the binary disagree) lists every probe that falls through, by class.
#
# Passes only when the ceiling reads 0: its summary line says
# `0 of N probes fall through: V 0, H 0, F 0, E 0, P 0`, its header is
# there, and it has no row. Every class counts here, P too (an edge that
# exists only without python3 is still a hand-off to the shell).
#
# --tag reads the file as the tag has it (`git show`); the tag must be a
# tag of <repo> (default: the repo this script is in). --tree reads the
# working tree (a pipeline that has checked the tag out). Exit 0 to build,
# 1 to refuse, 2 for a usage error.
set -euo pipefail

CEILING="crates/k5-cli/tests/data/native_bar_ceiling.tsv"
# The named, temporary allowance (K2's ruling, 0.46.18): exact rows kept out
# of the ceiling, which stays at zero for every other form. The gate prints
# them and their due line; it refuses an allowance without that line.
ALLOWANCE="crates/k5-cli/tests/data/native_port_allowance.tsv"
ALLOWANCE_DUE="# due: native port due before the n2 build (0.46.19)"
usage() { echo "usage: ceiling-zero-gate.sh --tag <tag> [<repo>] | --tree [<repo>]" >&2; exit 2; }
refuse() { echo "ceiling-zero: REFUSED ($where): $*" >&2; exit 1; }

mode="${1:-}"
case "$mode" in
    --tag) [ $# -ge 2 ] && [ $# -le 3 ] || usage; tag="$2"; repo="${3:-}" ;;
    --tree) [ $# -le 2 ] || usage; repo="${2:-}" ;;
    *) usage ;;
esac
[ -n "$repo" ] || repo="$(cd "$(dirname "$0")/../.." && pwd)"
[ -d "$repo" ] || usage

if [ "$mode" = --tag ]; then
    where="tag $tag"
    git -C "$repo" rev-parse -q --verify "refs/tags/$tag" >/dev/null || refuse "no tag named $tag in $repo"
    text="$(git -C "$repo" show "refs/tags/$tag:$CEILING" 2>/dev/null)" || refuse "the tag has no $CEILING"
    allow="$(git -C "$repo" show "refs/tags/$tag:$ALLOWANCE" 2>/dev/null || true)"
else
    where="the working tree"
    [ -f "$repo/$CEILING" ] || refuse "no $CEILING"
    text="$(cat "$repo/$CEILING")"
    allow="$(cat "$repo/$ALLOWANCE" 2>/dev/null || true)"
fi

summary="$(printf '%s\n' "$text" | grep -E '^# [0-9]+ of [0-9]+ probes fall through: ' || true)"
[ "$(printf '%s' "$summary" | grep -c .)" = 1 ] || refuse "the ceiling has no single summary line"
if ! [[ "$summary" =~ ^#\ ([0-9]+)\ of\ ([0-9]+)\ probes\ fall\ through:\ V\ ([0-9]+),\ H\ ([0-9]+),\ F\ ([0-9]+),\ E\ ([0-9]+),\ P\ ([0-9]+)$ ]]; then
    refuse "the summary line is not the native bar's: $summary"
fi
fell="${BASH_REMATCH[1]}" probes="${BASH_REMATCH[2]}"
counts="V ${BASH_REMATCH[3]}, H ${BASH_REMATCH[4]}, F ${BASH_REMATCH[5]}, E ${BASH_REMATCH[6]}, P ${BASH_REMATCH[7]}"
[ "$probes" -gt 0 ] || refuse "the ceiling names no probes at all"
body="$(printf '%s\n' "$text" | grep -v '^#' | grep -v '^$' || true)"
header="$(printf '%s\n' "$body" | head -n 1)"
[ "$header" = "$(printf 'class\tfamily\tform')" ] || refuse "the ceiling's header is not class, family, form: $header"
rows="$(printf '%s\n' "$body" | tail -n +2)"
nrows="$(printf '%s' "$rows" | grep -c . || true)"
if [ "$fell" != 0 ] || [ "$counts" != "V 0, H 0, F 0, E 0, P 0" ] || [ "$nrows" != 0 ]; then
    echo "ceiling-zero: $fell of $probes probes fall through ($counts), $nrows row(s):" >&2
    printf '%s\n' "$rows" | head -n 20 | sed 's/^/  /' >&2
    refuse "every command line must be native before a brand other than ours is built"
fi
nallow="$(printf '%s\n' "$allow" | grep -c '^native_bar	' || true)"
if [ "$nallow" != 0 ]; then
    printf '%s\n' "$allow" | grep -qxF "$ALLOWANCE_DUE" || refuse "$ALLOWANCE has $nallow row(s) and no due line ($ALLOWANCE_DUE)"
    echo "ceiling-zero: $where: plus $nallow form(s) on the named temporary allowance $ALLOWANCE (${ALLOWANCE_DUE#\# due: }):" >&2
    printf '%s\n' "$allow" | grep '^native_bar	' | cut -f3,4 | head -n 80 | sed 's/^/  /' >&2
fi
echo "ceiling-zero: $where: 0 of $probes probes fall through ($counts); build it"
