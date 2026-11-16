#!/bin/sh
# The private-corpus job: the pack's counts-only corpus row (tests/corpus_counts.rs) on the
# folder PSD_PRIVATE_CORPUS names, files kept outside every repo on the machine that runs it. It
# prints counts only, and the row fails on any output line that could show a path or a file
# name; nothing it reads enters the repo or a log. The gate (scripts/check.sh) never runs it and
# never needs it.
#
#   PSD_PRIVATE_CORPUS=DIR scripts/private-corpus.sh
set -eu
cd "$(dirname "$0")/.."
: "${PSD_PRIVATE_CORPUS:?set PSD_PRIVATE_CORPUS to the corpus folder}"
cargo test -q --locked -p k2-mpc-psd --test corpus_counts -- --ignored --exact --nocapture \
  the_corpus_writes_back_its_own_bytes_counts_only
echo "private corpus: ok (counts only)"
