#!/usr/bin/env bash
# k2 compute move / --avoid (0.46.8): the CLI's own usage refusals (exit 2,
# nothing sent) and the help/study text agents read first. No daemon: the
# hermetic env hands the CLI a dead port, so anything that did reach the
# network would fail loudly (exit 3), not pass. The daemon side (release,
# --to, refusals, the sweeper) is in crates/k2-daemon/tests/
# it_compute/compute_place_integration.rs; a live run is in compute_cli_smoke.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3"))"; fi; }

# usage <label> <stderr needle> <args…>: exit 2, the needle on stderr.
usage() {
    local label="$1" needle="$2" rc
    shift 2
    set +e
    "$K2" "$@" >/dev/null 2>"$K2_TEST_HOME/err"
    rc=$?
    set -e
    if [ "$rc" = "2" ]; then ok "$label exits 2"; else bad "$label exits 2 (got $rc: $(cat "$K2_TEST_HOME/err"))"; fi
    expect_has "$label says why" "$(cat "$K2_TEST_HOME/err")" "$needle"
}

echo "== k2 compute move usage =="
usage "no --auto or --to" "say where: --auto or --to <node>" compute move ab12cd34
usage "both --auto and --to" "--auto or --to, not both" compute move ab12cd34 --auto --to k2n11
usage "no job and no --all-mine" "name a job, or --all-mine" compute move --auto
usage "a job and --all-mine" "job ids or --all-mine, not both" compute move ab12cd34 --all-mine --auto
usage "--node without --all-mine" "--tag and --node pick jobs with --all-mine" compute move ab12cd34 --node k2n12 --auto
usage "an unknown flag" "unknown flag '--bogus'" compute move ab12cd34 --auto --bogus
usage "--to with no node" "--to needs a node" compute move ab12cd34 --to ""
usage "--avoid with no node" "--avoid needs a node" compute move ab12cd34 --auto --avoid ""
usage "a bad --label" "usage: --label key=value" compute move ab12cd34 --auto --label nolabel

echo "== run --avoid usage =="
usage "run --avoid with no node" "--avoid <node>" compute run --avoid "" -- true

echo "== help and study =="
help="$("$K2" compute --help)"
expect_has "help: move --auto" "$help" "k2 compute move <job>... --auto [--avoid <node>]..."
expect_has "help: move --to" "$help" "k2 compute move <job>... --to <node>"
expect_has "help: move --all-mine" "$help" "k2 compute move --all-mine|--workspace-all"
expect_has "help: run --avoid" "$help" "[--avoid <node>]..."
expect_has "help: a pinned job never moves" "$help" "A pinned job never moves"
study="$("$K2" study compute)"
expect_has "study: PINNED OR AUTO" "$study" "PINNED OR AUTO (0.46.8)"
expect_has "study: the release line" "$study" "k2 compute move ab12cd34 --auto"
expect_has "study: AVOID A NODE" "$study" "AVOID A NODE (0.46.8)"
expect_has "study: avoid code" "$study" "avoid_leaves_no_node"
expect_has "study: move codes" "$study" "no_other_node"

echo
echo "compute_move_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
