#!/bin/sh
# Fetches the public test files named in crates/psd/tests/corpus/manifest.tsv into DIR, by checksum:
# nothing is committed, and a file whose checksum differs fails the run.
#   scripts/fetch-corpus.sh DIR
set -eu
DIR=${1:?usage: fetch-corpus.sh DIR}
M=$(cd "$(dirname "$0")/.." && pwd)/crates/psd/tests/corpus/manifest.tsv
mkdir -p "$DIR"
tab=$(printf '\t')
grep -v '^#' "$M" | while IFS="$tab" read -r name url _lic sha; do
  [ -n "$name" ] || continue
  f="$DIR/$name"
  if [ -f "$f" ] && echo "$sha  $f" | sha256sum -c - >/dev/null 2>&1; then continue; fi
  rm -f "$f"
  curl -sSfL --retry 3 --max-time 600 -A 'psd-corpus-fetch' -o "$f" "$url"
  echo "$sha  $f" | sha256sum -c -
done
echo "corpus: ok"
