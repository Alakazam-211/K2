#!/usr/bin/env bash
# k2 zen widget, the parts that never reach a daemon
# (prd-zen-user-widgets-v2 UW41, TUW5.1, TUWA11). Hermetic: temp HOME, a
# dead port, no real ~/.k2.
#   - `k2 zen widget grant` / `revoke` exit 0 with the one sentence (widgets
#     need no permissions, Rosson 2026-10-08) and never reach a daemon;
#   - usage errors exit 2 before any request;
#   - help and the --schema manifest list the widget family and --widget;
#   - `k2 zen guide api` is generated, at most 80 lines, and --json parses.
# The daemon half (new/list/revoke/validate --widget, exit 3 before setup)
# is tests/cli/zen_widget_cli.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
# shellcheck source=_hermetic_cli.sh
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

run() { set +e; out="$("$K2_CLI" "$@" 2>&1)"; rc=$?; set -e; }

echo "== there are no permissions =="
MSG="Widgets in your own Gardens need no permissions: they can talk to your agents right away. To stop one, take its [[widget]] out of the Garden file."
for verb in "grant" "grant some-widget --json" "revoke" "revoke g p" "revoke --widget w" "allow w"; do
    # shellcheck disable=SC2086
    run zen widget $verb
    assert_eq "widget $verb exits 0 with no daemon" "$rc" "0"
    assert_eq "widget $verb says there are no permissions" "$out" "$MSG"
done

echo "== usage errors exit 2 before any request =="
run zen widget new
assert_eq "new without a name" "$rc" "2"
run zen widget new a b
assert_eq "new with two names" "$rc" "2"
run zen widget list --from hello
assert_eq "--from on list" "$rc" "2"
run zen widget frobnicate
assert_eq "unknown widget verb" "$rc" "2"
run zen validate --widget a --garden b
assert_eq "--widget and --garden together" "$rc" "2"
assert_contains "names --widget among the choices" "$out" "--widget"
run zen validate --widget
assert_eq "--widget without a name" "$rc" "2"

echo "== help and the manifest =="
run zen --help
assert_eq "zen help exits 0" "$rc" "0"
assert_contains "help lists widget new" "$out" "k2 zen widget new <name> [--from k2:diary|hello|arcade]"
assert_contains "help lists validate --widget" "$out" "--widget <name>"
assert_contains "help says widgets need no permissions" "$out" "no permissions, no review"
case "$out" in *"click Review"*|*"widget revoke"*) bad "help still sends people to a review or a revoke" ;; *) ok "help has no review or revoke" ;; esac
run zen widget --help
assert_eq "widget help exits 0 with no daemon" "$rc" "0"
run --schema json
assert_eq "--schema exits 0" "$rc" "0"
fam="$(printf '%s' "$out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
cmds = d.get("commands", d) if isinstance(d, dict) else d
w = [c for c in cmds if c.get("name") == "zen widget"]
print(len(w), w[0]["usage"] if w else "")
')"
assert_contains "the manifest has a zen widget entry" "$fam" "1 k2 zen widget <new <name>"

echo "== k2 zen guide api =="
run zen guide api
assert_eq "guide api exits 0 with no daemon" "$rc" "0"
lines="$(printf '%s\n' "$out" | wc -l | tr -d ' ')"
[ "$lines" -le 80 ] && ok "guide api is $lines lines (max 80)" || bad "guide api is $lines lines (max 80)"
for v in agents.list agents.subscribe conversation.open thread.read thread.subscribe thread.post thread.answer compose.draft gardens.switch theme.changed presence.get; do
    assert_contains "guide api names k2.$v" "$out" "k2.$v("
done
assert_contains "guide api groups by cap" "$out" "thread:post (Post to the Thread)"
run zen guide api --json
assert_eq "guide api --json exits 0" "$rc" "0"
id="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
assert_eq "guide api --json id" "$id" "api"
run zen guide
assert_contains "the guide index lists api" "$out" "  api "

echo
echo "zen_widget_offline: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
