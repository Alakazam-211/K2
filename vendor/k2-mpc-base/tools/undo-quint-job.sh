#!/bin/sh
# FALLBACK ONLY (an env-chunk bundle): the default is a --repo job running
# `sh tools/undo/job.sh quint <commands file>` (tools/README.md, "The undo model check").
# A compute-node job that runs Quint (0.33, installed in the job; TLC through Apalache) on
# spec/quint/undo.qnt, plus the spec-undo binaries (undo_enum, undo_itf_replay). Sources
# travel in the job (`--no-source`), chunked through environment variables: see
# tools/undo-bundle.sh.
#
#   tools/undo-quint-job.sh [--env NAME=VALUE]... <commands file> <out dir> [extra files]
#
# The commands file runs in spec/quint after the build and the Quint install, with:
#   inst NAME PIDS EVENTS UNDOS REVERTS MAXLEN "INIT" MULTI PURGE MUTANT   an instance
#   itfinst NAME ...same...    the instance with `act` and `out` (ITF; winit / wstep)
#   q ARGS                     quint ARGS
#   tlc NAME "INVS" SECONDS    TLC on an instance (after `. "$ROOT/tools/undo/tlc.sh"`)
#   E ARGS / R ARGS            undo_enum / undo_itf_replay (release, mutants on)
#   timed LABEL CMD...         CMD with its wall time and exit code; never fails the job
exec "$(dirname "$0")/undo-bundle.sh" quint "$@"
