#!/usr/bin/env bash
# shadow-linux-builds.sh — stage 0 of plan-ci-release-on-k2-compute-v1
# (§7): build a released tag's Linux set on K2 Compute, fetch it, verify
# every job's signed receipt against the tag's commit, and compare the
# files with the GitHub-built assets. Publishes nothing, signs nothing.
#
#   scripts/ci/shadow-linux-builds.sh <tag> [--wait-min N]
#
# Run on the Mac whose K2 submits the jobs (z3mbpZ once it is a server on
# the release nodes), from the repo checkout that has the tag. The same
# steps run inside release.sh with K2_LINUX_BUILDS=shadow, before the tag.
#
# Output: ~/k2-release-audit/<v>/compute-shadow/ (receipts, verdicts,
# shadow-report.json, summary.txt); fetched files in
# target/compute-dist/<v>-shadow/. Settings: scripts/lib/compute-release.sh.
#
# Exit 0: every job built, verified and was compared (the report says
# which files match GitHub). Exit 1: a job failed or didn't verify.
set -euo pipefail

TAG="${1:-}"
[ -n "$TAG" ] || { echo "usage: scripts/ci/shadow-linux-builds.sh <tag> [--wait-min N]" >&2; exit 2; }
shift
WAIT_MIN=90
while [ $# -gt 0 ]; do
    case "$1" in
        --wait-min) WAIT_MIN="${2:-}"; shift 2 ;;
        *) echo "usage: scripts/ci/shadow-linux-builds.sh <tag> [--wait-min N]" >&2; exit 2 ;;
    esac
done

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/lib/compute-release.sh
source "$ROOT/scripts/lib/compute-release.sh"

COMMIT="$(git rev-parse "${TAG}^{commit}")"
VERSION="${TAG#v}"
STATE="${K2_RELEASE_AUDIT_DIR:-$HOME/k2-release-audit}/$VERSION/compute-shadow"
FILES="$ROOT/target/compute-dist/${VERSION}-shadow"

echo "Shadow Linux builds for $TAG ($COMMIT) on K2 Compute"
k2cr_init "$VERSION" "$COMMIT" "$STATE" "$FILES"
rc=0
k2cr_submit_release_set || rc=1
if [ -s "$K2CR_STATE/jobs.tsv" ]; then
    k2cr_wait "$WAIT_MIN" || rc=1
    k2cr_collect || rc=1
    k2cr_compare_github "$TAG"
fi
echo "Audit: $K2CR_STATE (summary.txt, shadow-report.json)"
exit "$rc"
