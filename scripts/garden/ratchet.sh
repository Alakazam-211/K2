#!/usr/bin/env bash
# The size ratchet: the biggest source files may only get smaller.
#
#   scripts/garden/ratchet.sh             check; exit 1 if any file is over
#   scripts/garden/ratchet.sh --tighten   lower each ceiling to today's count
#                                         (never raises one) and drop rows
#                                         for files that are gone or small
#   scripts/garden/ratchet.sh --release PATH
#                                         a held file's lane has landed: move
#                                         it into the list at today's count
#
# scripts/garden/ratchet-ceilings.txt holds one "<lines> <path>" row per
# big file. Two rules:
#   1. A listed file may not have more lines than its ceiling.
#   2. A source file that is not listed may not reach LIMIT lines.
# Test files, vendored code and generated code are not counted.
# A "held <path> <lane>" row skips both rules for a file whose lane is
# still adding to it. Whoever lands that lane runs --release on it.
#
# Why: these files are where every lane collides. They are being split a
# piece at a time, and a split is lost if the file grows back. When this
# fails, put the new code in a new file or in the area's own file.
#
# For tests: --root DIR and --ceilings FILE point it at another tree.
set -euo pipefail

LIMIT=4000
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CEILINGS=""
TIGHTEN=0
RELEASE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --tighten) TIGHTEN=1 ;;
        --release) RELEASE="$2"; shift ;;
        --root) ROOT="$2"; shift ;;
        --ceilings) CEILINGS="$2"; shift ;;
        *) echo "ratchet: unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done
[ -n "$CEILINGS" ] || CEILINGS="$ROOT/scripts/garden/ratchet-ceilings.txt"
[ -f "$CEILINGS" ] || { echo "ratchet: no ceilings file at $CEILINGS" >&2; exit 2; }
cd "$ROOT"

# The files rule 2 looks at: source the lanes edit by hand.
counted_files() {
    # The CLI is counted by its parts (cli/src); cli/k2 is their join.
    git ls-files -- 'cli/src/*.sh' 'crates/*.rs' 'src/*.ts' 'src/*.tsx' 'src-tauri/*.rs' \
        | grep -v -E '(^|/)(tests|generated|dist|migrations)/|(^|/)tests\.rs$|_tests?\.rs$|\.test\.tsx?$|\.generated\.'
}

lines_of() { wc -l < "$1" | tr -d ' '; }

listed="$(mktemp)"; next="$(mktemp)"
trap 'rm -f "$listed" "$next"' EXIT
fail=0
notes=0
held=""

if [ -n "$RELEASE" ]; then
    grep -q -E "^held $RELEASE( |\$)" "$CEILINGS" || { echo "ratchet: $RELEASE is not held in $CEILINGS" >&2; exit 2; }
    [ -f "$RELEASE" ] || { echo "ratchet: $RELEASE does not exist" >&2; exit 2; }
    now="$(lines_of "$RELEASE")"
    # Out of the held rows, into the list before the first smaller ceiling.
    awk -v f="$RELEASE" -v n="$now" '
        $1 == "held" && $2 == f { next }
        !done && $1 ~ /^[0-9]+$/ && $1 + 0 < n + 0 { print n " " f; done = 1 }
        $1 == "held" && !done { print n " " f; done = 1 }
        { print }
        END { if (!done) print n " " f }' "$CEILINGS" > "$next"
    cat "$next" > "$CEILINGS"
    echo "ratchet: released $RELEASE into the list at $now lines."
    exit 0
fi

while IFS= read -r row || [ -n "$row" ]; do
    case "$row" in ''|'#'*) [ "$TIGHTEN" = 1 ] && printf '%s\n' "$row" >> "$next"; continue ;; esac
    if [ "${row%% *}" = held ]; then
        rest="${row#held }"; file="${rest%% *}"; lane="${rest#* }"
        if [ -z "$file" ] || [ "$lane" = "$rest" ] || [ -z "$lane" ]; then
            echo "ratchet: bad row in $CEILINGS (want: held <path> <lane>): $row" >&2; exit 2
        fi
        printf '%s\n' "$file" >> "$listed"
        held="$held${held:+, }$file ($lane)"
        [ "$TIGHTEN" = 1 ] && printf '%s\n' "$row" >> "$next"
        continue
    fi
    ceiling="${row%% *}"
    file="${row#* }"
    case "$ceiling" in ''|*[!0-9]*) echo "ratchet: bad row in $CEILINGS: $row" >&2; exit 2 ;; esac
    printf '%s\n' "$file" >> "$listed"
    if [ ! -f "$file" ]; then
        if [ "$TIGHTEN" = 1 ]; then echo "ratchet: dropped $file (gone)"; continue; fi
        echo "ratchet: FAIL $file is listed but does not exist. Remove its row (or run --tighten)." >&2
        fail=1; continue
    fi
    now="$(lines_of "$file")"
    if [ "$now" -gt "$ceiling" ]; then
        echo "ratchet: FAIL $file has $now lines, its ceiling is $ceiling (+$((now - ceiling))). Put new code in a new file or the area file." >&2
        fail=1
        [ "$TIGHTEN" = 1 ] && printf '%s %s\n' "$ceiling" "$file" >> "$next"
    elif [ "$TIGHTEN" = 1 ]; then
        if [ "$now" -lt "$LIMIT" ]; then
            echo "ratchet: dropped $file ($now lines, under $LIMIT)"
        else
            [ "$now" -lt "$ceiling" ] && echo "ratchet: $file $ceiling -> $now"
            printf '%s %s\n' "$now" "$file" >> "$next"
        fi
    elif [ "$now" -lt "$ceiling" ]; then
        notes=$((notes + 1))
    fi
done < "$CEILINGS"

while IFS= read -r file; do
    grep -q -x -F -- "$file" "$listed" && continue
    now="$(lines_of "$file")"
    if [ "$now" -ge "$LIMIT" ]; then
        echo "ratchet: FAIL $file has $now lines and is not listed; an unlisted file must stay under $LIMIT. Put new code in a new file or the area file." >&2
        fail=1
    fi
done < <(counted_files)

[ -n "$held" ] && echo "ratchet: held out until their lanes land, not checked: $held"
if [ "$fail" = 1 ]; then exit 1; fi
if [ "$TIGHTEN" = 1 ]; then
    cat "$next" > "$CEILINGS"
    echo "ratchet: ceilings tightened."
else
    [ "$notes" -gt 0 ] && echo "ratchet: $notes file(s) are below their ceiling; run scripts/garden/ratchet.sh --tighten to lock that in."
    echo "ratchet: ok."
fi
