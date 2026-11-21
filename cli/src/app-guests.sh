# ── RS-A15 guests:rooms (0.46.1) ─────────────────────────────────────
# k2 app grant <app> --kind guests-rooms, k2 app guest rooms <app> <user>.
# Exit codes: 0 done, 2 usage, 3 refused or infra.

_app_guest_rooms_usage() {
    cat <<'EOF'
%include cli/text/help/app/guest/rooms.txt
EOF
}

# prd-app-caps-0464-v1 AC4 (0.46.4): `k2 app guest ceiling` / `list`.
cmd_app_guest_reads() {
    local what="$1"; shift
    local app="" json_out=0 limit="" cursor=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --limit) _need_value "$@"; limit="${2:-}"; shift 2 ;;
            --cursor) _need_value "$@"; cursor="${2:-}"; shift 2 ;;
            -h|--help) _app_guest_rooms_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app guest $what <app> [--json]" >&2; exit 2; }
    if [ "$what" = ceiling ]; then
        _compute_call GET /cli/app/guest/ceiling "app=$(urlencode "$app")"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a = v.get("appInfo") or {}
own = v.get("ownRoom") or {}
print("%s: own room %s" % (a.get("handle") or v.get("app"), own.get("handle") or "(gone)"))
if not v.get("granted"):
    print("  no guests-rooms grant: its own code may hand out nothing (k2 app grant %s --kind guests-rooms). An agent with the App-users switch on sets any room itself." % (a.get("name") or v.get("app")))
else:
    rooms = [r.get("handle") for r in (v.get("grantRooms") or [])]
    print("  granted; hands out: %s" % (", ".join([own.get("handle")] * bool(own) + rooms) or "nothing"))'
        return 0
    fi
    local q="app=$(urlencode "$app")"
    [ -n "$limit" ] && q="$q&limit=$(urlencode "$limit")"
    [ -n "$cursor" ] && q="$q&cursor=$(urlencode "$cursor")"
    _compute_call GET /cli/app/guest/list "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
gs = v.get("guests") or []
if not gs:
    print("%s: no guests" % v.get("app"))
for g in gs:
    rooms = ", ".join(g.get("roomHandles") or []) or "no rooms"
    tags = (" role " + g["role"]) if g.get("role") else ""
    tags += "" if g.get("enabled", True) else "  [disabled]"
    tags += "  [agent login]" if g.get("agent") else ""
    print("%-24s %s%s" % (g.get("username"), rooms, tags))
if v.get("nextCursor"):
    print("more: --cursor %s" % v["nextCursor"])'
}

_app_guest_rooms_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
rooms = ", ".join(v.get("roomHandles") or []) or "no rooms"
role = (" role " + v["role"]) if v.get("role") else " (Thread-only rooms)"
state = "" if v.get("enabled", True) else "  [disabled on this App]"
line = "%s on %s: %s%s%s" % (v.get("username"), v.get("app"), rooms, role, state)
if "changed" in v and "before" in v:
    if v.get("changed"):
        line += "  (was: %s)" % (", ".join(v.get("beforeHandles") or []) or "no rooms")
    else:
        line += "  (unchanged)"
print(line)'
}

# k2 agent access set <workspace> chat_manage|chat_refresh|agents_hire|agents_manage
# on|off (Sew for Apps, 0.46.6; the chat refresh switch, 0.46.8; hiring and
# sidecars, 0.46.12): one workspace's agent. The owner or an admin (the
# daemon refuses an agent: it never loosens its own access).
cmd_agent_access() {
    local usage="usage: k2 agent access set <workspace> chat_manage|chat_refresh|agents_hire|agents_manage on|off [--json]"
    case "${1:-}" in
        -h|--help|"") echo "$usage"; echo "  chat_manage = \"Manage chat and voice\": the agent may run chat and voice here, create Spaces and bind them to its Apps. On also makes its own chat account (off keeps it)."; echo "  chat_refresh = \"Let this agent refresh its pinned chat\"."; echo "  agents_hire = \"Let this agent hire agents\": its main session may run k2 agent hire (a new agent in its own workspace)."; echo "  agents_manage = \"Let this agent manage its sidecars\": its main session may open and stop its sidecars (also k2 sidecar access on|off)."; [ -z "${1:-}" ] && exit 2; return 0 ;;
        set) shift ;;
        *) echo "$usage" >&2; exit 2 ;;
    esac
    local ws="" toggle="" value="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) echo "$usage"; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; echo "$usage" >&2; exit 2 ;;
            *) if [ -z "$ws" ]; then ws="$1"; elif [ -z "$toggle" ]; then toggle="$1"; elif [ -z "$value" ]; then value="$1"; else echo "$usage" >&2; exit 2; fi; shift ;;
        esac
    done
    [ -n "$ws" ] && { [ "$toggle" = chat_manage ] || [ "$toggle" = chat_refresh ] || [ "$toggle" = agents_hire ] || [ "$toggle" = agents_manage ]; } || { echo "$usage" >&2; exit 2; }
    case "$value" in on|off) ;; *) echo "$usage" >&2; exit 2 ;; esac
    _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_toggle="$toggle" K2_CJ_value_b="$([ "$value" = on ] && echo 1 || echo 0)")"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "agentsCanRefreshChat" in v:
    print("Can refresh its own pinned chat is %s for %s" % ("on" if v.get("agentsCanRefreshChat") else "off", v.get("workspace")))
elif "agentsCanHire" in v:
    print("Let this agent hire agents is %s for %s" % ("on" if v.get("agentsCanHire") else "off", v.get("workspace")))
elif v.get("toggle") == "agents_manage":
    print("Let this agent manage its sidecars is %s for %s" % ("on" if v.get("agentsCanManage") else "off", v.get("workspace")))
else:
    print("Manage chat and voice is %s for %s" % ("on" if v.get("agentsCanManageChat") else "off", v.get("workspace")))'
}

cmd_app_grant_kind() {
    local app="" kind="" revoke=0 json_out=0 rooms_csv="" max="" caps="" max_level="" uploads=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --max) _need_value "$@"; max="${2:-}"; shift 2 ;;
            --caps) _need_value "$@"; caps="${2:-}"; shift 2 ;;
            --max-level) _need_value "$@"; max_level="${2:-}"; shift 2 ;;
            --uploads) _need_value "$@"; uploads="${2:-}"; shift 2 ;;
            --kind) _need_value "$@"; kind="${2:-}"; shift 2 ;;
            --kind=*) kind="${1#--kind=}"; shift ;;
            --room|--agent) _need_value "$@"; rooms_csv="${rooms_csv:+$rooms_csv,}${2:-}"; shift 2 ;;
            --room=*) rooms_csv="${rooms_csv:+$rooms_csv,}${1#--room=}"; shift ;;
            --revoke) revoke=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_guest_rooms_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    if [ -z "$app" ] || [ -z "$kind" ]; then
        echo "usage: k2 app grant <app> --kind guests-rooms [--room <handle>]... [--revoke]" >&2
        echo "       k2 app grant <app> --kind guests-chat [--max N] [--revoke]" >&2
        echo "       k2 app grant <app> --kind chat-key [--caps \"…\"] [--max-level member|moderator|admin] [--uploads on|off] [--max N] [--revoke]" >&2
        exit 2
    fi
    if [ "$kind" = chat-key ]; then
        _app_chat_key "$app" "$revoke" "$json_out" "$caps" "$max_level" "$uploads" "$max"
        return $?
    fi
    if [ "$revoke" = 1 ]; then
        _compute_call POST /cli/app/grant/revoke "$(_compute_json K2_CJ_app="$app" K2_CJ_kind="$kind")"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a = v["app"]
print("%s: %s grant %s" % (a.get("handle") or a.get("serviceId"), sys.argv[1], "revoked" if v.get("revoked") else "was not there"))' "$kind"
        return 0
    fi
    if [ "$kind" = guests-chat ]; then
        _app_rt_post /cli/app/grant "$(_compute_json K2_CJ_app="$app" K2_CJ_kind="$kind" K2_CJ_maxMembers_n="$max")"
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a, g = v["app"], v["grant"]
print("%s may give its own guests chat: up to %s people (k2 app user chat <user> member|guest|none --app %s)" % (a.get("handle") or a.get("serviceId"), g.get("maxMembers"), a.get("name")))
print("  its server: GET|POST /cli/skin/guests/chat with its own pass (k2.asApp().guests.setChat)")
if v.get("warning"):
    print("  note: %s; until Sew has app_scoped_sessions the App cannot give chat itself (chat_not_app_scoped)" % v["warning"])'
        return 0
    fi
    local rooms_j
    rooms_j="$(K2_GR_ROOMS="$rooms_csv" python3 -c 'import json,os; print(json.dumps([r.strip() for r in os.environ["K2_GR_ROOMS"].split(",") if r.strip()]))')"
    _app_rt_post /cli/app/grant "$(_compute_json K2_CJ_app="$app" K2_CJ_kind="$kind" K2_CJ_rooms_j="$rooms_j")"
    # An agent without "Let this agent manage its Apps' users", or naming
    # another workspace's room: one held request with a card for the owner.
    _app_grace_done "$json_out"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a, g = v["app"], v["grant"]
print("%s may set its own guests rooms: %s (never above a guest role)" % (a.get("handle") or a.get("serviceId"), ", ".join(g.get("handsOut") or []) or "its own room"))
print("  its server: GET|POST /cli/skin/guests/rooms with its own pass (k2.asApp().guests.setRooms)")'
}

# App-governed Spaces S1 (0.46.7): `k2 app grant <app> --kind chat-key`.
# Turning it on or widening it asks for a vault confirm here (outside K2).
_app_chat_key() {
    local app="$1" revoke="$2" json_out="$3" caps="$4" max_level="$5" uploads="$6" max="$7"
    if [ "$revoke" = 1 ]; then
        _app_rt_post /cli/app/chat-key/revoke "$(_compute_json K2_CJ_app="$app")"
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a = v["app"]
name = a.get("handle") or a.get("serviceId")
if not v.get("revoked"):
    print("%s: its chat key was already off" % name)
else:
    print("%s: chat key off (no more passes through it)" % name)
    print("  Sew ends this App'"'"'s chat sessions" if v.get("sessionsEnded") else "  this Sew does not end sessions by App (no app_governed_spaces); passes stop at once")'
        return 0
    fi
    local up_b=""
    case "$uploads" in
        "") ;;
        on|true|yes) up_b=1 ;;
        off|false|no) up_b=0 ;;
        *) echo "usage: --uploads on|off" >&2; exit 2 ;;
    esac
    local caps_j=""
    if [ -n "$caps" ]; then
        caps_j="$(K2_CK_CAPS="$caps" python3 -c 'import json,os; print(json.dumps([c for c in os.environ["K2_CK_CAPS"].replace(",", " ").split() if c]))')"
    fi
    local body
    body="$(_compute_json K2_CJ_app="$app" K2_CJ_caps_j="$caps_j" K2_CJ_maxLevel="$max_level" K2_CJ_uploads_b="$up_b" K2_CJ_maxMembers_n="$max")"
    _app_rt_post /cli/app/chat-key "$body"
    if [ "$_CP_STATUS" = "403" ] && [[ "$_CP_BODY" == *'"vault_confirm_required"'* ]] && ! _compute_in_session; then
        local svc tok
        svc="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("serviceId") or "")')"
        [ -n "$svc" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        tok="$(_vault_confirm_token chat-key "$svc" "")" || exit 3
        body="$(_with_confirm_token "$body" "$tok")"
        tok=""
        _app_rt_post /cli/app/chat-key "$body"
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a, k = v["app"], v["key"]
print("%s holds a chat key: K2 signs chat passes for its own users, no setup per person" % (a.get("handle") or a.get("serviceId")))
print("  caps %s, up to level %s, up to %s people, uploads %s" % (" ".join(k.get("caps") or []), k.get("maxLevel"), k.get("maxMembers"), "on" if k.get("uploads") else "off"))
print("  its server: chat.pass() with the person'"'"'s own session (k2.withGuest); the first pass may answer chat_member_pending, retry after 2 s")'
}

cmd_app_grants() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_guest_rooms_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app grants <app> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/grants "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
a = v["app"]
gs = v.get("grants") or []
if not gs:
    print("%s: no owner grants (k2 app grant %s --kind guests-rooms, the owner)" % (a.get("handle"), a.get("name"))); sys.exit(0)
for g in gs:
    if g["kind"] == "chat-key":
        what = "%s%s, up to level %s, up to %s people, uploads %s" % ("" if g.get("enabled") else "(off) ", " ".join(g.get("caps") or []), g.get("maxLevel"), g.get("maxMembers"), "on" if g.get("uploads") else "off")
    elif g["kind"] == "guests-chat":
        what = "up to %s people" % g.get("maxMembers")
    else:
        what = "hands out " + ", ".join(g.get("handsOut") or [])
    print("%s  %s  %s  by %s %s" % (a.get("handle"), g["kind"], what, g.get("grantedBy"),
        time.strftime("%Y-%m-%d %H:%M", time.localtime(g["grantedAt"]))))'
}

cmd_app_guest() {
    local what="${1:-}"
    case "$what" in
        rooms) shift ;;
        ceiling|list) shift; cmd_app_guest_reads "$what" "$@"; return $? ;;
        -h|--help|"") _app_guest_rooms_usage; return 0 ;;
        *) echo "usage: k2 app guest rooms|ceiling|list <app> …" >&2; exit 2 ;;
    esac
    local app="" user="" set_mode=0 rooms_csv="" json_out=0 have_rooms=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_guest_rooms_usage; return 0 ;;
            set) set_mode=1; shift
                if [ $# -gt 0 ] && [ "${1:-}" != "--json" ]; then rooms_csv="$1"; have_rooms=1; shift; fi ;;
            -*) [ "$1" = "-" ] && [ "$set_mode" = 1 ] && { rooms_csv=""; have_rooms=1; shift; continue; }
                echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$user" ]; then user="$1"; else echo "unexpected: $1" >&2; exit 2; fi; shift ;;
        esac
    done
    if [ -z "$app" ] || [ -z "$user" ]; then
        echo "usage: k2 app guest rooms <app> <user> [set <room,...>]" >&2
        exit 2
    fi
    if [ "$set_mode" = 1 ]; then
        [ "$have_rooms" = 1 ] || { echo "usage: k2 app guest rooms <app> <user> set <room,room,...>   (- for no rooms)" >&2; exit 2; }
        [ "$rooms_csv" = "-" ] && rooms_csv=""
        local rooms_j
        rooms_j="$(K2_GR_ROOMS="$rooms_csv" python3 -c 'import json,os; print(json.dumps([r.strip() for r in os.environ["K2_GR_ROOMS"].split(",") if r.strip()]))')"
        _compute_call POST /cli/app/guest/rooms "$(_compute_json K2_CJ_app="$app" K2_CJ_username="$user" K2_CJ_rooms_j="$rooms_j")"
    else
        _compute_call GET /cli/app/guest/rooms "app=$(urlencode "$app")&username=$(urlencode "$user")"
    fi
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_guest_rooms_print
}

