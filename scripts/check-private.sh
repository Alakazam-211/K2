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
# Patterns: kept outside the tree, never in it; the first of these that is set or exists:
#   MPC_PRIVATE_PATTERNS        the patterns themselves, one per line
#   MPC_PRIVATE_PATTERNS_FILE   a file of them
#   ~/.config/k2-mpc/private-patterns.txt
# One extended regex per line, matched case-insensitively; `#` starts a comment (to the end of
# the line), blank lines are ignored. No patterns there, or none left after comments: exit 2
# (fail closed). The script never prints a pattern.
# Allowlist: private-allow.txt beside this script, one entry per line, `#` comments (the
# allowlist may quote a match, so it is always skipped):
#   PATH          the whole file
#   PATH:TEXT     a matching line of PATH that contains TEXT (fixed string)
# Exit 0 clean, 1 findings (each printed as PATH:LINE:TEXT), 2 usage or no patterns.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/.." && pwd)}
[ -d "$ROOT" ] || { echo "check-private: no directory $ROOT" >&2; exit 2; }
PATS=$(mktemp); LIST=$(mktemp); HITS=$(mktemp)
trap 'rm -f "$PATS" "$LIST" "$HITS"' EXIT
# the patterns: one per line, comments and blank lines dropped
clean() { sed -e 's/#.*$//' -e 's/[[:space:]]*$//' -e 's/^[[:space:]]*//' | grep -v '^$' || true; }
CONF=${HOME:-/nonexistent}/.config/k2-mpc/private-patterns.txt
if [ -n "${MPC_PRIVATE_PATTERNS:-}" ]; then
  printf '%s\n' "$MPC_PRIVATE_PATTERNS" | clean > "$PATS"
elif [ -n "${MPC_PRIVATE_PATTERNS_FILE:-}" ]; then
  [ -r "$MPC_PRIVATE_PATTERNS_FILE" ] && clean < "$MPC_PRIVATE_PATTERNS_FILE" > "$PATS"
elif [ -r "$CONF" ]; then
  clean < "$CONF" > "$PATS"
fi
if [ ! -s "$PATS" ]; then
  echo "check-private: no patterns: set MPC_PRIVATE_PATTERNS (the patterns, one per line) or MPC_PRIVATE_PATTERNS_FILE (a file of them), or write ~/.config/k2-mpc/private-patterns.txt (one extended regex per line, # comments)" >&2
  exit 2
fi
cd "$ROOT"
ALLOW=$HERE/private-allow.txt
# the allowlist, as a path under ROOT (skipped: it may quote a match)
ALLOWREL=${HERE#"$(pwd)"/}/private-allow.txt
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git ls-files -z | tr '\0' '\n' > "$LIST"
else
  find . -type f ! -path './target/*' ! -path './.git/*' | sed 's|^\./||' > "$LIST"
fi
: > "$HITS"
while IFS= read -r f; do
  [ -f "$f" ] || continue
  grep -nIiE -f "$PATS" "$f" 2>/dev/null | sed "s|^|$f:|" >> "$HITS" || true
done < "$LIST"
[ -f "$ALLOW" ] || ALLOW=/dev/null
BAD=$(awk -v allow="$ALLOW" -v allowrel="$ALLOWREL" '
  BEGIN {
    whole[allowrel] = 1
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
