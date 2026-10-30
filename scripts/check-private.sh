#!/bin/sh
# Fail if a file in the tree names an internal build host: these repos are written to be
# open-sourced, and a host's machines are the host's (k2-mpc-base's tools/README.md, "Build
# machines are the host's"). POSIX sh, grep and awk only. A copy of k2-mpc-base's
# tools/check-private.sh: keep them the same.
#
#   tools/check-private.sh [ROOT]       (default: the repo root)
#
# Files: `git ls-files` when ROOT is a git checkout, else every file under ROOT (a node job's
# unpacked bundle), `target/` and `.git/` left out. Binary files are skipped.
# Patterns: PATTERNS below (extended regexes, case-insensitive).
# Allowlist: private-allow.txt beside this script, one entry per line, `#` comments (this script
# and the allowlist hold the patterns, and are always skipped):
#   PATH          the whole file
#   PATH:TEXT     a matching line of PATH that contains TEXT (fixed string)
# Exit 0 clean, 1 findings (each printed as PATH:LINE:TEXT), 2 usage.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/.." && pwd)}
[ -d "$ROOT" ] || { echo "check-private: no directory $ROOT" >&2; exit 2; }
cd "$ROOT"
PATTERNS='k2n[0-9]|-vin\b|k2-arch|z3mbp|\.k2\.dev\b'
ALLOW=$HERE/private-allow.txt
# this script and the allowlist, as paths under ROOT (skipped: they hold the patterns)
SELF=${HERE#"$(pwd)"/}/check-private.sh
ALLOWREL=${HERE#"$(pwd)"/}/private-allow.txt
LIST=$(mktemp); HITS=$(mktemp)
trap 'rm -f "$LIST" "$HITS"' EXIT
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git ls-files -z | tr '\0' '\n' > "$LIST"
else
  find . -type f ! -path './target/*' ! -path './.git/*' | sed 's|^\./||' > "$LIST"
fi
: > "$HITS"
while IFS= read -r f; do
  [ -f "$f" ] || continue
  grep -nIiE "$PATTERNS" "$f" 2>/dev/null | sed "s|^|$f:|" >> "$HITS" || true
done < "$LIST"
[ -f "$ALLOW" ] || ALLOW=/dev/null
BAD=$(awk -v allow="$ALLOW" -v self="$SELF" -v allowrel="$ALLOWREL" '
  BEGIN {
    whole[self] = 1; whole[allowrel] = 1
    while ((getline l < allow) > 0) {
      sub(/[ \t]+$/, "", l); if (l == "" || l ~ /^#/) continue
      i = index(l, ":")
      if (i == 0) whole[l] = 1; else { n++; ap[n] = substr(l, 1, i - 1); at[n] = substr(l, i + 1) }
    }
  }
  {
    i = index($0, ":"); path = substr($0, 1, i - 1); rest = substr($0, i + 1)
    j = index(rest, ":"); text = substr(rest, j + 1)
    if (path in whole) next
    for (k = 1; k <= n; k++) if (ap[k] == path && index(text, at[k]) > 0) next
    print
  }' "$HITS")
FILES=$(wc -l < "$LIST" | tr -d ' ')
if [ -n "$BAD" ]; then
  printf '%s\n' "$BAD"
  echo "check-private: $(printf '%s\n' "$BAD" | wc -l | tr -d ' ') line(s) name an internal host ($FILES files checked); write \"a build node\", or allow a neutral match in $ALLOWREL" >&2
  exit 1
fi
echo "check-private: clean ($FILES files checked)"
