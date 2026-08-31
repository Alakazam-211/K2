#!/usr/bin/env bash
# k2 study — daemon-optional bounded pages (Fair Source, people, errors).
# No daemon: skip-conn-gate matches --schema. Never touch real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
K2="$K2_CLI"

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
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}

# Fake missing heartbeat: empty HOME, no PORT/TOKEN.
SANDBOX="$(mktemp -d -t k2-study-XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT
export HOME="$SANDBOX"
unset K2_PORT K2_HOOK_TOKEN K2SO_PORT K2SO_HOOK_TOKEN K2_HOST || true

TOPICS="what source map identity send human people auth errors context db mail connect-boundary skins feedback-loop"

echo "== k2 study source (no daemon) =="
set +e
source_out="$("$K2" study source 2>&1)"
source_rc=$?
set -e
assert_eq "study source exit 0" "$source_rc" "0"
assert_contains "FSL" "$source_out" "FSL"
assert_contains "Fair Source" "$source_out" "Fair Source"
assert_contains "not MIT" "$source_out" "not MIT"

echo "== k2 study people =="
people_out="$("$K2" study people)"
assert_contains "connections (agents)" "$people_out" "CONNECTIONS (AGENTS)"
assert_contains "Connect users" "$people_out" "CONNECT USERS"
assert_contains "skin guests" "$people_out" "SKIN GUESTS"
assert_contains "--users humans" "$people_out" "--users"
assert_contains "never k2 msg" "$people_out" "never"
assert_contains "k2 msg names" "$people_out" "k2 msg"

echo "== k2 study errors =="
errors_out="$("$K2" study errors)"
assert_contains "exit 3" "$errors_out" "EXIT 3"
assert_contains "owner_only" "$errors_out" "owner_only"

echo "== k2 study send =="
send_out="$("$K2" study send)"
assert_contains "k2 msg" "$send_out" "k2 msg"
assert_contains "k2 thread" "$send_out" "k2 thread"
assert_contains "k2 mail" "$send_out" "k2 mail"

echo "== k2 study nosuch =="
set +e
nosuch_out="$("$K2" study nosuch 2>&1)"
nosuch_rc=$?
set -e
assert_eq "unknown topic exit 2" "$nosuch_rc" "2"
assert_contains "unknown topic usage" "$nosuch_out" "Usage: k2 study"
assert_contains "lists valid ids" "$nosuch_out" "feedback-loop"

echo "== k2 study --json catalog =="
json_catalog="$("$K2" study --json)"
json_list="$("$K2" study list --json)"
python3 -c 'import json,sys; json.loads(sys.argv[1])' "$json_catalog"
python3 -c 'import json,sys; json.loads(sys.argv[1])' "$json_list"
echo "  PASS: catalog JSON parses"
pass=$((pass + 1))
echo "  PASS: list --json JSON parses"
pass=$((pass + 1))
for id in $TOPICS; do
    assert_contains "catalog has $id" "$json_catalog" "\"id\":\"$id\""
    assert_contains "list --json has $id" "$json_list" "\"id\":\"$id\""
done

echo "== k2 study source --json =="
source_json="$("$K2" study source --json)"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d.get("id")=="source" and "FSL" in d.get("body",""), d' "$source_json"
echo "  PASS: source --json id+body"
pass=$((pass + 1))

echo "== no this-box hostnames / raw secrets =="
all="$("$K2" study)"
all="$all$("$K2" study list)"
all="$all$json_catalog"
for id in $TOPICS; do
    all="$all$("$K2" study "$id")"
done
assert_absent "no rosson.k2.dev" "$all" "rosson.k2.dev"
assert_absent "no z3mbp" "$all" "z3mbp"
if printf '%s' "$all" | grep -Eq 'k2skn_[A-Za-z0-9]{8,}'; then
    echo "  FAIL: raw k2skn_ secret material" >&2
    fail=$((fail + 1))
else
    echo "  PASS: no raw k2skn_ secret material"
    pass=$((pass + 1))
fi

echo "== --help exit 0; bad flag exit 2 =="
set +e
help_out="$("$K2" study --help 2>&1)"
help_rc=$?
bad_out="$("$K2" study --bogus 2>&1)"
bad_rc=$?
unknown_verb_out="$("$K2" nosuch 2>&1)"
unknown_verb_rc=$?
help_gate_out="$("$K2" help 2>&1)"
help_gate_rc=$?
set -e
assert_eq "study --help exit 0" "$help_rc" "0"
assert_contains "study --help usage" "$help_out" "Usage: k2 study"
assert_eq "bad flag exit 2" "$bad_rc" "2"
assert_eq "unknown live verb still needs PORT/TOKEN" "$unknown_verb_rc" "1"
assert_contains "unknown verb connection error" "$unknown_verb_out" "Cannot connect to K2"
assert_eq "k2 help is not daemon-optional" "$help_gate_rc" "1"

echo "== schema mentions study =="
assert_contains "schema usage" "$(grep -F 'k2 study [list' "$K2_CLI" || true)" "k2 study [list|<topic>|--json]"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
