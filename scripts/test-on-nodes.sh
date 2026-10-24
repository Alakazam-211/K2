#!/usr/bin/env bash
# K2's own Rust suites on compute nodes ("compute B").
#
# Each crate's cargo-nextest run is split across N nodes with nextest
# partitions (`k2 compute test --shards N`, default 3: the three Linux
# nodes): one shard per node where it can, the repo's `ci` profile
# (.config/nextest.toml: no retries, JUnit), JUnit kept and parsed, ONE
# combined pass/fail per crate with the merged failure list. Runs from a
# K2 terminal (the agent's passport) or a plain shell (the owner token);
# needs "Allow compute nodes" and at least one granted node.
#
# Every job starts from an empty env on the node: HOME and TMPDIR are the
# job's own dirs, XDG_RUNTIME_DIR is set to a fresh dir under TMPDIR, and
# no K2_* / K2SO_* variable reaches a test (the prod-isolation guard).
#
# usage: scripts/test-on-nodes.sh [--shards N] [--crate k2-core|k2-daemon]...
#                                 [--os linux|macos] [--detach] [--json]
#                                 [-- <extra nextest args, e.g. a filter>]
#   --os defaults to linux: --shards runs on --any, and the Macs are
#   either the release builder's or too small for a k2-core build.
#
# Exit: 0 every crate passed; the first failing crate's nextest exit code
# (100 = tests failed, 101 = build failed); 2 bad usage; 3 infra (no node
# fits, queue_timeout, node lost with no retries left…). With --detach it
# prints one run id per crate and returns; each run sends one [compute]
# message when it ends.
set -euo pipefail

shards=3
os=linux
detach=0
as_json=0
crates=()
extra=()
while [ $# -gt 0 ]; do
    case "$1" in
        --shards) shards="${2:-}"; shift 2 ;;
        --crate) crates+=("${2:-}"); shift 2 ;;
        --os) os="${2:-}"; shift 2 ;;
        --detach) detach=1; shift ;;
        --json) as_json=1; shift ;;
        --) shift; extra=("$@"); break ;;
        -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
        *) echo "usage: scripts/test-on-nodes.sh [--shards N] [--crate C]... [--os O] [--detach] [--json] [-- nextest args]" >&2; exit 2 ;;
    esac
done
[ "${#crates[@]}" -gt 0 ] || crates=(k2-core k2-daemon)
case "$shards" in ''|*[!0-9]*) echo "usage: --shards N (1–16)" >&2; exit 2 ;; esac

K2="${K2_CLI:-k2}"
common=(
    compute test --any --shards "$shards"
    --profile ci
    # No sys: packages: --any also picks k2-arch, where the Debian names
    # (zlib1g-dev, pkg-config) are package_not_approved. The Linux nodes
    # have the C build deps installed; K2_TEST_NEEDS overrides the list.
    --needs "${K2_TEST_NEEDS:-rust@stable,cargo-nextest,cmake@3,ninja,protoc}"
    --mem 16G --env CARGO_BUILD_JOBS=4
    # A cold node builds the whole workspace first.
    --timeout 2h --stall-timeout 30m --queue-timeout 2h --retries 1
)
[ -n "$os" ] && common+=(--os "$os")
[ "$detach" = "1" ] && common+=(--detach)
[ "$as_json" = "1" ] && common+=(--json)

status=0
for crate in "${crates[@]}"; do
    echo "[test-on-nodes] $crate: $shards shard(s)" >&2
    set +e
    "$K2" "${common[@]}" -- -p "$crate" "${extra[@]+"${extra[@]}"}"
    rc=$?
    set -e
    if [ "$rc" -ne 0 ] && [ "$status" -eq 0 ]; then
        status=$rc
    fi
    echo "[test-on-nodes] $crate: exit $rc" >&2
done
exit "$status"
