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

echo "== k2 study skins thread/overlay contract =="
skins_out="$("$K2" study skins)"
assert_contains "thread addr param" "$skins_out" "GET /cli/thread?addr="
assert_contains "not agent query" "$skins_out" "not ?agent="
assert_contains "conversation_id" "$skins_out" "conversation_id"
assert_contains "item.doc.text" "$skins_out" "item.doc.text"
assert_contains "overlay conversation query" "$skins_out" "WS /cli/overlay/events?conversation="
assert_contains "post addr text" "$skins_out" '{"addr":"<handle>","text":"…"}'
assert_absent "gateway no files this cut" "$skins_out" "does NOT proxy /cli/fs/*"
assert_contains "files read-dir" "$skins_out" "/cli/fs/read-dir?workspace="
assert_contains "files read-file" "$skins_out" "/cli/fs/read-file?workspace="
assert_contains "files write-file" "$skins_out" "/cli/fs/write-file"
assert_contains "files events workspace" "$skins_out" "/cli/fs/events?workspace="
assert_contains "has_cap_in_room doors" "$skins_out" "has_cap_in_room"
assert_contains "static dir is public" "$skins_out" "GET / and /assets/* are PUBLIC"
assert_contains "platform mint --name" "$skins_out" "k2 skin-token create --name"
assert_contains "two credentials" "$skins_out" "TWO CREDENTIALS"
assert_contains "byo bff heading" "$skins_out" "BYO BFF (not --skin)"
assert_contains "byo same session" "$skins_out" "same session k2skn_"
assert_contains "byo bearer" "$skins_out" "Authorization: Bearer"
assert_contains "byo not platform for dentist" "$skins_out" "Never a platform --name token for \"this"
assert_contains "login json principalId" "$skins_out" "principalId"
assert_contains "roles for guests" "$skins_out" "ROLES (named bundles for guests"
assert_contains "platform token is not a role" "$skins_out" "it is not a role"
assert_contains "empty rooms dark" "$skins_out" "Empty rooms on a role = Thread dark"
assert_contains "find the room" "$skins_out" "FIND THE ROOM, THEN THE FUNCTIONS"
assert_contains "files on docs not sales" "$skins_out" "Files on Documents does not grant files on Sales"
assert_contains "role room example" "$skins_out" "k2 skin role room dentist sales"
assert_absent "no leftover cartesian create" "$skins_out" "k2 skin role create dentist --caps"
assert_absent "files later cut" "$skins_out" "later gateway cut"
assert_absent "no mint-for-user" "$skins_out" "skin-token create <username>"
assert_contains "tickets:read scope" "$skins_out" "tickets:read"
assert_contains "tickets:post scope" "$skins_out" "tickets:post"
assert_contains "tickets list path" "$skins_out" "/cli/feedback/list?project="
assert_contains "tickets show path" "$skins_out" "/cli/feedback/show?id="
assert_contains "tickets create path" "$skins_out" "/cli/feedback/create"
assert_contains "tickets comment path" "$skins_out" "/cli/feedback/comment"
assert_contains "tickets answer path" "$skins_out" "/cli/feedback/answer"
assert_contains "tickets resolve path" "$skins_out" "/cli/feedback/resolve"
assert_contains "tickets on docs not sales" "$skins_out" "tickets on Documents does not grant tickets on Sales"
assert_contains "tickets project handle" "$skins_out" "project= is handle or uuid only"
assert_contains "wiki:read scope" "$skins_out" "wiki:read"
assert_contains "wiki index path" "$skins_out" "/cli/wiki/index?project="
assert_contains "wiki note path" "$skins_out" "/cli/wiki/note?project="
assert_contains "wiki on docs not sales" "$skins_out" "wiki on Documents does not grant wiki on Sales"
assert_contains "store:read scope" "$skins_out" "store:read"
assert_contains "store list path" "$skins_out" "/cli/store/list?workspace="
assert_contains "store get path" "$skins_out" "/cli/store/get?workspace="
assert_contains "store query path" "$skins_out" "/cli/store/query?workspace="
assert_contains "store guc" "$skins_out" "set_config('k2.skin_principal'"
assert_contains "store on docs not sales" "$skins_out" "store on Documents does not grant store on Sales"
assert_contains "never dsn in spa" "$skins_out" "Never GET /cli/db/dsn"
assert_absent "no dsn body" "$skins_out" "postgres://"
assert_absent "no chatter scope" "$skins_out" "chatter"
assert_contains "custom login.html" "$skins_out" "login.html"
assert_contains "guest card answer" "$skins_out" "POST /cli/thread/answer"
assert_contains "agents ask from PTY" "$skins_out" "k2 thread ask"
assert_contains "agents secret from PTY" "$skins_out" "k2 thread secret"
assert_contains "owner vs agent heading" "$skins_out" "OWNER VS AGENT"
assert_contains "list always" "$skins_out" "workspace agent always"
assert_contains "agent tab toggle" "$skins_out" "Allow this agent to manage Skin Access"
assert_contains "leftover front-door owner" "$skins_out" "front-door"
assert_contains "leftover hydra owner" "$skins_out" "hydra"
assert_contains "do not sudo" "$skins_out" "Do not sudo"

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
