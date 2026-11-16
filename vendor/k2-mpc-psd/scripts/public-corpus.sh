#!/bin/sh
# The public-corpus job: fetches the public files crates/psd/tests/corpus/manifest.tsv names
# (scripts/fetch-corpus.sh: by checksum, a neutral User-Agent, nothing committed) into DIR, then
# runs the tests that read them, each by name: they are #[ignore]d in the gate (scripts/check.sh)
# and fail without PSD_PUBLIC_CORPUS. PSD_TOOLS_ORACLE, when the caller sets it, passes through
# (tests/pack_corpus.rs's cross-check). Any Linux runner with a checkout, the kit beside it, curl
# and network.
#
#   scripts/public-corpus.sh [DIR]     (default: a new temporary folder)
set -eu
cd "$(dirname "$0")/.."
DIR=${1:-$(mktemp -d)}
sh scripts/fetch-corpus.sh "$DIR"
PSD_PUBLIC_CORPUS=$DIR cargo test -q --locked -p k2-mpc-psd \
  --test pack_anchor --test pack_corpus --test records_corpus -- --ignored --exact --nocapture \
  every_public_corpus_node_follows_the_packs_rules \
  every_public_corpus_file_indexes_and_agrees_with_both_readers \
  every_public_corpus_file_matches_the_full_reader
echo "public corpus: ok"
