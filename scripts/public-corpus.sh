#!/bin/sh
# The public-corpus job: fetches the public files crates/fab/tests/corpus/manifest.tsv names
# (scripts/fetch-corpus.sh: by checksum, a neutral User-Agent, nothing committed) into DIR, then
# runs the test that reads them by name: it is #[ignore]d in the gate (scripts/check.sh) and
# fails without FAB_PUBLIC_CORPUS. Any Linux runner with a checkout, the kit beside it, curl and
# network.
#
#   scripts/public-corpus.sh [DIR]     (default: a new temporary folder)
set -eu
cd "$(dirname "$0")/.."
DIR=${1:-$(mktemp -d)}
sh scripts/fetch-corpus.sh "$DIR"
FAB_PUBLIC_CORPUS=$DIR cargo test -q --locked -p k2-mpc-fab --test corpus -- --ignored --exact \
  --nocapture the_public_corpus_reads_or_declines_with_a_typed_reason
echo "public corpus: ok"
