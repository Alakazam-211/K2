# BEGIN_CLI_FOCUS_GROUP
# `k2 focus-group list | create | rename | add | remove` (0.46.17): the
# sidebar's focus groups, on the routes Settings → Workspaces uses:
#   GET  /cli/focus-groups/list
#   POST /cli/focus-groups/create {name}
#   POST /cli/focus-groups/update {id, name}
#   POST /cli/focus-groups/assign {projectId, focusGroupId}  (null = no group)
# An `id` tool: inside a K2 terminal it sends the session's passport, never
# the owner token. The list is open to agents; a change is a person's, and
# the daemon refuses an agent `focus_group_needs_person` (exit 3), which
# ends with the permission card to ask with when cards are on.
# Exit: 0 ok · 1 error · 2 bad usage · 3 refused · 4 not found or ambiguous.

cmd_help_focus_group() {
    cat <<'EOF'
%include cli/text/help/focus-group.txt
EOF
}

_fg_err() {
    local code="$1" hint="$2"
    if [ "${_FG_JSON:-false}" = true ]; then
        CODE="$code" HINT="$hint" python3 -c 'import json,os
print(json.dumps({"error": {"code": os.environ["CODE"], "hint": os.environ["HINT"]}}))' >&2
    else
        echo "Error ($code): $hint" >&2
    fi
}

_fg_usage() {
    _fg_err "usage" "$1 (usage: k2 focus-group list | create <name> | rename <group> <new-name> | add <group> <workspace> | remove <group> <workspace>)"
    exit 2
}

# The groups, as the daemon lists them. Over TCP: the cell socket doesn't
# serve the list, and the daemon admits a passport there (a shared read).
_fg_list_raw() {
    _K2_FORCE_TCP=1 cli_send_json GET /cli/focus-groups/list
}

# Read "<body>\n<status>" on stdin and end the command on an error.
# Prints the body when it is a good answer.
_fg_check_answer() {
    WHAT="$1" FG_JSON="${_FG_JSON:-false}" python3 -c '
import json, os, sys
raw = sys.stdin.read()
body, _, status = raw.rpartition("\n")
try:
    status = int(status)
except ValueError:
    status = 0
as_json = os.environ["FG_JSON"] == "true"
def fail(code, hint, rc):
    if as_json:
        print(json.dumps({"error": {"code": code, "hint": hint}}), file=sys.stderr)
    else:
        print("Error (%s): %s" % (code, hint), file=sys.stderr)
    sys.exit(rc)
if status == 0:
    fail("daemon_unreachable", "no answer from the K2 daemon (%s)" % os.environ["WHAT"], 1)
try:
    d = json.loads(body)
except Exception:
    fail("daemon_error", "unparseable answer (HTTP %d): %s" % (status, body[:300]), 1)
err = d.get("error") if isinstance(d, dict) else None
if status >= 300 or err:
    if isinstance(err, dict):
        code, hint = err.get("code") or "error", err.get("hint") or err.get("message") or ""
    else:
        code, hint = "error", str(err or "HTTP %d" % status)
    if as_json:
        print(json.dumps(d), file=sys.stderr)
    else:
        print("Error (%s): %s" % (code, hint), file=sys.stderr)
    sys.exit(3 if status in (401, 403) else 4 if status == 404 else 1)
sys.stdout.write(body)
'
}

# _fg_group <selector>: the one group a selector names (its id, its name
# in any case, or the start of exactly one name), as `{"id","name"}`.
_fg_group() {
    local raw
    raw="$(_fg_list_raw)"
    printf '%s' "$raw" | _fg_check_answer "/cli/focus-groups/list" >/dev/null || exit $?
    printf '%s' "$raw" | SEL="$1" FG_JSON="${_FG_JSON:-false}" python3 -c '
import json, os, sys
body = sys.stdin.read().rpartition("\n")[0]
groups = json.loads(body)
if not isinstance(groups, list):
    groups = []
sel = os.environ["SEL"].strip()
low = sel.lower()
hits = [g for g in groups if g.get("id") == sel or (g.get("name") or "").lower() == low]
if not hits:
    hits = [g for g in groups if (g.get("name") or "").lower().startswith(low)]
def fail(code, hint):
    if os.environ["FG_JSON"] == "true":
        print(json.dumps({"error": {"code": code, "hint": hint}}), file=sys.stderr)
    else:
        print("Error (%s): %s" % (code, hint), file=sys.stderr)
    sys.exit(4)
if not hits:
    fail("not_found", "no focus group matches %r (see: k2 focus-group list)" % sel)
if len(hits) > 1:
    fail("ambiguous", "%r matches more than one focus group: %s (pass the id)" % (sel, ", ".join(sorted(g.get("name") or g.get("id") or "?" for g in hits))))
print(json.dumps({"id": hits[0].get("id") or "", "name": hits[0].get("name") or sel}))
'
}

# _fg_workspace <workspace>: `{"id","name","focusGroupId"}` from its conf.
_fg_workspace() {
    local conf rc=0
    conf="$(_agent_conf_fetch "$1")" || rc=$?
    [ "$rc" -eq 0 ] || exit "$rc"
    printf '%s' "$conf" | python3 -c '
import json, sys
d = json.loads(sys.stdin.read())
print(json.dumps({"id": d.get("id") or "", "name": d.get("name") or d.get("path") or "", "focusGroupId": d.get("focusGroupId")}))
'
}

# _fg_send <route> <json body> <done line>: the change, then its line.
_fg_send() {
    local route="$1" body="$2" line="$3" raw
    raw="$(cli_send_json POST "$route" "$body")"
    printf '%s' "$raw" | _fg_check_answer "$route" >/dev/null || exit $?
    if [ "${_FG_JSON:-false}" = true ]; then
        printf '%s' "${raw%$'\n'*}"
        echo
    else
        printf '%s\n' "$line"
    fi
}

# _fg_assign <workspace json> <group id or empty> <group label>: put the
# workspace in that group, or in none. Skipped when it is there already.
_fg_assign() {
    local ws="$1" gid="$2" label="$3" plan body line
    plan="$(printf '%s' "$ws" | GID="$gid" LABEL="$label" python3 -c '
import json, os, sys
w = json.loads(sys.stdin.read())
gid = os.environ["GID"] or None
label = os.environ["LABEL"]
same = (w.get("focusGroupId") or None) == gid
body = json.dumps({"projectId": w["id"], "focusGroupId": gid})
if gid:
    line = "%s is in focus group %s%s." % (w["name"], label, " already" if same else " now")
else:
    line = "%s is in no focus group%s." % (w["name"], " already" if same else " now")
print("skip" if same else "send")
print(body)
print(line)
')"
    body="$(sed -n 2p <<<"$plan")"
    line="$(sed -n 3p <<<"$plan")"
    if [ "$(sed -n 1p <<<"$plan")" = skip ]; then
        if [ "${_FG_JSON:-false}" = true ]; then
            printf '{"ok":true,"changed":false}\n'
        else
            printf '%s\n' "$line"
        fi
        return 0
    fi
    _fg_send /cli/focus-groups/assign "$body" "$line"
}

cmd_focus_group() {
    if _wants_help "$@"; then cmd_help_focus_group; exit 0; fi
    # The bare noun (or flags only) is the list.
    local verb="list"
    case "${1:-}" in ""|-*) ;; *) verb="$1"; shift ;; esac
    local _FG_JSON=false args=() a ended=false
    for a in "$@"; do
        if [ "$ended" = true ]; then args+=("$a"); continue; fi
        case "$a" in
            --) ended=true ;;
            --json) _FG_JSON=true ;;
            -*) _fg_usage "unknown flag '$a'" ;;
            *) args+=("$a") ;;
        esac
    done
    local n=${#args[@]}
    case "$verb" in
        list|ls)
            [ "$n" -eq 0 ] || _fg_usage "list takes no arguments"
            local raw body
            raw="$(_fg_list_raw)"
            body="$(printf '%s' "$raw" | _fg_check_answer "/cli/focus-groups/list")" || exit $?
            printf '%s' "$body" | FG_JSON="$_FG_JSON" python3 -c '
import json, os, sys
groups = json.loads(sys.stdin.read())
if not isinstance(groups, list):
    groups = []
if os.environ["FG_JSON"] == "true":
    print(json.dumps({"ok": True, "focusGroups": groups}))
    sys.exit(0)
if not groups:
    print("No focus groups. Create one: k2 focus-group create <name>")
    sys.exit(0)
for g in groups:
    print("  %-24s %s" % (g.get("name") or "?", g.get("id") or ""))
'
            ;;
        create)
            [ "$n" -eq 1 ] || _fg_usage "create takes one name"
            [ -n "${args[0]// /}" ] || _fg_usage "the name is empty"
            local body
            body="$(NAME="${args[0]}" python3 -c 'import json,os; print(json.dumps({"name": os.environ["NAME"]}))')"
            _fg_send /cli/focus-groups/create "$body" "Created focus group ${args[0]}."
            ;;
        rename)
            [ "$n" -eq 2 ] || _fg_usage "rename takes a group and its new name"
            [ -n "${args[1]// /}" ] || _fg_usage "the new name is empty"
            local g body
            g="$(_fg_group "${args[0]}")" || exit $?
            body="$(printf '%s' "$g" | TO="${args[1]}" python3 -c 'import json,os,sys; g=json.loads(sys.stdin.read()); print(json.dumps({"id": g["id"], "name": os.environ["TO"]}))')"
            _fg_send /cli/focus-groups/update "$body" "Renamed focus group $(printf '%s' "$g" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["name"])') to ${args[1]}."
            ;;
        add)
            [ "$n" -eq 2 ] || _fg_usage "add takes a group and a workspace"
            local g ws gid gname
            g="$(_fg_group "${args[0]}")" || exit $?
            ws="$(_fg_workspace "${args[1]}")" || exit $?
            gid="$(printf '%s' "$g" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["id"])')"
            gname="$(printf '%s' "$g" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["name"])')"
            _fg_assign "$ws" "$gid" "$gname"
            ;;
        remove|rm)
            [ "$n" -eq 2 ] || _fg_usage "remove takes a group and a workspace"
            local g ws gid gname now
            g="$(_fg_group "${args[0]}")" || exit $?
            ws="$(_fg_workspace "${args[1]}")" || exit $?
            gid="$(printf '%s' "$g" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["id"])')"
            gname="$(printf '%s' "$g" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["name"])')"
            now="$(printf '%s' "$ws" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read()).get("focusGroupId") or "")')"
            if [ "$now" != "$gid" ]; then
                if [ "$_FG_JSON" = true ]; then
                    printf '{"ok":true,"changed":false}\n'
                else
                    echo "$(printf '%s' "$ws" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["name"])') is not in focus group $gname; nothing changed."
                fi
                return 0
            fi
            _fg_assign "$ws" "" ""
            ;;
        *) _fg_usage "unknown verb '$verb'" ;;
    esac
}
# END_CLI_FOCUS_GROUP
