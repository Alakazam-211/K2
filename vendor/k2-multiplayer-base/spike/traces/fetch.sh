#!/bin/sh
# Fetch the public automerge-paper editing trace (josephg/editing-traces) for the D0 bench.
set -eu
cd "$(dirname "$0")"
curl -sSfL -o automerge-paper.json.gz https://github.com/josephg/editing-traces/raw/master/sequential_traces/automerge-paper.json.gz
gunzip -kf automerge-paper.json.gz
