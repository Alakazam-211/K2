#!/bin/sh
# Build a client-only repo from one kit commit: `client/` and nothing else.
#
#   tools/client-ship.sh <kit-repo> <kit-sha> <out-dir>
#
# For browser jobs (the client's Playwright tests), which need the JS client alone. The
# runner ships <out-dir> (`k2 compute run … --repo <out-dir> --sha <printed sha>`), so a job
# gets `client/` at that kit commit and no other file of the kit: no Rust crates, no vendored
# engine, no history.
#
# <out-dir> becomes a fresh git repo with ONE commit ("client/ only, from kit <full sha>"),
# no remote. An existing <out-dir> is replaced only when it is an earlier output of this
# script; anything else is refused.
#
# The script checks its own output and fails (exit 1, nothing to ship) when:
#   - any path is outside client/;
#   - any path is under vendor/ or crates/, is a Rust source or a Cargo file, or names the
#     engine (yrs);
#   - the repo has more than one commit.
# The last line it prints ("ship <sha> <- kit <sha>") is the proof to quote with the job id.
#
# The typecheck, the full client test suite and the licence check run in a full checkout:
# the client's tests also read the kit's copy of the shared vectors when it is there, and
# only a full checkout has it.
set -eu
[ $# -eq 3 ] || { echo "usage: $0 <kit-repo> <kit-sha> <out-dir>" >&2; exit 2; }
KIT="$1"; SHA="$2"; OUT="$3"
FULL=$(git -C "$KIT" rev-parse --verify "$SHA^{commit}")
MSG="client/ only, from kit"

if [ -e "$OUT" ]; then
  if [ -n "$(ls -A "$OUT")" ]; then
    SUBJ=$(git -C "$OUT" log -1 --format=%s 2>/dev/null || true)
    case "$SUBJ" in
      "$MSG "*) ;;
      *) echo "REFUSED: $OUT exists and is not an earlier client-only output" >&2; exit 1 ;;
    esac
  fi
  rm -rf "$OUT"
fi
mkdir -p "$OUT"

git -C "$KIT" archive --format=tar "$FULL" client | tar -x -C "$OUT"
git -C "$OUT" init -q
git -C "$OUT" add -A
git -C "$OUT" -c user.name=client-ship -c user.email=client-ship@localhost \
  -c commit.gpgsign=false commit -q -m "$MSG $FULL"

PATHS=$(git -C "$OUT" ls-tree -r --name-only HEAD)
BAD=$(printf '%s\n' "$PATHS" | grep -v '^client/' || true)
[ -z "$BAD" ] || { echo "REFUSED: paths outside client/:" >&2; echo "$BAD" >&2; exit 1; }
BAD=$(printf '%s\n' "$PATHS" | grep -E -i '(^|/)(vendor|crates)/|\.rs$|(^|/)Cargo\.(toml|lock)$|yrs' || true)
[ -z "$BAD" ] || { echo "REFUSED: kit or engine files:" >&2; echo "$BAD" >&2; exit 1; }
[ "$(git -C "$OUT" rev-list --count HEAD)" = 1 ] || { echo "REFUSED: history" >&2; exit 1; }

echo "files: $(printf '%s\n' "$PATHS" | wc -l | tr -d ' ')"
echo "bytes: $(git -C "$OUT" ls-tree -r -l HEAD | awk '{s+=$4} END {print s}')"
echo "top level: $(git -C "$OUT" ls-tree --name-only HEAD | tr '\n' ' ')"
echo "ship $(git -C "$OUT" rev-parse HEAD) <- kit $FULL"
