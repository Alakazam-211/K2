#!/usr/bin/env bash
# Thread file objects (0.46.1, thread-attachments-v1): `k2 thread <addr>
# "text" --file path[:line[-end]]`. No daemon: help, usage errors and the
# --file → JSON parsing (extracted from cli/k2). The daemon end to end is
# crates/k2-daemon/tests/thread_attachments_integration.rs.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    # A here-string, not a pipe: `grep -q` stops early and pipefail would
    # turn the writer's broken pipe into a miss on a long haystack.
    if grep -Fq -- "$needle" <<<"$hay"; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2; fail=$((fail + 1))
    fi
}
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (got $(printf %q "$got") want $(printf %q "$want"))" >&2; fail=$((fail + 1))
    fi
}

echo "== help =="
help_out="$("$K2_CLI" thread --help 2>&1 || true)"
assert_contains "help: --file" "$help_out" '--file path[:line[-end]]'
assert_contains "help: text optional" "$help_out" "text is optional when a file rides"
assert_contains "help: secrets refused" "$help_out" "secrets go through k2 vault"
study_out="$("$K2_CLI" study human 2>&1 || true)"
assert_contains "study human: attach section" "$study_out" "ATTACH THE FILES YOU MENTION"
schema_out="$("$K2_CLI" --schema 2>&1 || true)"
printf '%s' "$schema_out" | python3 -c 'import json,sys; json.load(sys.stdin)' \
    && { echo "  PASS: --schema is JSON"; pass=$((pass + 1)); } \
    || { echo "  FAIL: --schema is not JSON" >&2; fail=$((fail + 1)); }
assert_contains "schema: --file flag" "$schema_out" '"name": "--file"'

echo "== usage errors (no daemon reached) =="
out="$("$K2_CLI" thread sales "x" --file 2>&1 || true)"
assert_contains "--file needs a value" "$out" "--file needs a path"
args=()
for i in $(seq 1 11); do args+=(--file "f$i.md"); done
out="$("$K2_CLI" thread sales "x" "${args[@]}" 2>&1 || true)"
assert_contains "more than 10 refused" "$out" "at most 10 files per message"

echo "== --file parsing =="
eval "$(sed -n '/^_thread_attachments_json()/,/^}/p' "$K2_CLI")"
ws="$(mktemp -d)"
trap 'rm -rf "$ws"; hermetic_cli_cleanup' EXIT
mkdir -p "$ws/docs" "$ws/src"
printf 'x' > "$ws/docs/plan.md"
printf 'x' > "$ws/src/a.rs"
printf 'x' > "$ws/odd:7"
real_ws="$(cd "$ws" && pwd -P)"
cd "$ws/src"
got="$(_thread_attachments_json a.rs:12 a.rs:3-9 ../docs/plan.md "$ws/docs/plan.md" not/here.md:4 not/here.md)"
want="$(python3 -c 'import json,sys; w=sys.argv[1]; print(json.dumps([
  {"path": w+"/src/a.rs", "line": 12},
  {"path": w+"/src/a.rs", "line": 3, "lineEnd": 9},
  {"path": w+"/docs/plan.md"},
  {"path": w+"/docs/plan.md"},
  {"path": "not/here.md", "line": 4},
  {"path": "not/here.md"}]))' "$PWD/..")"
# Normalise both through Python (the paths may differ only by ../).
norm() { python3 -c 'import json,os,sys; print(json.dumps([dict(x, path=os.path.normpath(x["path"])) for x in json.loads(sys.argv[1])]))' "$1"; }
assert_eq "relative, absolute, line, range, unknown" "$(norm "$got")" "$(norm "$want")"
cd "$ws"
got="$(_thread_attachments_json odd:7)"
assert_eq "a name with a colon that exists is a file, not a line" "$(norm "$got")" "$(norm "[{\"path\": \"$PWD/odd:7\"}]")"
got="$(_thread_attachments_json docs/plan.md:2)"
assert_contains "line spec on an existing file" "$got" '"line": 2'
case "$got" in *"$real_ws"*|*"$ws"*) echo "  PASS: sent absolute"; pass=$((pass + 1)) ;; *) echo "  FAIL: not absolute: $got" >&2; fail=$((fail + 1)) ;; esac

echo
echo "thread_attachments: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
