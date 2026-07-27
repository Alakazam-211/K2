#!/bin/bash
# k2-home-gate — CI guard: no new home-anchored ~/.k2so path literals.
#
# Canonical home is ~/.k2 (k2_core::paths::k2_home()). The ~/.k2so name is
# compat-only (symlink / dual-read). This gate keeps the 0.40.33 class of
# bug (hardcoded ~/.k2so before the symlink exists) from coming back.
#
# Policy: .k2/prds/prd-k2so-cleanup-v1.md §5 + prd-k2so-endgame-v1.md.
# Allowlist: scripts/k2-home-allowlist.txt — `path :: substring` per line
# (`*` = whole file). Every entry needs a reason. Prefer shrinking.
set -euo pipefail
cd "$(dirname "$0")/.."

PATTERN='~/\.k2so|\$HOME/\.k2so|HOME.*/\.k2so|home(_dir\(\))?\.join\("\.k2so|join\('"'"'\.k2so|homedir\(\), *'"'"'\.k2so|\{home\}/\.k2so|/tmp/\.k2so'
ALLOWLIST="scripts/k2-home-allowlist.txt"

hits=$(git grep -InE "$PATTERN" -- src-tauri crates src cli scripts \
  ':!scripts/k2-home-gate.sh' ':!scripts/k2-home-allowlist.txt' \
  | grep -vE ':[0-9]+:\s*(//|#|\*|//!|///)' || true)

fail=0
while IFS= read -r hit; do
  [ -z "$hit" ] && continue
  file="${hit%%:*}"
  allowed=0
  while IFS= read -r entry; do
    case "$entry" in ''|'#'*) continue ;; esac
    epath="${entry%% :: *}"
    esub="${entry#* :: }"
    if [ "$file" = "$epath" ]; then
      if [ "$esub" = "*" ] || printf '%s' "$hit" | grep -qF "$esub"; then
        allowed=1
        break
      fi
    fi
  done < "$ALLOWLIST"
  if [ "$allowed" -eq 0 ]; then
    echo "k2-home-gate VIOLATION: $hit"
    fail=1
  fi
done <<< "$hits"

if [ "$fail" -ne 0 ]; then
  echo ""
  echo "k2-home-gate: home-anchored .k2so path literal(s) found outside the"
  echo "allowlist. ~/.k2 is canonical. See .k2/prds/prd-k2so-cleanup-v1.md."
  exit 1
fi
echo "k2-home-gate: clean"
