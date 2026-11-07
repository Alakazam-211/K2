#!/usr/bin/env bash
# node-root-e2e.sh — k2-node's root e2e tests (per-job uids, C2:
# crates/k2-node/tests/job_uid_linux.rs) on a THROWAWAY Linux box.
#
#   scripts/ci/node-root-e2e.sh <ssh-host> [--sha REV] [--test NAME]
#
# 1. Builds k2-node and the test binary for x86_64 Linux on K2 Compute
#    (never on this Mac): a job at REV (default HEAD) on an Ubuntu 24.04
#    node (NODE_E2E_DISTRO, default ubuntu@24.04, so glibc matches the
#    box), kept as artifacts and fetched.
# 2. Copies both to <ssh-host>:/tmp/k2-node-root-e2e/ and runs, as root:
#      K2_NODE_ROOT_E2E=1 K2_NODE_E2E_BIN=<k2-node> job_uid_linux \
#        --ignored --test-threads=1 --nocapture [NAME]
#    Installs `acl` (setfacl/getfacl) there first if it's missing.
#
# The tests make and delete real users and systemd units: the box must be
# one nobody uses and that is wiped afterwards. The ssh user needs
# passwordless sudo. Exit status = the tests'.
#
# Inside the compute job this same script runs with --build-job.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT_REL="node-root-e2e"

build_job() {
    cd "$ROOT"
    export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
    cargo build --locked -p k2-node --bin k2-node
    local exe
    exe="$(cargo test --locked -p k2-node --test job_uid_linux --no-run --message-format=json \
        | python3 -c 'import json,sys
for l in sys.stdin:
    try: m = json.loads(l)
    except ValueError: continue
    if m.get("reason") == "compiler-artifact" and m.get("target", {}).get("name") == "job_uid_linux" and m.get("executable"):
        print(m["executable"])')"
    [ -n "$exe" ] && [ -x "$exe" ] || { echo "FATAL: no job_uid_linux test binary" >&2; exit 1; }
    rm -rf "$OUT_REL"
    mkdir -p "$OUT_REL"
    cp "$exe" "$OUT_REL/job_uid_linux"
    cp "${CARGO_TARGET_DIR:-target}/debug/k2-node" "$OUT_REL/k2-node"
    (cd "$OUT_REL" && sha256sum job_uid_linux k2-node)
}

if [ "${1:-}" = "--build-job" ]; then
    build_job
    exit 0
fi

HOST="${1:-}"
[ -n "$HOST" ] || { echo "usage: scripts/ci/node-root-e2e.sh <ssh-host> [--sha REV] [--test NAME]" >&2; exit 2; }
shift
SHA="HEAD"
FILTER=""
while [ $# -gt 0 ]; do
    case "$1" in
        --sha) SHA="${2:?}"; shift 2 ;;
        --test) FILTER="${2:?}"; shift 2 ;;
        *) echo "usage: unknown argument $1" >&2; exit 2 ;;
    esac
done
DISTRO="${NODE_E2E_DISTRO:-ubuntu@24.04}"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/k2-node-root-e2e.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

echo "== build on K2 Compute ($DISTRO, $SHA)"
k2 compute run --sha "$SHA" --distro "$DISTRO" --needs rust@1.99.1,cmake@3 --cpus 8 --mem 16G \
    --stall-timeout off --artifact "$OUT_REL/*" \
    -- bash scripts/ci/node-root-e2e.sh --build-job 2>&1 | tee "$tmp/build.log"
job="$(sed -n -E 's/^\[k2 compute\] job ([0-9a-f]+) on .*/\1/p' "$tmp/build.log" | head -1)"
[ -n "$job" ] || { echo "FATAL: no job id in the build output" >&2; exit 1; }
grep -q '^\[k2 compute\] exit 0 ' "$tmp/build.log" || { echo "FATAL: the build job $job failed" >&2; exit 1; }
mkdir -p "$tmp/bin"
k2 compute fetch "$job" --out "$tmp/bin"
bin_dir="$(dirname "$(find "$tmp/bin" -name job_uid_linux -type f | head -1)")"
[ -f "$bin_dir/job_uid_linux" ] && [ -f "$bin_dir/k2-node" ] || { echo "FATAL: fetched artifacts lack job_uid_linux/k2-node" >&2; exit 1; }

REMOTE=/tmp/k2-node-root-e2e
echo "== copy to $HOST:$REMOTE"
ssh "$HOST" "rm -rf $REMOTE && mkdir -p $REMOTE"
scp -q "$bin_dir/job_uid_linux" "$bin_dir/k2-node" "$HOST:$REMOTE/"
ssh "$HOST" "chmod 0755 $REMOTE/job_uid_linux $REMOTE/k2-node"

echo "== run on $HOST (root, K2_NODE_ROOT_E2E=1)"
ssh "$HOST" "command -v setfacl >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q acl >/dev/null"
# shellcheck disable=SC2029 # expanded here on purpose
ssh "$HOST" "cd / && sudo env K2_NODE_ROOT_E2E=1 K2_NODE_E2E_BIN=$REMOTE/k2-node $REMOTE/job_uid_linux --ignored --test-threads=1 --nocapture $FILTER"
