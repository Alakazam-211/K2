#!/bin/sh
# Fetch a test corpus named by reference in corpus/<set>.tsv into DIR, and check every listed
# file's sha256. Nothing fetched is ever committed. The request carries a neutral User-Agent
# and no credentials. Prints counts only, never file contents.
#
#   scripts/fetch-corpus.sh DIR [SET]       (SET: xlsxwriter, the default)
#
# Exit 0 when every listed file is there with its hash; 1 on any missing or different file.
set -eu
cd "$(dirname "$0")/.."
DIR=${1:?usage: scripts/fetch-corpus.sh DIR [SET]}
SET=${2:-xlsxwriter}
MAN=corpus/$SET.tsv
[ -f "$MAN" ] || { echo "fetch-corpus: no manifest $MAN" >&2; exit 2; }
field() { sed -n "s|^# $1	||p" "$MAN" | head -1; }
ARCHIVE=$(field archive)
WITHIN=$(field path | cut -d' ' -f1)
[ -n "$ARCHIVE" ] && [ -n "$WITHIN" ] || { echo "fetch-corpus: $MAN names no archive or path" >&2; exit 2; }
mkdir -p "$DIR"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
curl -sSfL --retry 3 -A "Mozilla/5.0" -o "$TMP/archive.tgz" "$ARCHIVE"
mkdir "$TMP/x"
tar xzf "$TMP/archive.tgz" -C "$TMP/x"
SRC=$(find "$TMP/x" -mindepth 1 -maxdepth 1 -type d | head -1)/$WITHIN
[ -d "$SRC" ] || { echo "fetch-corpus: the archive has no $WITHIN" >&2; exit 1; }
want=0; bad=0
while IFS='	' read -r name sum expect; do
  case $name in ''|'#'*) continue ;; esac
  want=$((want + 1))
  if [ ! -f "$SRC/$name" ]; then echo "missing: $name"; bad=$((bad + 1)); continue; fi
  got=$(sha256sum "$SRC/$name" | cut -d' ' -f1)
  if [ "$got" != "$sum" ]; then echo "different: $name"; bad=$((bad + 1)); continue; fi
  cp "$SRC/$name" "$DIR/$name"
done < "$MAN"
echo "fetch-corpus: $SET: $((want - bad)) of $want files, each with its sha256, in $DIR"
[ $bad -eq 0 ]
