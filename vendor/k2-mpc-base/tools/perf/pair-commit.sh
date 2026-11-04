#!/bin/sh
# tools/perf/pair-commit.sh A_REV B_REV [BRANCH]
#
# For a paired performance job (tools/perf/pair.sh): a local commit whose tree is A's, with
# B's whole tree under `.perf-b/` and a `.perf-pair` file naming both. A runner that ships a
# commit (`k2 compute run --repo . --sha <it>`) then carries both builds' sources in one job.
# The commit goes on BRANCH (default perf/pair-<A>-<B>); it is never merged or pushed. Run it
# at the repo root (or in a worktree). Prints the commit.
set -e
A=$(git rev-parse --verify "${1:?usage: pair-commit.sh A_REV B_REV [BRANCH]}^{commit}")
B=$(git rev-parse --verify "${2:?usage: pair-commit.sh A_REV B_REV [BRANCH]}^{commit}")
a7=$(echo "$A" | cut -c1-7); b7=$(echo "$B" | cut -c1-7)
BR=${3:-perf/pair-$a7-$b7}
IDX=$(mktemp -d)/index
trap 'rm -rf "$(dirname "$IDX")"' EXIT
GIT_INDEX_FILE=$IDX git read-tree "$A"
GIT_INDEX_FILE=$IDX git read-tree --prefix=.perf-b/ "$B"
PAIR=$(printf 'A=%s\nB=%s\n' "$a7" "$b7" | git hash-object -w --stdin)
GIT_INDEX_FILE=$IDX git update-index --add --cacheinfo "100644,$PAIR,.perf-pair"
T=$(GIT_INDEX_FILE=$IDX git write-tree)
C=$(git commit-tree "$T" -p "$A" -m "perf pair: A $a7, B $b7 (B's tree under .perf-b/; for one job, never merged)")
git branch -f "$BR" "$C" >/dev/null
echo "$C"
