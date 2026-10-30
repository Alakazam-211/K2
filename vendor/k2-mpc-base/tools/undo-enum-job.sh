#!/bin/sh
# A compute-node job that builds the spec-undo binaries (release, mutants on) and runs a
# commands file in the unpacked repo root, with E ARGS (undo_enum), R ARGS
# (undo_itf_replay) and timed LABEL CMD.... No Quint or Java. Sources travel in the job
# (`--no-source`), chunked through environment variables: see tools/undo-bundle.sh and
# tools/README.md.
#
#   tools/undo-enum-job.sh <commands file> <out dir> [extra files, e.g. a checkpoint]
exec "$(dirname "$0")/undo-bundle.sh" enum "$@"
