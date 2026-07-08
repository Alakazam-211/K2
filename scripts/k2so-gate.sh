#!/bin/bash
# k2so-gate — CI guard for the k2so cleanup (PRD prd-k2so-cleanup-v1.md §5).
#
# Phase 1 scope: HOME-ANCHORED `.k2so` path literals are the class that
# broke 0.40.33 (fresh installs dead because a reader hit ~/.k2so before
# the compat symlink existed). This gate makes reintroducing one a CI
# failure, not a code-review hope. Workspace-relative `<project>/.k2so/`
# is a SEPARATE namespace owned by dot_dir_migration and is not matched.
#
# Allowlist: scripts/k2so-allowlist.txt — `path :: substring` per line
# (substring `*` allows the whole file). Every entry carries a reason
# comment. Shrinking the list is always welcome; growing it requires the
# same review this PRD got.
#
# Phase 2 (identifier sweep) will widen the pattern; keep this file the
# single place the policy lives.
set -euo pipefail
cd "$(dirname "$0")/.."

PATTERN='~/\.k2so|\$HOME/\.k2so|HOME.*/\.k2so|home(_dir\(\))?\.join\("\.k2so|join\('"'"'\.k2so|homedir\(\), *'"'"'\.k2so|\{home\}/\.k2so|/tmp/\.k2so'
ALLOWLIST="scripts/k2so-allowlist.txt"

# The gate's own policy files necessarily CONTAIN the forbidden patterns
# (that's their job) — exempt them from their own scan.
hits=$(git grep -InE "$PATTERN" -- src-tauri crates src cli scripts \
  ':!scripts/k2so-gate.sh' ':!scripts/k2so-allowlist.txt' \
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
    echo "k2so-gate VIOLATION: $hit"
    fail=1
  fi
done <<< "$hits"

if [ "$fail" -ne 0 ]; then
  echo ""
  echo "k2so-gate: home-anchored .k2so path literal(s) found outside the"
  echo "allowlist. ~/.k2 is canonical (k2_core::paths::k2_home()); the"
  echo ".k2so symlink is compat-only. See .k2/prds/prd-k2so-cleanup-v1.md."
  exit 1
fi
echo "k2so-gate: clean"
