#!/bin/sh
# The lint part lints the COMMITTED files: fail (exit 1) when a tracked file under ROOT differs
# from the commit (an earlier step formatted or edited the tree), listing each one, so a lint
# that passes has checked what was committed. Untracked files don't count (a job's own
# outputs). LINT_DIRTY=1 lets a deliberately dirty tree through (a --dirty job), and says so.
#   tools/lint-guard.sh [ROOT]      exit 0 the tree is the commit, 1 it isn't, 2 usage
set -eu
ROOT=${1:-.}
[ -d "$ROOT" ] || { echo "lint-guard: no folder $ROOT" >&2; exit 2; }
# a job's checkout belongs to another user: git refuses it unless told it's safe
CHANGED=$(git -c safe.directory='*' -C "$ROOT" status --porcelain --untracked-files=no) \
  || { echo "lint-guard: $ROOT is not a git checkout" >&2; exit 2; }
if [ -z "$CHANGED" ]; then
  echo "lint-guard: the tree is the commit ($(git -c safe.directory='*' -C "$ROOT" rev-parse --short=12 HEAD))"
  exit 0
fi
echo "lint-guard: tracked files differ from the commit, so a lint here would not check what was committed:"
printf '%s\n' "$CHANGED" | sed 's/^/  /'
if [ "${LINT_DIRTY:-0}" = 1 ]; then
  echo "lint-guard: LINT_DIRTY=1: going on with a dirty tree"
  exit 0
fi
exit 1
