#!/usr/bin/env bash
# The brand mark is static data (K2's ruling): only its own reader reads it.
#
#   bash scripts/brand/mark-static-gate.sh [<repo>]
#
# In the Rust sources (crates/, src-tauri/src):
#   - the bytes (BRAND_MARK) and the reader (marked_brand_id) are named only
#     in the mark's own file (crates/k2-brand/src/mark.rs) and the one
#     module that turns the id into a header, an environment value and a
#     warning line (crates/k2-client/src/brand/session.rs);
#   - k2_brand::mark::keep() is called only in a program's src/main.rs (or
#     the mark's own tests), and returns nothing;
#   - a helper program (k2-ptyd, k2-node, k2-app-engine, k2-root-helper,
#     and the protocol crates beside them) names the mark in no other way.
# Exit 0 clean, 1 with every line that breaks a rule.
set -euo pipefail
ROOT="${1:-$(cd "$(dirname "$0")/../.." && pwd)}"
cd "$ROOT"
OWN="crates/k2-brand/src/mark.rs"
READER="crates/k2-client/src/brand/session.rs"
HELPERS="crates/k2-ptyd crates/k2-node crates/k2-node-proto crates/k2-app-engine crates/k2-root-helper crates/k2-root-proto"
bad=0
say() { echo "mark-static-gate: $*" >&2; bad=1; }
files() { find crates src-tauri/src -name '*.rs' -not -path '*/target/*' 2>/dev/null | LC_ALL=C sort; }

while IFS= read -r f; do
    case "$f" in "$OWN"|"$READER") continue ;; esac
    while IFS= read -r hit; do
        say "$f:$hit: names the mark's bytes or its reader (only $OWN and $READER may)"
    done < <(grep -nE '\b(BRAND_MARK|marked_brand_id|BRAND_MARK_PREFIX)\b|K5_BRAND_MARK' "$f" | grep -vE '^[0-9]+:\s*//' || true)
    while IFS= read -r hit; do
        case "$f" in */src/main.rs) continue ;; esac
        say "$f:$hit: keeps the mark outside a program's main.rs"
    done < <(grep -nE 'mark::keep\(' "$f" | grep -vE '^[0-9]+:\s*//' || true)
done < <(files)

for h in $HELPERS; do
    [ -d "$h" ] || continue
    while IFS= read -r hit; do
        line="${hit#*:}"; line="${line#*:}"
        trimmed="$(printf '%s' "$line" | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')"
        case "$trimmed" in
            "k2_brand::mark::keep();"|//*) ;;
            *) say "$hit: a helper names the mark other than k2_brand::mark::keep()" ;;
        esac
    done < <(grep -rnE '\bmark::|k2_brand::mark' "$h" --include='*.rs' || true)
done

grep -qE '^pub fn keep\(\) \{$' "$OWN" || say "$OWN: no pub fn keep() returning nothing"
if [ "$bad" = 0 ]; then echo "mark-static-gate: ok (the mark is read only by $OWN and $READER)"; fi
exit "$bad"
