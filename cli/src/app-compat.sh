# ── k2 app compat / identity / user meta (prd-app-supabase-compat-v1, lane A)
# The Supabase-compatible data layer for one App: POST /cli/app/compat/*,
# /cli/app/identity/*, /cli/app/user/meta. Exit codes: 0 ok, 1 usage or
# failed, 3 refused. Only names and counts are printed, never a password,
# a key or a URL with credentials.

_app_compat_usage() {
    cat <<'EOF'
%include cli/text/help/app/compat.txt
EOF
}

_app_identity_usage() {
    cat <<'EOF'
%include cli/text/help/app/identity.txt
EOF
}

_app_compat_bad() {
    printf '{"error":"usage","hint":%s}\n' "$(printf '%s' "$1" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')" >&2
    exit 1
}

# Print an answer's compat view and warnings (stdin).
_app_compat_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
c = v.get("compat") or {}
if v.get("on") is False:
    print("%s: the Supabase-compatible layer is off (k2 app compat on %s --db <name>)" % (v.get("app"), v.get("app")))
    sys.exit(0)
if c:
    print("%s → %s  door login %s" % (c.get("app"), c.get("database"), c.get("authenticator")))
    print("anon %s · server key on the public host %s · sub %s · unknown email %s" % (
        "on" if c.get("anon") else "off", "on" if c.get("serviceKeyPublic") else "off", c.get("sub"), c.get("unknownEmail")))
    print("schemas %s · origins %s · max rows %s" % (",".join(c.get("schemas") or []), ",".join(c.get("origins") or []) or "same origin only", (c.get("limits") or {}).get("maxRows")))
u = v.get("users")
if isinstance(u, dict):
    print("auth.users: %s added, %s changed, %s removed" % (u.get("inserted"), u.get("updated"), u.get("deleted")))
for i in v.get("issuers") or []:
    k = i.get("keys") or {}
    print("issuer %s  %s  aud %s  keys %s" % (i.get("key"), i.get("issuer"), i.get("audience"), "fetched" if k.get("fetched") else "not fetched yet"))
for w in v.get("warnings") or []:
    print("warning: %s" % w)
if v.get("note"):
    print("note: %s" % v["note"])'
}

_app_compat_post() {
    local path="$1" body="$2" json_out="$3"
    K2_CP_MAX_TIME=300 _app_rt_post "$path" "$body"
    [ "$_CP_STATUS" = 200 ] || _app_db_fail
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_compat_print
}

cmd_app_compat() {
    local sub="${1:-}"
    case "$sub" in
        on|off|status|set|sync-users) shift ;;
        ""|-h|--help|help) _app_compat_usage; return 0 ;;
        *) _app_compat_bad "k2 app compat on|off|status|set|sync-users <app-id> (k2 app compat --help)" ;;
    esac
    local app="" db="" json_out=0 pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --db) [ $# -ge 2 ] && [ -n "${2:-}" ] || _app_compat_bad "--db needs a value"; db="$2"; shift 2 ;;
            --db=*) db="${1#*=}"; shift ;;
            -h|--help) _app_compat_usage; return 0 ;;
            -*) _app_compat_bad "unknown flag $1 (k2 app compat --help)" ;;
            *) if [ -z "$app" ]; then app="$1"; else pos+=("$1"); fi; shift ;;
        esac
    done
    [ -n "$app" ] || _app_compat_bad "name the App: k2 app compat $sub <app-id>"
    case "$sub" in
        on)
            [ -n "$db" ] || _app_compat_bad "name the database: k2 app compat on $app --db <name> (k2 db list)"
            _app_compat_post /cli/app/compat/on "$(_compute_json K2_CJ_app="$app" K2_CJ_db="$db")" "$json_out" ;;
        off) _app_compat_post /cli/app/compat/off "$(_compute_json K2_CJ_app="$app")" "$json_out" ;;
        sync-users) _app_compat_post /cli/app/compat/sync-users "$(_compute_json K2_CJ_app="$app")" "$json_out" ;;
        set)
            [ "${#pos[@]}" = 2 ] || _app_compat_bad "k2 app compat set $app <setting> <value> (k2 app compat --help)"
            _app_compat_post /cli/app/compat/set "$(_compute_json K2_CJ_app="$app" K2_CJ_key="${pos[0]}" K2_CJ_value="${pos[1]}")" "$json_out" ;;
        status)
            _compute_split "$(_compute_send GET /cli/app/compat/status "app=$(urlencode "$app")")"
            [ "$_CP_STATUS" = 200 ] || _app_db_fail
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | _app_compat_print ;;
    esac
}

cmd_app_identity() {
    local sub="${1:-}" verb=""
    case "$sub" in
        issuer)
            shift; verb="${1:-}"
            case "$verb" in
                add|rm) shift ;;
                -h|--help|help) _app_identity_usage; return 0 ;;
                *) _app_compat_bad "k2 app identity issuer add|rm <app-id> … (k2 app identity --help)" ;;
            esac ;;
        issuers|unlink) shift ;;
        ""|-h|--help|help) _app_identity_usage; return 0 ;;
        *) _app_compat_bad "k2 app identity issuer add|rm, issuers, unlink (k2 app identity --help)" ;;
    esac
    local app="" json_out=0 key="" iss="" aud="" jwks="" pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --key|--issuer|--audience|--jwks)
                [ $# -ge 2 ] && [ -n "${2:-}" ] || _app_compat_bad "$1 needs a value"
                case "$1" in --key) key="$2" ;; --issuer) iss="$2" ;; --audience) aud="$2" ;; --jwks) jwks="$2" ;; esac
                shift 2 ;;
            -h|--help) _app_identity_usage; return 0 ;;
            -*) _app_compat_bad "unknown flag $1 (k2 app identity --help)" ;;
            *) if [ -z "$app" ]; then app="$1"; else pos+=("$1"); fi; shift ;;
        esac
    done
    [ -n "$app" ] || _app_compat_bad "name the App (k2 app identity --help)"
    case "$sub:$verb" in
        issuer:add)
            [ -n "$key" ] && [ -n "$iss" ] && [ -n "$aud" ] && [ -n "$jwks" ] || _app_compat_bad "issuer add needs --key, --issuer, --audience and --jwks"
            _app_compat_post /cli/app/identity/issuer/add "$(_compute_json K2_CJ_app="$app" K2_CJ_key="$key" K2_CJ_issuer="$iss" K2_CJ_audience="$aud" K2_CJ_jwks="$jwks")" "$json_out" ;;
        issuer:rm)
            [ "${#pos[@]}" = 1 ] || _app_compat_bad "k2 app identity issuer rm $app <iss|key>"
            _app_compat_post /cli/app/identity/issuer/rm "$(_compute_json K2_CJ_app="$app" K2_CJ_issuer="${pos[0]}")" "$json_out" ;;
        unlink:)
            [ "${#pos[@]}" = 1 ] || _app_compat_bad "k2 app identity unlink $app <user>"
            K2_CP_MAX_TIME=60 _app_rt_post /cli/app/identity/unlink "$(_compute_json K2_CJ_app="$app" K2_CJ_user="${pos[0]}")"
            [ "$_CP_STATUS" = 200 ] || _app_db_fail
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s recorded sign-in pair(s) cleared" % (v.get("user"), v.get("cleared")))' ;;
        issuers:)
            _compute_split "$(_compute_send GET /cli/app/identity/issuers "app=$(urlencode "$app")")"
            [ "$_CP_STATUS" = 200 ] || _app_db_fail
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | _app_compat_print ;;
    esac
}

# k2 app user meta <app-id> <user> [--app-metadata JSON] [--user-metadata JSON] [--json]
cmd_app_user_meta() {
    local app="" user="" am="" um="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --app-metadata|--user-metadata)
                [ $# -ge 2 ] || _app_compat_bad "$1 needs a JSON object"
                if [ "$1" = --app-metadata ]; then am="$2"; else um="$2"; fi
                shift 2 ;;
            -h|--help)
                echo "k2 app user meta <app-id> <user> [--app-metadata '{…}'] [--user-metadata '{…}'] [--json]"
                echo "  read, or replace, an App user's app_metadata (yours to set, never the user's) and"
                echo "  user_metadata (the user may set it too). JSON objects, 16 KiB each. Both reach the"
                echo "  claims (auth.jwt()) and auth.users when the Supabase-compatible layer is on."
                return 0 ;;
            -*) _app_compat_bad "unknown flag $1 (k2 app user meta --help)" ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$user" ]; then user="$1"; else _app_compat_bad "k2 app user meta <app-id> <user>"; fi; shift ;;
        esac
    done
    [ -n "$app" ] && [ -n "$user" ] || _app_compat_bad "k2 app user meta <app-id> <user> (k2 app user meta --help)"
    local body
    body="$(_compute_json K2_CJ_app="$app" K2_CJ_user="$user" K2_CJ_appMetadata_j="$am" K2_CJ_userMetadata_j="$um" 2>/dev/null)" \
        || _app_compat_bad "--app-metadata and --user-metadata take JSON objects"
    K2_CP_MAX_TIME=60 _app_rt_post /cli/app/user/meta "$body"
    [ "$_CP_STATUS" = 200 ] || _app_db_fail
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s (%s)" % (v.get("user"), v.get("id")))
print("app_metadata  %s" % json.dumps(v.get("appMetadata")))
print("user_metadata %s" % json.dumps(v.get("userMetadata")))'
}
