#!/bin/sh
# Fetches the public test files named in crates/blend/tests/corpus/manifest.tsv into DIR, by
# checksum: nothing is committed, and a file whose checksum differs fails the run. A zip is
# checked, then only the named member is taken out (python3, standard library), and checked
# too. A neutral User-Agent; no account or personal data in any request; a pause between
# downloads, and retries on a busy server.
#   scripts/fetch-corpus.sh DIR [MANIFEST]   (another manifest of the same form: unit-probes.tsv)
set -eu
DIR=${1:?usage: fetch-corpus.sh DIR [MANIFEST]}
M=${2:-$(cd "$(dirname "$0")/.." && pwd)/crates/blend/tests/corpus/manifest.tsv}
mkdir -p "$DIR"
tab=$(printf '\t')
grep -v '^#' "$M" | while IFS="$tab" read -r name url _lic sha member msha; do
  [ -n "$name" ] || continue
  f="$DIR/$name"
  want=$sha
  [ "$member" = - ] || want=$msha
  if [ -f "$f" ] && echo "$want  $f" | sha256sum -c - >/dev/null 2>&1; then continue; fi
  rm -f "$f" "$f.download"
  curl -sSfL --retry 6 --retry-delay 20 --max-time 900 -A 'corpus-fetch' -o "$f.download" "$url"
  echo "$sha  $f.download" | sha256sum -c - >/dev/null
  if [ "$member" = - ]; then
    mv "$f.download" "$f"
  else
    python3 -I -c 'import sys, zipfile; open(sys.argv[3], "wb").write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))' \
      "$f.download" "$member" "$f"
    rm -f "$f.download"
    echo "$msha  $f" | sha256sum -c - >/dev/null
  fi
  sleep 5
done
echo "corpus: ok ($(grep -vc '^#' "$M") files)"
