#!/usr/bin/env bash
# Thread markdown reminder: help copy + first/every-10th counter.
# No daemon: help fires before HTTP; nudge helper is extracted from cli/k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    fi
}
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got $(printf %q "$got") want $(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}
assert_absent() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  FAIL: $label (unexpected $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    else
        echo "  PASS: $label"
        pass=$((pass + 1))
    fi
}

echo "== k2 thread --help mentions markdown =="
help_out="$(env K2_PORT=9 K2_HOOK_TOKEN=fake "$K2_CLI" thread --help 2>&1 || true)"
assert_contains "help markdown" "$help_out" "render markdown"

echo "== nudge first + every 10 =="
eval "$(sed -n '/^_ws_dot_dir()/,/^}/p; /^_thread_md_nudge()/,/^}/p' "$K2_CLI")"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/.k2"
export K2_PROJECT_PATH="$tmp"
export PROJECT="$tmp"

nudge_err=""
for i in $(seq 1 21); do
    err=$(_thread_md_nudge 2>&1 >/dev/null || true)
    case "$i" in
        1|10|20)
            assert_contains "nudge on use $i" "$err" "Thread renders markdown"
            ;;
        *)
            assert_absent "silent on use $i" "$err" "Thread renders markdown"
            ;;
    esac
done
assert_eq "counter file is 21" "$(tr -d '[:space:]' < "$tmp/.k2/.thread-md-nudge")" "21"

echo
echo "thread_md_nudge: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
