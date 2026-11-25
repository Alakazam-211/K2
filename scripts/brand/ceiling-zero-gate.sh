#!/usr/bin/env bash
# The ceiling-zero gate: a partner pipeline runs this on the K2 tag it is
# about to build, and builds only when it passes.
#
#   bash scripts/brand/ceiling-zero-gate.sh [--brand <folder>] --tag <tag> [<repo>]
#   bash scripts/brand/ceiling-zero-gate.sh [--brand <folder>] --tree [<repo>]
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
# `0 of N probes fall through: V 0, H 0, F 0, E 0, P 0, R 0`, its header
# is there, and it has no row. Every class counts here, P too (an edge
# that exists only without python3 is still a hand-off to the shell).
#
# R is the run-time class: a form under an environment or input variant
# (a proxy by scheme, a locale, HOME unset, a TTY, an odd argument). It
# counts behind the brand file's cli.no_bash. --brand names the brand's
# folder (or its brand.toml); when cli.no_bash is true there, or left out
# (a brand that leaves it out carries no shell CLI), every R row is a
# refusal, since that build answers each with exit 70. Without --brand
# (our default brand), or for a brand with cli.no_bash = false, R rows
# are listed and not counted, as before the run-time class.
#
# --tag reads the file as the tag has it (`git show`); the tag must be a
# tag of <repo> (default: the repo this script is in). --tree reads the
# working tree (a pipeline that has checked the tag out). Exit 0 to build,
# 1 to refuse, 2 for a usage error. --brand reads TOML with Python 3.11 or
# newer; a brand file it can't read is a refusal.
set -euo pipefail

CEILING="crates/k5-cli/tests/data/native_bar_ceiling.tsv"
# The named, temporary allowance (K2's ruling, 0.46.18): exact rows kept out
# of the ceiling, which stays at zero for every other form. The gate prints
# them and their due line; it refuses an allowance without that line.
ALLOWANCE="crates/k5-cli/tests/data/native_port_allowance.tsv"
ALLOWANCE_DUE="# due: native port; take each row out when its form is native"
usage() { echo "usage: ceiling-zero-gate.sh [--brand <folder>] --tag <tag> [<repo>] | [--brand <folder>] --tree [<repo>]" >&2; exit 2; }
refuse() { echo "ceiling-zero: REFUSED ($where): $*" >&2; exit 1; }

brand="" args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --brand) [ $# -ge 2 ] && [ -n "$2" ] || usage; brand="$2"; shift 2 ;;
        *) args+=("$1"); shift ;;
    esac
done
set -- ${args[@]+"${args[@]}"}

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
else
    where="the working tree"
fi

# The brand: no_bash yes or no, and its id for the lines below.
no_bash=no brand_id="the default brand"
if [ -n "$brand" ]; then
    file="$brand"
    [ -d "$file" ] && file="$file/brand.toml"
    [ -f "$file" ] || refuse "no brand file at $brand"
    read_brand="$(python3 -I - "$file" <<'EOF' 2>&1
import sys, tomllib
try:
    with open(sys.argv[1], "rb") as f:
        doc = tomllib.load(f)
except (OSError, tomllib.TOMLDecodeError) as e:
    print("can't read it: %s" % e)
    sys.exit(1)
cli = doc.get("cli", {})
nb = cli.get("no_bash", True) if isinstance(cli, dict) else None
bid = doc.get("id")
if not isinstance(nb, bool) or not isinstance(bid, str) or not bid:
    print("it has no id, or a cli.no_bash that is not true or false")
    sys.exit(1)
print("%s %s" % ("yes" if nb else "no", bid))
EOF
)" || refuse "the brand file $file: $read_brand"
    no_bash="${read_brand%% *}" brand_id="${read_brand#* }"
fi

if [ "$mode" = --tag ]; then
    git -C "$repo" rev-parse -q --verify "refs/tags/$tag" >/dev/null || refuse "no tag named $tag in $repo"
    text="$(git -C "$repo" show "refs/tags/$tag:$CEILING" 2>/dev/null)" || refuse "the tag has no $CEILING"
    allow="$(git -C "$repo" show "refs/tags/$tag:$ALLOWANCE" 2>/dev/null || true)"
else
    [ -f "$repo/$CEILING" ] || refuse "no $CEILING"
    text="$(cat "$repo/$CEILING")"
    allow="$(cat "$repo/$ALLOWANCE" 2>/dev/null || true)"
fi

summary="$(printf '%s\n' "$text" | grep -E '^# [0-9]+ of [0-9]+ probes fall through: ' || true)"
[ "$(printf '%s' "$summary" | grep -c .)" = 1 ] || refuse "the ceiling has no single summary line"
if ! [[ "$summary" =~ ^#\ ([0-9]+)\ of\ ([0-9]+)\ probes\ fall\ through:\ V\ ([0-9]+),\ H\ ([0-9]+),\ F\ ([0-9]+),\ E\ ([0-9]+),\ P\ ([0-9]+),\ R\ ([0-9]+)$ ]]; then
    refuse "the summary line is not the native bar's: $summary"
fi
fell="${BASH_REMATCH[1]}" probes="${BASH_REMATCH[2]}" run_time="${BASH_REMATCH[8]}"
counts="V ${BASH_REMATCH[3]}, H ${BASH_REMATCH[4]}, F ${BASH_REMATCH[5]}, E ${BASH_REMATCH[6]}, P ${BASH_REMATCH[7]}"
[ "$probes" -gt 0 ] || refuse "the ceiling names no probes at all"
body="$(printf '%s\n' "$text" | grep -v '^#' | grep -v '^$' || true)"
# Shell words, not `| head`: under pipefail a reader that stops early
# kills the writer (SIGPIPE, exit 141) once the ceiling is long.
header="${body%%$'\n'*}"
[ "$header" = "$(printf 'class\tfamily\tform')" ] || refuse "the ceiling's header is not class, family, form: $header"
rows="$(printf '%s\n' "$body" | tail -n +2)"
r_rows="$(printf '%s\n' "$rows" | grep $'^R\t' || true)"
other_rows="$(printf '%s\n' "$rows" | grep -v $'^R\t' | grep . || true)"
nr="$(printf '%s' "$r_rows" | grep -c . || true)"
nother="$(printf '%s' "$other_rows" | grep -c . || true)"

# V, H, F, E and P: for every brand.
if [ "$counts" != "V 0, H 0, F 0, E 0, P 0" ] || [ "$nother" != 0 ]; then
    echo "ceiling-zero: $fell of $probes probes fall through ($counts, R $run_time), $nother row(s) besides R:" >&2
    printf '%s\n' "$other_rows" | awk 'NR <= 20 { print "  " $0 }' >&2
    refuse "every command line must be native before a brand other than ours is built"
fi
nallow="$(printf '%s\n' "$allow" | grep -c '^native_bar	' || true)"
if [ "$nallow" != 0 ]; then
    printf '%s\n' "$allow" | grep -qxF "$ALLOWANCE_DUE" || refuse "$ALLOWANCE has $nallow row(s) and no due line ($ALLOWANCE_DUE)"
    echo "ceiling-zero: $where: plus $nallow form(s) on the named temporary allowance $ALLOWANCE (${ALLOWANCE_DUE#\# due: }):" >&2
    printf '%s\n' "$allow" | grep '^native_bar	' | cut -f3,4 | head -n 80 | sed 's/^/  /' >&2
fi
# R: for a brand with no shell CLI.
if [ "$no_bash" = yes ]; then
    if [ "$run_time" != 0 ] || [ "$nr" != 0 ]; then
        echo "ceiling-zero: $brand_id has no shell CLI (cli.no_bash): R $run_time run-time hand-off(s) would each be refused (exit 70), $nr row(s):" >&2
        printf '%s\n' "$r_rows" | awk 'NR <= 20 { print "  " $0 }' >&2
        refuse "every command line must be native in every environment before a brand with no shell CLI is built"
    fi
    echo "ceiling-zero: $where: 0 of $probes probes fall through ($counts, R 0) for $brand_id, which has no shell CLI; build it"
elif [ "$run_time" = 0 ] && [ "$nr" = 0 ]; then
    echo "ceiling-zero: $where: 0 of $probes probes fall through ($counts, R 0); build it"
else
    echo "ceiling-zero: $where: no counted probe of $probes falls through ($counts); R $run_time run-time hand-off(s) listed, not counted for $brand_id (it has the shell CLI); build it"
fi
