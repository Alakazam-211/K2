#!/bin/sh
# Fail if a tracked file holds a host's private forms: a blob scheme or person keys of the kind one
# host stores. This repo is written to be open-sourced, and the conversion pairs in vectors/widget
# use neutral forms, which a host swaps for its own. POSIX sh, grep and sed only.
#
#   scripts/check-host-forms.sh [ROOT]       (default: the repo root)
#
# Files: `git ls-files` when ROOT is a git checkout, else every file under ROOT. Binary files are
# skipped, and so are this script and its self-test, which hold the patterns.
# Exit 0 clean, 1 findings (each printed as PATH:LINE:TEXT), 2 usage.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/.." && pwd)}
[ -d "$ROOT" ] || { echo "check-host-forms: no directory $ROOT" >&2; exit 2; }
cd "$ROOT"
PATTERNS='k2blob|k2-board|person:[a-z0-9]|agent:[a-z0-9]'
LIST=$(mktemp); HITS=$(mktemp)
trap 'rm -f "$LIST" "$HITS"' EXIT
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git ls-files > "$LIST"
else
  find . -type f ! -path './target/*' ! -path './.git/*' | sed 's|^\./||' > "$LIST"
fi
FILES=$(wc -l < "$LIST" | tr -d ' ')
[ "$FILES" -gt 0 ] || { echo "check-host-forms: no files in $ROOT" >&2; exit 2; }
while IFS= read -r f; do
  case $f in scripts/check-host-forms.sh|scripts/test-host-forms.sh) continue ;; esac
  [ -f "$f" ] || continue
  grep -nIE "$PATTERNS" "$f" 2>/dev/null | cut -c1-160 | sed "s|^|$f:|" >> "$HITS" || true
done < "$LIST"
if [ -s "$HITS" ]; then
  cat "$HITS"
  echo "check-host-forms: $(wc -l < "$HITS" | tr -d ' ') line(s) hold a host's private form ($FILES files checked)" >&2
  exit 1
fi
echo "check-host-forms: clean ($FILES files checked)"
