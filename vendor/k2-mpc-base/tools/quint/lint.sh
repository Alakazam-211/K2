#!/bin/sh
# lint.sh: guard against Quint -> TLA+ export bugs (see the export-issue notes in the engine repository).
#
# Usage (POSIX sh; needs python3 and quint on PATH; the export check also needs java and the
# Apalache jar that `quint verify` downloads, or APALACHE_JAR):
#
#   lint.sh FILE.qnt [FILE.qnt ...]
#       Source lint (qnt_lint.py, no Java, a second or two per file): fails on every
#       expression the TLA+ printer would emit as a hoisted `LET ... IN` (an inline
#       foldl / select / fold / range, or a user operator called with a lambda) in the
#       right operand of an infix operator with more formula after it.
#
#   lint.sh --export FILE.qnt MODULE [MODULE ...]
#       Exact export check (export_check.py): compiles MODULE (an instance module with every
#       constant bound) to TLA+, reads it back through SANY and compares every definition
#       with the Quint IR; every nullary action is exported as a step and every nullary
#       boolean val/def as an invariant. Extra export_check.py options go before FILE.qnt
#       (e.g. --keep DIR, --invariants a,b, --steps s1,s2, --apalache-jar J).
#
# Exit status: 0 clean, 1 findings, 2 tool error. Environment: QUINT (default "quint"),
# PYTHON (default "python3"), JAVA, APALACHE_JAR. The two .py files live next to this
# script; copy all three to use them in another repository.
set -e
here=$(cd "$(dirname "$0")" && pwd)
PY=${PYTHON:-python3}
command -v "$PY" >/dev/null 2>&1 || { echo "lint.sh: $PY not found (add python@3 to the job's --needs)" >&2; exit 2; }
if [ "${1:-}" = "--export" ]; then
  shift
  exec "$PY" "$here/export_check.py" "$@"
fi
[ $# -gt 0 ] || { sed -n '2,24p' "$0" >&2; exit 2; }
exec "$PY" "$here/qnt_lint.py" "$@"
