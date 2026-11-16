#!/bin/sh
# The public-corpus job: fetches the public files crates/blend/tests/corpus/manifest.tsv and
# unit-probes.tsv name (scripts/fetch-corpus.sh: by checksum, a neutral User-Agent, nothing
# committed) into DIR/corpus and DIR/unit-probes, then runs the tests that read them, each by
# name: they are #[ignore]d in the gate (scripts/check.sh) and fail without their folder
# (BLEND_PUBLIC_CORPUS, BLEND_UNIT_PROBES). Any Linux runner with a checkout, the kit beside it,
# curl, python3 and network.
#
#   scripts/public-corpus.sh [DIR]     (default: a new temporary folder)
set -eu
cd "$(dirname "$0")/.."
DIR=${1:-$(mktemp -d)}
sh scripts/fetch-corpus.sh "$DIR/corpus"
sh scripts/fetch-corpus.sh "$DIR/unit-probes" crates/blend/tests/corpus/unit-probes.tsv
BLEND_PUBLIC_CORPUS=$DIR/corpus BLEND_UNIT_PROBES=$DIR/unit-probes \
  cargo test -q --locked -p k2-mpc-blend --test corpus --test live --test corpus_place --test corpus_mesh -- \
  --ignored --exact --nocapture \
  every_public_file_reads_as_the_checker_says \
  public_corpus_live \
  public_corpus_placements_match_cached_world_matrices \
  unit_probes_mixed_systems_give_no_scale \
  mesh_schema_survey \
  every_corpus_mesh_reads_from_its_layout \
  every_corpus_scene_reads_and_matches_cached_matrices
echo "public corpus: ok"
