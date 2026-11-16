#!/bin/sh
# The public-corpus job: fetches the public files crates/timeline/tests/corpus/manifest.tsv names
# (scripts/fetch-corpus.sh: by checksum, nothing committed) into DIR, then runs the tests that
# read them, each by name: they are #[ignore]d in the gate (scripts/check.sh) and fail without
# TIMELINE_PUBLIC_CORPUS. Any Linux runner with a checkout, the kit beside it, curl and network.
#
#   scripts/public-corpus.sh [DIR]     (default: a new temporary folder)
set -eu
cd "$(dirname "$0")/.."
DIR=${1:-$(mktemp -d)}
sh scripts/fetch-corpus.sh "$DIR"
TIMELINE_PUBLIC_CORPUS=$DIR cargo test -q --locked -p k2-mpc-timeline \
  --test corpus --test tick_grid --test live -- --ignored --exact --nocapture \
  the_public_corpus_reads_and_matches_an_independent_count \
  share_of_public_files_off_the_tick_grid \
  public_corpus_live
echo "public corpus: ok"
