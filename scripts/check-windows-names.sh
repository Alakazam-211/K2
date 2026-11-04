#!/bin/sh
# Fail if a path in the tree can't be checked out on Windows: a host's machines may run it, and
# one such name stops the whole tree checking out there. POSIX sh, git, tr, sed and awk only.
#
#   tools/check-windows-names.sh [ROOT]       (default: the repo root)
#
# Names: `git ls-files` when ROOT is the top of a git checkout (the name in the index is what
# matters, whether or not the file is on this disk), else every non-directory under ROOT (a node
# job's unpacked bundle), `target/` and `.git/` left out. A line says which.
# Every path component is checked:
#   - a reserved device name, whatever the case and extension: con prn aux nul conin$ conout$
#     com0-9 lpt0-9, and com and lpt with a superscript 1, 2 or 3 (written below as their UTF-8
#     bytes, so this file stays ASCII). The stem is what's before the first dot, trailing spaces
#     trimmed: `NUL.tar.gz` counts;
#   - a character Windows refuses: < > : " \ | ? * and the control characters 0x01-0x1f (a newline
#     in a name is read as 0x01, so it is reported as a control character, not as two names);
#   - a trailing dot or space;
#   - a case collision: two spellings of one path or directory prefix, so `a/X` with `A/Y`
#     collides, and a file `Foo` with `foo/bar`. The fold is ASCII only.
# And every whole path: over 200 bytes is a finding. A host vendors the repo under a prefix, and
# Windows' default limit is 260.
# Exit 0 clean, 1 findings (each printed as PATH: WHY), 2 usage or a tree with no files.
set -eu
LC_ALL=C; export LC_ALL
case $# in 0|1) ;; *) echo "usage: check-windows-names.sh [ROOT]" >&2; exit 2 ;; esac
case ${1:-} in -*) echo "usage: check-windows-names.sh [ROOT]" >&2; exit 2 ;; esac
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/.." && pwd)}
[ -d "$ROOT" ] || { echo "check-windows-names: no directory $ROOT" >&2; exit 2; }
cd "$ROOT"
LIST=$(mktemp)
trap 'rm -f "$LIST"' EXIT
# at the top of a checkout only: a bundle unpacked inside some other repo is read from its files
if [ "$(git rev-parse --is-inside-work-tree 2>/dev/null || echo no)" = true ] \
  && [ -z "$(git rev-parse --show-cdup 2>/dev/null || echo no)" ]; then
  echo "check-windows-names: reading names from git ls-files"
  git ls-files -z | tr '\n' '\001' | tr '\0' '\n' > "$LIST"
else
  echo "check-windows-names: reading names from find"
  find . ! -type d ! -path './target/*' ! -path './.git/*' -print0 \
    | tr '\n' '\001' | tr '\0' '\n' | sed 's|^\./||' > "$LIST"
fi
FILES=$(wc -l < "$LIST" | tr -d ' ')
[ "$FILES" -gt 0 ] || { echo "check-windows-names: no files under $ROOT" >&2; exit 2; }
RC=0
# index() and a table, not character classes: the same on every awk here
awk '
  function show(s,   i, ch, out) {
    out = ""
    for (i = 1; i <= length(s); i++) {
      ch = substr(s, i, 1)
      out = out ((ch in ctl) ? sprintf("\\x%02x", ctl[ch]) : ch)
    }
    return out
  }
  function finding(p, why) { print show(p) ": " why; found++ }
  function component(p, c,   stem, i, ch, done) {
    stem = c
    i = index(stem, ".")
    if (i > 0) stem = substr(stem, 1, i - 1)
    while (length(stem) > 0 && substr(stem, length(stem), 1) == " ") stem = substr(stem, 1, length(stem) - 1)
    if (tolower(stem) in reserved) finding(p, "reserved name " tolower(stem))
    for (i = 1; i <= length(c); i++) {
      ch = substr(c, i, 1)
      if ((ch in done)) continue
      done[ch] = 1
      if (index(refused, ch) > 0) finding(p, "reserved character " ch)
      else if (ch in ctl) finding(p, "control character " sprintf("0x%02x", ctl[ch]))
    }
    ch = substr(c, length(c), 1)
    if (ch == ".") finding(p, "ends in a dot")
    if (ch == " ") finding(p, "ends in a space")
  }
  BEGIN {
    for (i = 1; i < 32; i++) ctl[sprintf("%c", i)] = i
    refused = "<>:\"\\|?*"
    n = split("con prn aux nul conin$ conout$", names, " ")
    for (i = 1; i <= n; i++) reserved[names[i]] = 1
    for (i = 0; i <= 9; i++) { reserved["com" i] = 1; reserved["lpt" i] = 1 }
    n = split("\302\271 \302\262 \302\263", names, " ")
    for (i = 1; i <= n; i++) { reserved["com" names[i]] = 1; reserved["lpt" names[i]] = 1 }
  }
  {
    if (length($0) > 200) finding($0, "path is " length($0) " bytes; the limit is 200")
    k = split($0, parts, "/")
    prefix = ""
    for (j = 1; j <= k; j++) {
      prefix = (j == 1) ? parts[1] : prefix "/" parts[j]
      if (prefix in seen) continue
      seen[prefix] = 1
      component(prefix, parts[j])
      key = tolower(prefix)
      if (key in first) finding(prefix, "case collision with " show(first[key]))
      else first[key] = prefix
    }
  }
  END { exit found ? 1 : 0 }' "$LIST" || RC=$?
case $RC in
  0) echo "check-windows-names: clean ($FILES files checked)" ;;
  1) echo "check-windows-names: names Windows can't check out ($FILES files checked)"; exit 1 ;;
  *) echo "check-windows-names: awk failed (exit $RC)" >&2; exit 2 ;;
esac
