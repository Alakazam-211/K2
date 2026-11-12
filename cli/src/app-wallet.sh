# ── App users' personal wallets (0.46.3) ─────────────────────────────
# k2 app wallet settings|people|remove|personal-agent|personal-agents.
# The owner side only: a person adds and signs in from the App itself
# (wallet.* verbs with their own session). Exit codes: 0 done, 2 usage,
# 3 refused or infra.

_app_wallet_usage() {
    cat <<'EOF'
k2 app wallet — App users' personal wallets (0.46.3). Each App user can keep
their own LLM subscriptions (Claude, Codex, Grok) and API keys in a wallet on
this server, from the App (wallet.* verbs). Their subscription runs only
their own personal agent. You see that it exists and its status; never the
secret, the sign-in link or code.

  k2 app wallet settings <app> [--on|--off] [--fallback on|off] [--json]
                                       Show or change an App's wallet. --on lets
                                       the App's users use their wallet. With
                                       --fallback on, a personal agent whose
                                       person's subscription can't be used runs
                                       on the company plan; off (default), it
                                       doesn't start. Owner or Admin; an agent of
                                       the App's own workspace with "…and manage
                                       them end to end" may set --fallback only.
                                       <app> is the App id (k2 app list), its
                                       service id or <workspace>/<name>; an
                                       unknown App is refused.
  k2 app wallet people [<app>] [--json]
                                       Every wallet item (or those of <app>'s
                                       users): user, provider, label, state,
                                       which agents use it.
  k2 app wallet remove <item-id>       Delete one item for good (not Trash): its
                                       sessions stop; the person signs in again
                                       to use it. Audited.
  k2 app wallet personal-agent <app> <user> <workspace>
                                       Mark <workspace> as <user>'s own agent
                                       (they must have its room in <app>). It
                                       then runs on <user>'s own subscription
                                       while nobody else has its room. Your
                                       extra tabs in it use the server default.
  k2 app wallet personal-agent --clear <workspace>
  k2 app wallet personal-agents [--json]

Lifecycle: an App user who loses App access keeps their items unused for
30 days (back at once if access returns), then they are deleted. An App user
who is deleted, or turned into an agent's login, loses their items and their
personal-agent marks at the next sweep (within a minute). Removed items are
deleted, never kept in Trash.

Note: each person's subscription runs only their own agent. Check that your
plan's terms allow the way your App uses it.
EOF
}

cmd_app_wallet() {
    local sub="${1:-}"
    [ $# -gt 0 ] && shift
    local json_out=0 a1="" a2="" a3="" on="" fb="" clear=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --on) on=1; shift ;;
            --off) on=0; shift ;;
            --fallback) case "${2:-}" in on) fb=1 ;; off) fb=0 ;; *) echo "usage: --fallback on|off" >&2; exit 2 ;; esac; shift 2 ;;
            --fallback=on) fb=1; shift ;;
            --fallback=off) fb=0; shift ;;
            --clear) clear=1; shift ;;
            -h|--help) _app_wallet_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$a1" ]; then a1="$1"; elif [ -z "$a2" ]; then a2="$1"; elif [ -z "$a3" ]; then a3="$1"; else echo "unexpected: $1" >&2; exit 2; fi; shift ;;
        esac
    done
    case "$sub" in
        settings)
            [ -n "$a1" ] || { echo "usage: k2 app wallet settings <app> [--on|--off] [--fallback on|off]" >&2; exit 2; }
            if [ -n "$on" ] || [ -n "$fb" ]; then
                local en_b="" fb_b=""
                [ "$on" = 1 ] && en_b=1; [ "$on" = 0 ] && en_b=0
                [ "$fb" = 1 ] && fb_b=1; [ "$fb" = 0 ] && fb_b=0
                # _compute_json skips empty values; "0" is false for _b keys.
                _compute_call POST /cli/app/wallet/settings "$(_compute_json K2_CJ_app="$a1" K2_CJ_enabled_b="$en_b" K2_CJ_fallback_b="$fb_b")"
            else
                _compute_call GET /cli/app/wallet/settings "app=$(urlencode "$a1")"
            fi
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin); s = v["settings"]
name = ("%s (%s)" % (v["appId"], s["serviceId"])) if v.get("appId") else s["serviceId"]
print("%s: wallet %s, fallback to the company plan %s" % (name, "on" if s["enabled"] else "off", "on" if s["fallback"] else "off"))'
            ;;
        people)
            if [ -n "$a1" ]; then
                _compute_call GET /cli/app/wallet/people "app=$(urlencode "$a1")"
            else
                _compute_call GET /cli/app/wallet/people ""
            fi
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
items = json.load(sys.stdin).get("items") or []
if not items:
    print("No wallet items."); sys.exit(0)
for i in items:
    used = ", ".join(i.get("usedBy") or []) or "no agent"
    extra = "  dormant" if i.get("dormantSince") else ""
    print("%s  %s  %s %s \"%s\"  %s  used by %s%s" % (i["id"], i["username"], i["provider"], i["type"], i["label"], i["state"], used, extra))'
            ;;
        remove|rm)
            [ -n "$a1" ] || { echo "usage: k2 app wallet remove <item-id>" >&2; exit 2; }
            _compute_call POST /cli/app/wallet/remove "$(_compute_json K2_CJ_id="$a1")"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            echo "Removed $a1 (deleted, not Trash; the person signs in again to use it)."
            ;;
        personal-agent)
            if [ "$clear" = 1 ]; then
                [ -n "$a1" ] || { echo "usage: k2 app wallet personal-agent --clear <workspace>" >&2; exit 2; }
                _compute_call POST /cli/app/wallet/personal-agent "$(_compute_json K2_CJ_workspace="$a1" K2_CJ_clear_b=1)"
            else
                [ -n "$a3" ] || { echo "usage: k2 app wallet personal-agent <app> <user> <workspace>" >&2; exit 2; }
                _compute_call POST /cli/app/wallet/personal-agent "$(_compute_json K2_CJ_app="$a1" K2_CJ_user="$a2" K2_CJ_workspace="$a3")"
            fi
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "cleared" in v:
    print("%s: %s" % (v["workspace"], "no longer a personal agent" if v["cleared"] else "was not a personal agent"))
else:
    print("%s is now %s'"'"'s own agent (through %s)" % (v["workspace"], v["username"], v["app"]))'
            ;;
        personal-agents)
            _compute_call GET /cli/app/wallet/personal-agents ""
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("agents") or []
if not rows:
    print("No personal agents."); sys.exit(0)
for r in rows:
    print("%s  %s  through %s" % (r.get("handle") or r["workspace"], r.get("username") or "(gone)", r["app"]))'
            ;;
        ""|-h|--help|help) _app_wallet_usage ;;
        *) echo "usage: k2 app wallet settings|people|remove|personal-agent|personal-agents (k2 app wallet --help)" >&2; exit 2 ;;
    esac
}

cmd_app() {
    # TC-19: App refusals carry the registry's fix (`_compute_fail`).
    _K2_REFUSAL_AREA=app
    case "${1:-}" in
        init) shift; cmd_app_init "$@" ;;
        validate) shift; cmd_app_validate "$@" ;;
        status) shift; cmd_app_status "$@" ;;
        build) shift; cmd_app_build "$@" ;;
        install) shift; cmd_app_install "$@" ;;
        deploy) shift; cmd_app_deploy "$@" ;;
        rollback) shift; cmd_app_rollback "$@" ;;
        stop|start|restart) cmd_app_lifecycle "$@" ;;
        deployment) shift; cmd_app_deployment "$@" ;;
        link) shift; cmd_app_link "$@" ;;
        seal) shift; cmd_app_seal "$@" ;;
        config) shift; cmd_app_config "$@" ;;
        list) shift; cmd_app_list "$@" ;;
        inspect) shift; cmd_app_inspect "$@" ;;
        history) shift; cmd_app_history "$@" ;;
        logs) shift; cmd_app_logs "$@" ;;
        errors) shift; cmd_app_errors "$@" ;;
        uninstall) shift; cmd_app_uninstall "$@" ;;
        expose) shift; cmd_app_expose "$@" ;;
        handover|unpark) cmd_app_handover "$@" ;;
        park) cmd_app_ownership "$@" ;;
        release) shift; cmd_app_release "$@" ;;
        exec) shift; cmd_app_exec "$@" ;;
        shell) shift; cmd_app_shell "$@" ;;
        exception) shift; cmd_app_exception "$@" ;;
        doctor) shift; cmd_app_doctor "$@" ;;
        box) shift; cmd_app_box "$@" ;;
        engine) shift; cmd_app_engine "$@" ;;
        rehost) shift; cmd_app_rehost "$@" ;;
        data) shift; cmd_app_data "$@" ;;
        sdk) shift; cmd_app_sdk "$@" ;;
        updates) shift; cmd_app_updates "$@" ;;
        resources) shift; cmd_app_resources "$@" ;;
        label) shift; cmd_app_label "$@" ;;
        domain) shift; cmd_app_domain "$@" ;;
        move-plan) shift; cmd_app_move_plan "$@" ;;
        test-guests|test-guest) shift; cmd_app_test_guests "$@" ;;
        domains) shift; cmd_app_domains "$@" ;;
        warnings) shift; cmd_app_warnings "$@" ;;
        clean-mode) shift; cmd_app_clean_mode "$@" ;;
        key) shift; cmd_app_key "$@" ;;
        keys) shift; cmd_app_keys "$@" ;;
        push) shift; cmd_app_push "$@" ;;
        pairing) shift; cmd_app_pairing "$@" ;;
        links) shift; cmd_app_links "$@" ;;
        auth) shift; cmd_app_auth "$@" ;;
        email) shift; cmd_app_email "$@" ;;
        approvals) shift; cmd_app_approvals "$@" ;;
        approve) shift; cmd_app_approve "$@" ;;
        deny) shift; cmd_app_deny "$@" ;;
        access) shift; cmd_app_access "$@" ;;
        suspend) shift; cmd_app_suspend "$@" ;;
        resume) shift; cmd_app_resume "$@" ;;
        audit) shift; cmd_app_audit "$@" ;;
        quota|quotas) shift; cmd_app_quota "$@" ;;
        remote-pass) shift; cmd_app_remote_pass "$@" ;;
        # RS-A15: `grant <app> --kind …` is an App's own owner grant; the
        # list|add|rm|set|hooks|copy-host forms stay the guest-login and hooked
        # (Sew chat) grants (cmd_skin).
        grant|grants)
            case "${2:-}" in
                ""|list|add|create|rm|remove|delete|revoke|set|update|role|hooks|hook|copy-host|copyhost|-h|--help) cmd_skin "$@" ;;
                *)
                    if [ "$1" = grants ]; then shift; cmd_app_grants "$@"; else shift; cmd_app_grant_kind "$@"; fi
                    ;;
            esac
            ;;
        guest) shift; cmd_app_guest "$@" ;;
        wallet) shift; cmd_app_wallet "$@" ;;
        members) shift; cmd_app_members "$@" ;;
        people) shift; cmd_app_people "$@" ;;
        discovery) shift; cmd_app_discovery "$@" ;;
        stay-signed-in) shift; cmd_app_stay "$@" ;;
        invite-settings) shift; cmd_app_invite_settings "$@" ;;
        # 0.46.9: account deletion mode and grace, client log retention.
        set) shift; cmd_app_set "$@" ;;
        # 0.46.11: recording in an App's calls (Sew 0.9.0).
        recording|recordings) shift; cmd_app_recording "$@" ;;
        account-deletion|account-deletions) shift; cmd_app_account_deletion "$@" ;;
        profile)
            shift
            case "${1:-}" in
                settings) shift; cmd_app_profile_settings "$@" ;;
                *) echo "usage: k2 app profile settings <app> [--names on|off] [--image on|off] [--json]" >&2; exit 2 ;;
            esac
            ;;
        login)
            shift
            case "${1:-}" in
                settings) shift; cmd_app_invite_settings "$@" ;;
                -h|--help) cmd_app_invite_settings --help ;;
                *) echo "usage: k2 app login settings <app> [--invite-ttl 72h] [--reset-ttl 24h] [--set-password-url <url>] [--invite-subject <s>] [--invite-body-file <f>] [--reset-subject <s>] [--reset-body-file <f>] [--max-devices <n>] [--default <field>]" >&2; exit 2 ;;
            esac
            ;;
        room-names) shift; cmd_app_room_names "$@" ;;
        self) shift; cmd_app_self "$@" ;;
        chat) shift; cmd_app_chat "$@" ;;
        usage) shift; cmd_app_usage "$@" ;;
        surfaces) shift; _app_surfaces_usage ;;
        door) shift; _app_door_usage ;;
        grace|crossover) shift; _app_grace_usage ;;
        egress) shift; cmd_app_egress "$@" ;;
        # 0.46.11: an App's database (prd-app-database-integrations-v1 S2).
        db|database) shift; cmd_app_db "$@" ;;
        box-service|box-services) shift; cmd_app_box_service "$@" ;;
        token|tokens)
            shift || true
            if [ "${1:-}" = rotate ]; then shift; cmd_app_token_rotate "$@"; else cmd_skin_token "$@"; fi
            ;;
        *)
            cmd_skin "$@"
            ;;
    esac
}

cmd_skin() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        user|users)
            local _a _has_app=0
            for _a in "$@"; do
                # A test guest names its App as the positional <app-id>
                # (k2 app user add <app-id> <name> --test --role R), 0.46.5.
                case "$_a" in --app|--app=*|--test) _has_app=1 ;; esac
            done
            for _a in "$@"; do
                if [[ "$_a" == "--role" && "$_has_app" -eq 0 ]]; then
                    echo "Error: Connect --role is not a skin command. Use k2 app user role <username> <role> or k2 app role create (--role on user add needs --app)." >&2
                    exit 2
                fi
            done
            local action="${1:-list}"
            shift || true
            # 0.46.4: test guests (--test) go through the App runtime.
            case "$action" in
                add|remove|rm|password)
                    for _a in "$@"; do
                        if [[ "$_a" == "--test" ]]; then
                            _app_test_guest "$action" "$@"
                            return $?
                        fi
                    done ;;
            esac
            case "$action" in
                add)
                    local username="" password_stdin=0 temp_password=0 email="" app="" role=""
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --password-stdin) password_stdin=1; shift ;;
                            --temp-password) temp_password=1; shift ;;
                            --email) email="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --email=*) email="${1#--email=}"; shift ;;
                            --app) app="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --app=*) app="${1#--app=}"; shift ;;
                            --role) role="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --role=*) role="${1#--role=}"; shift ;;
                            --help|-h) echo "Usage: k2 app user add <username> [--email e] [--password-stdin | --temp-password] [--app <app> [--role R]]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" ]] && { echo "Usage: k2 app user add <username> [--email e] [--password-stdin | --temp-password]" >&2; exit 1; }
                    if [[ "$password_stdin" -eq 1 && "$temp_password" -eq 1 ]]; then
                        echo "Use --password-stdin or --temp-password, not both." >&2; exit 2
                    fi
                    local pw=""
                    if [[ "$password_stdin" -eq 1 ]]; then
                        pw=$(cat)
                    fi
                    local body raw
                    body=$(K2_SKIN_USER="$username" K2_SKIN_PW="$pw" K2_SKIN_PW_SET="$password_stdin" K2_SKIN_EMAIL="$email" K2_SKIN_APP="$app" K2_SKIN_ROLE="$role" K2_SKIN_TEMP="$temp_password" python3 -c '
import json,os
b={"username": os.environ["K2_SKIN_USER"]}
if os.environ.get("K2_SKIN_PW_SET","0") == "1":
    b["password"]=os.environ.get("K2_SKIN_PW","")
if os.environ.get("K2_SKIN_TEMP","0") == "1":
    b["generate"]=True
em=os.environ.get("K2_SKIN_EMAIL","")
if em:
    b["email"]=em
app=os.environ.get("K2_SKIN_APP","").strip()
if app:
    b["app"]=app
role=os.environ.get("K2_SKIN_ROLE","").strip()
if role:
    b["role"]=role
print(json.dumps(b))')
                    raw=$(cli_post_json "/cli/skin/users" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | K2_SKIN_APP="$app" python3 -c '
import json,os,sys
d=json.load(sys.stdin)
app=os.environ.get("K2_SKIN_APP","")
if app:
    print("App user added: %s (App %s only; no box login)" % (d.get("username",""), app))
else:
    print("Skin user added: %s" % d.get("username",""))
pw=d.get("generatedPassword")
if pw:
    print("Temporary password (shown once, K2 keeps no copy): %s" % pw)
    print("Give it to them privately, never in a Thread, chat or file others can read. At their first sign-in K2 asks them to choose their own.")
    print("To handle no password at all: k2 app user invite <username> --app <app> mails them a link.")
w=d.get("waitingForReset") or {}
if w.get("rooms"):
    print("Waiting: everything else works now, but they can'"'"'t use room %s until they choose their own password, because you set this login up: first sign-in with a temporary password, an invite (%s), or your App'"'"'s forgot-password flow." % (", ".join(w["rooms"]), w.get("ask")))'
                    ;;
                show)
                    # app-user-profile P6 (0.46.5): a person's profile in one
                    # App as the owner sees it (owner only).
                    local username="" app="" json_out=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --app) app="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --app=*) app="${1#--app=}"; shift ;;
                            --json) json_out=1; shift ;;
                            --help|-h) echo "Usage: k2 app user show <username> --app <app> [--json]   (owner only)" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" || -z "$app" ]] && { echo "Usage: k2 app user show <username> --app <app> [--json]" >&2; exit 1; }
                    local raw
                    raw=$(cli_request "/cli/skin/access/profile" "app=$(urlencode "$app")" "user=$(urlencode "$username")")
                    printf '%s' "$raw" | _skin_die_if_err
                    if [[ "$json_out" -eq 1 ]]; then printf '%s\n' "$raw"; return 0; fi
                    printf '%s' "$raw" | python3 -c '
import json,sys,time
d=json.load(sys.stdin)
def t(x): return time.strftime("%Y-%m-%d %H:%M", time.localtime(x)) if x else "—"
full=d.get("fullName") or "—"
if d.get("nameSetBySelf") and d.get("fullName"):
    full="%s (set by %s)" % (full, d.get("username"))
print("%s in App %s" % (d.get("username"), d.get("app")))
print("  full name     %s" % full)
print("  display name  %s" % (d.get("displayName") or "—"))
print("  email         %s" % (d.get("email") or "—"))
print("  picture       %s" % ("yes (%s…)" % d["imageHash"][:12] if d.get("imageHash") else "—"))
print("  role here     %s" % (d.get("role") or "—"))
print("  made          %s    last sign-in %s" % (t(d.get("createdAt")), t(d.get("lastSignInAt"))))
s=d.get("settings") or {}
print("  may edit      names %s, picture %s" % ("on" if s.get("editNames") else "off", "on" if s.get("editImage") else "off"))
rows=d.get("sessions") or []
print("  sessions      %d" % len(rows))
for r in rows:
    print("    %-38s %-7s %-24s last %s%s" % (r.get("id"), r.get("kind"), (r.get("client") or "—")[:24], t(r.get("lastSeenAt") or r.get("createdAt")), "  push:%s" % r.get("pushPlatform") if r.get("push") else ""))'
                    ;;
                image)
                    # app-user-profile P6: the owner clears a person's picture.
                    local username="" clear=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --clear) clear=1; shift ;;
                            --help|-h) echo "Usage: k2 app user image <username> --clear   (owner only)" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" || "$clear" -ne 1 ]] && { echo "Usage: k2 app user image <username> --clear" >&2; exit 1; }
                    local body raw
                    body=$(_compute_json K2_CJ_user="$username")
                    raw=$(cli_post_json "/cli/skin/access/profile/image/clear" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print(("Picture cleared for %s" if d.get("cleared") else "%s had no picture") % d.get("username"))'
                    ;;
                sessions)
                    # app-user-profile P6: a person's browsers and devices in
                    # one App; --revoke <id> ends one, --revoke-all every one.
                    local username="" app="" revoke="" all=0 json_out=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --app) app="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --app=*) app="${1#--app=}"; shift ;;
                            --revoke) revoke="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --revoke-all) all=1; shift ;;
                            --json) json_out=1; shift ;;
                            --help|-h) echo "Usage: k2 app user sessions <username> --app <app> [--revoke <id> | --revoke-all] [--json]   (the owner, or the App's own agent with \"Let this agent manage its Apps' users\")" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" || -z "$app" ]] && { echo "Usage: k2 app user sessions <username> --app <app> [--revoke <id> | --revoke-all]" >&2; exit 1; }
                    local body raw
                    if [[ -n "$revoke" || "$all" -eq 1 ]]; then
                        body=$(_compute_json K2_CJ_user="$username" K2_CJ_app="$app" K2_CJ_id="$revoke" K2_CJ_all_b="$all")
                        raw=$(cli_post_json "/cli/skin/users/sessions/revoke" "$body")
                        printf '%s' "$raw" | _skin_die_if_err
                        if [[ "$json_out" -eq 1 ]]; then printf '%s\n' "$raw"; return 0; fi
                        printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Ended %d session(s) of %s in App %s" % (d.get("ended",0), d.get("username"), d.get("app")))'
                        return 0
                    fi
                    raw=$(cli_request "/cli/skin/users/sessions" "app=$(urlencode "$app")" "user=$(urlencode "$username")")
                    printf '%s' "$raw" | _skin_die_if_err
                    if [[ "$json_out" -eq 1 ]]; then printf '%s' "$raw" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(json.dumps({"ok": True, "sessions": d.get("sessions") or []}))'; return 0; fi
                    printf '%s' "$raw" | python3 -c '
import json,sys,time
d=json.load(sys.stdin)
def t(x): return time.strftime("%Y-%m-%d %H:%M", time.localtime(x)) if x else "—"
rows=d.get("sessions") or []
if not rows:
    print("%s has no live sessions in App %s" % (d.get("username"), d.get("app"))); sys.exit(0)
for r in rows:
    print("%-38s %-7s %-24s since %s  last %s%s" % (r.get("id"), r.get("kind"), (r.get("client") or "—")[:24], t(r.get("createdAt")), t(r.get("lastSeenAt")), "  push:%s" % r.get("pushPlatform") if r.get("push") else ""))'
                    ;;
                invite)
                    local username="" app="" email="" role="" from="" json_out=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --app) app="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --app=*) app="${1#--app=}"; shift ;;
                            --email) email="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --email=*) email="${1#--email=}"; shift ;;
                            --role) role="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --role=*) role="${1#--role=}"; shift ;;
                            --from) from="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --from=*) from="${1#--from=}"; shift ;;
                            --json) json_out=1; shift ;;
                            --help|-h) echo "Usage: k2 app user invite <username> --app <app> [--email e] [--role R] [--from <address>] [--json]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" || -z "$app" ]] && { echo "Usage: k2 app user invite <username> --app <app> [--email e] [--role R] [--from <address>]" >&2; exit 1; }
                    local body raw
                    body=$(_compute_json K2_CJ_username="$username" K2_CJ_app="$app" K2_CJ_email="$email" K2_CJ_role="$role" K2_CJ_from="$from")
                    raw=$(cli_post_json "/cli/skin/users/invite" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    if [[ "$json_out" -eq 1 ]]; then printf '%s\n' "$raw"; return 0; fi
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
how=d.get("delivered")
if how=="sent":
    print("Invite sent to %s from %s (the link works once; it expires in %s hours)" % (d.get("sentTo"), d.get("from"), d.get("ttlSecs",0)//3600 or "<1"))
elif how=="held":
    print(d.get("hint") or "Invite held for approval.")
else:
    print("No mail path on this box: give %s this one-time link yourself (shown once):\n%s" % (d.get("username"), d.get("link")))
w=d.get("waitingForReset") or {}
if w.get("rooms"):
    print("Waiting: everything else works now, but they can'"'"'t use room %s until they choose their own password, because you set this login up: first sign-in with a temporary password, an invite (%s), or your App'"'"'s forgot-password flow." % (", ".join(w["rooms"]), w.get("ask")))'
                    ;;
                reset)
                    local username="" app="" to="" code=0 from="" json_out=0 revoke=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --app) app="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --app=*) app="${1#--app=}"; shift ;;
                            --revoke) revoke=1; shift ;;
                            --to) to="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --to=*) to="${1#--to=}"; shift ;;
                            --code) code=1; shift ;;
                            --from) from="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --from=*) from="${1#--from=}"; shift ;;
                            --json) json_out=1; shift ;;
                            --help|-h) echo "Usage: k2 app user reset <username> [--app <app>] [--revoke] [--to <email> | --code] [--from <address>] [--json]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" ]] && { echo "Usage: k2 app user reset <username> [--app <app>] [--revoke] [--to <email> | --code] [--from <address>]" >&2; exit 1; }
                    local body raw
                    body=$(_compute_json K2_CJ_username="$username" K2_CJ_app="$app" K2_CJ_to="$to" K2_CJ_code_b="$code" K2_CJ_from="$from")
                    if [[ "$revoke" -eq 1 ]]; then
                        body=$(printf '%s' "$body" | python3 -c 'import json,sys; d=json.load(sys.stdin); d["revoke"]=True; print(json.dumps(d))')
                    fi
                    raw=$(cli_post_json "/cli/skin/users/reset-link" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    if [[ "$json_out" -eq 1 ]]; then printf '%s\n' "$raw"; return 0; fi
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
how=d.get("delivered")
if "revoked" in d:
    print("Took back %s unused link(s) for %s on that App" % (d.get("revoked"), d.get("username")))
elif how=="sent":
    print("Reset link sent to %s (works once; when it is used, %s gets everything their role names)" % (d.get("sentTo"), d.get("username")))
elif how=="held":
    print(d.get("hint") or "Reset link held for approval.")
else:
    print("One-time reset link for %s (shown once; give it to them yourself):\n%s" % (d.get("username"), d.get("link")))'
                    ;;
                password)
                    local username=""
                    local password_stdin=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --password-stdin) password_stdin=1; shift ;;
                            --help|-h) echo "Usage: k2 app user password <username> --password-stdin" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" || "$password_stdin" -ne 1 ]] && { echo "Usage: k2 app user password <username> --password-stdin" >&2; exit 1; }
                    local pw
                    pw=$(cat)
                    local body raw
                    body=$(K2_SKIN_USER="$username" K2_SKIN_PW="$pw" python3 -c '
import json,os
print(json.dumps({"username": os.environ["K2_SKIN_USER"], "password": os.environ.get("K2_SKIN_PW","")}))')
                    raw=$(cli_post_json "/cli/skin/users/password" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Skin password %s for %s" % ("set" if d.get("hasPassword") else "cleared", d.get("username","")))
if d.get("passwordMustChange"):
    print("At their next sign-in they choose a new password (no session until then).")'
                    ;;
                chat)
                    # Sew for Apps A2 (0.46.6): a person's chat (their Sew
                    # account). The owner: anyone; an agent: its own App's
                    # guests (--app) within that App's guests-chat grant.
                    local username="" scope="" app="" json_out=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --app) app="${2:-}"; shift 2 ;;
                            --json) json_out=1; shift ;;
                            --help|-h) echo "Usage: k2 app user chat <username> [member|guest|none] [--app <app>] [--json]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
                            *) if [[ -z "$username" ]]; then username="$1"; elif [[ -z "$scope" ]]; then scope="$1"; else echo "Unexpected: $1" >&2; exit 2; fi; shift ;;
                        esac
                    done
                    [[ -z "$username" ]] && { echo "Usage: k2 app user chat <username> [member|guest|none] [--app <app>] [--json]" >&2; exit 2; }
                    if [[ -z "$scope" ]]; then
                        local q="username=$(urlencode "$username")"
                        [[ -n "$app" ]] && q="$q&app=$(urlencode "$app")"
                        _compute_call GET /cli/app/guest/chat "$q"
                    else
                        case "$scope" in member|guest|none) ;; *) echo "scope is member, guest or none" >&2; exit 2 ;; esac
                        _compute_call POST /cli/app/guest/chat "$(_compute_json K2_CJ_username="$username" K2_CJ_scope="$scope" K2_CJ_app="$app")"
                    fi
                    if [[ "$json_out" -eq 1 ]]; then printf '%s\n' "$_CP_BODY"; return 0; fi
                    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
scope = v.get("scope") or "no chat"
state = "" if v.get("enabled") or not v.get("scope") else "  [switched off by the owner]"
by = {"app": "  (given by the App)", "owner": "  (given by the owner)"}.get(v.get("managedBy") or "", "")
line = "%s: %s%s%s" % (v.get("username"), scope, by, state)
if v.get("app"):
    line += "  on %s, %s of %s people" % (v["app"], v.get("members"), v.get("maxMembers") if v.get("maxMembers") is not None else "?")
if "changed" in v and not v.get("changed"):
    line += "  (unchanged)"
print(line)
print("  chat id: k2:principal:%s" % v.get("principalId"))
if v.get("warning"):
    print("  note: " + v["warning"])'
                    ;;
                unlock)
                    local username="" json_out=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --json) json_out=1; shift ;;
                            --help|-h) echo "Usage: k2 app user unlock <username> [--json]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" ]] && { echo "Usage: k2 app user unlock <username> [--json]" >&2; exit 1; }
                    local body raw
                    body=$(K2_SKIN_USER="$username" python3 -c 'import json,os; print(json.dumps({"username": os.environ["K2_SKIN_USER"]}))')
                    raw=$(cli_post_json "/cli/skin/users/unlock" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    if [ "$json_out" = 1 ]; then
                        printf '%s\n' "$raw"
                        return 0
                    fi
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
key=d.get("username","")
if d.get("cleared") is True:
    print("Lock cleared for %s" % key)
else:
    print("No lock for %s" % key)'
                    ;;
                email)
                    local username="" email=""
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --email) email="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --email=*) email="${1#--email=}"; shift ;;
                            --help|-h) echo "Usage: k2 app user email <username> [--email e]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" ]] && { echo "Usage: k2 app user email <username> [--email e]" >&2; exit 1; }
                    local body raw
                    body=$(K2_SKIN_USER="$username" K2_SKIN_EMAIL="$email" python3 -c '
import json,os
print(json.dumps({"username": os.environ["K2_SKIN_USER"], "email": os.environ.get("K2_SKIN_EMAIL","")}))')
                    raw=$(cli_post_json "/cli/skin/users/email" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
em=d.get("email") or ""
print("Skin email %s for %s" % (("set to %s" % em) if em else "cleared", d.get("username","")))'
                    ;;
                full-name|fullname)
                    local username="" name=""
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --name) name="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --name=*) name="${1#--name=}"; shift ;;
                            --help|-h) echo "Usage: k2 app user full-name <username> [--name <text>]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" ]] && { echo "Usage: k2 app user full-name <username> [--name <text>]" >&2; exit 1; }
                    local body raw
                    body=$(K2_SKIN_USER="$username" K2_SKIN_FULL_NAME="$name" python3 -c '
import json,os
print(json.dumps({"username": os.environ["K2_SKIN_USER"], "fullName": os.environ.get("K2_SKIN_FULL_NAME","")}))')
                    raw=$(cli_post_json "/cli/skin/users/full-name" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
full=d.get("fullName") or d.get("full_name") or ""
print("Skin full name %s for %s" % (("set to %s" % full) if full else "cleared", d.get("username","")))'
                    ;;
                agent)
                    local username="" workspace="" clear=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --clear|--no-agent) clear=1; shift ;;
                            --workspace) workspace="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --workspace=*) workspace="${1#--workspace=}"; shift ;;
                            --help|-h) echo "Usage: k2 app user agent <username> <workspace> | k2 app user agent <username> --clear" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) if [[ -z "$username" ]]; then username="$1"; elif [[ -z "$workspace" ]]; then workspace="$1"; fi; shift ;;
                        esac
                    done
                    if [[ -z "$username" || ( "$clear" -eq 1 && -n "$workspace" ) || ( "$clear" -eq 0 && -z "$workspace" ) ]]; then
                        echo "Usage: k2 app user agent <username> <workspace> | k2 app user agent <username> --clear" >&2
                        exit 1
                    fi
                    local body raw
                    body=$(K2_SKIN_USER="$username" K2_SKIN_WS="$workspace" K2_SKIN_CLEAR="$clear" python3 -c '
import json,os
b={"username": os.environ["K2_SKIN_USER"]}
if os.environ.get("K2_SKIN_CLEAR","0") == "1":
    b["clear"]=True
else:
    b["workspace"]=os.environ["K2_SKIN_WS"]
print(json.dumps(b))')
                    raw=$(cli_post_json "/cli/skin/users/agent" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
ws=d.get("agentHandle") or d.get("agentWorkspace") or ""
if ws:
    print("App user %s is now the agent of %s (db actor kind: agent)" % (d.get("username",""), ws))
else:
    print("App user %s is a person again (db actor kind: guest)" % d.get("username",""))'
                    ;;
                list|"")
                    local raw roles_raw
                    raw=$(cli_request "/cli/skin/users")
                    printf '%s' "$raw" | _skin_die_if_err
                    roles_raw=$(cli_request "/cli/skin/roles")
                    printf '%s' "$roles_raw" | _skin_die_if_err
                    { printf '%s\n' "$raw"; printf '%s\n' "$roles_raw"; } | python3 -c '
import json,sys,time
users=json.loads(sys.stdin.readline() or "{}").get("users",[])
roles={ (r.get("name") or ""): r for r in json.loads(sys.stdin.readline() or "{}").get("roles",[]) }
if not users:
    print("No skin users."); sys.exit(0)
for u in users:
    name=u.get("username") or ""
    role=u.get("roleName") or u.get("role_name") or "—"
    if u.get("roleName") or u.get("role_name"):
        r=roles.get(role) or {}
        handles=r.get("roomHandles") or r.get("room_handles") or []
    else:
        handles=u.get("defaultRoomHandles") or u.get("default_room_handles") or []
    rooms=", ".join(handles) if handles else "—"
    # 0.46.5: an App login keeps its role and rooms on its App grant.
    if role == "—" and rooms == "—":
        gs=[g for g in (u.get("appGrants") or []) if g.get("role") or g.get("rooms")]
        if gs:
            role=", ".join("%s (App %s)" % (g.get("role"), g.get("app") or g.get("appServiceId")) for g in gs if g.get("role")) or "—"
            rh=[]
            for g in gs:
                for h in g.get("rooms") or []:
                    if h not in rh:
                        rh.append(h)
            rooms=", ".join(rh) if rh else "—"
    email=u.get("email") or "—"
    full=u.get("fullName") or u.get("full_name") or "—"
    if u.get("nameSetBySelf") and full != "—":
        full="%s (set by %s)" % (full, name)
    agent=u.get("agentHandle") or u.get("agentWorkspace") or ""
    tail=("  agent:%s" % agent) if agent else ""
    t=u.get("test")
    if t:
        tail += "  [test: App %s, %s]" % (t.get("app") or t.get("appServiceId"), "expired" if t.get("expired") else "ends %s" % time.strftime("%Y-%m-%d %H:%M", time.localtime(t.get("expiresAt") or 0)))
    if u.get("tiedTo"):
        tail += "  [%s: set up by %s'"'"'s agent]" % ("waiting for reset" if u.get("pendingReset") else "tied", u.get("tiedTo"))
    print("%s  %s  %s  %s  %s%s" % (name, full, role, rooms, email, tail))'
                    ;;
                remove|rm)
                    local username="${1:-}"
                    [[ -z "$username" ]] && { echo "Usage: k2 app user remove <username>" >&2; exit 1; }
                    local body raw
                    body=$(K2_SKIN_USER="$username" python3 -c 'import json,os; print(json.dumps({"username": os.environ["K2_SKIN_USER"]}))')
                    raw=$(cli_post_json "/cli/skin/users/remove" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Skin user removed." if d.get("success") else "No matching skin user.")'
                    ;;
                rooms)
                    local username="" agents=() app=""
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --agent|--agents) agents+=("${2:-}"); shift 2 ;;
                            --apply-tokens|--applyTokens) echo "--apply-tokens is gone: signed-in people get a rooms change at their next request, so there is nothing to apply. Run it without the flag." >&2; exit 2 ;;
                            --app) app="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --app=*) app="${1#--app=}"; shift ;;
                            --help|-h) echo "Usage: k2 app user rooms <username> --agent sales [--app <app>]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$username" ]] && username="$1"; shift ;;
                        esac
                    done
                    [[ -z "$username" ]] && { echo "Usage: k2 app user rooms <username> --agent sales" >&2; exit 1; }
                    local agents_nl="" a
                    if [ ${#agents[@]} -gt 0 ]; then
                        for a in "${agents[@]}"; do
                            agents_nl="${agents_nl}${a}"$'\n'
                        done
                    fi
                    local body raw
                    body=$(K2_SKIN_USER="$username" K2_SKIN_AGENTS="$agents_nl" K2_SKIN_APP="$app" python3 -c '
import json,os
b={"username": os.environ.get("K2_SKIN_USER","")}
b["rooms"]=[s.strip() for s in os.environ.get("K2_SKIN_AGENTS","").splitlines() if s.strip()]
app=os.environ.get("K2_SKIN_APP","").strip()
if app:
    b["app"]=app
print(json.dumps(b))')
                    raw=$(cli_post_json "/cli/skin/users/rooms" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
if d.get("app"):
    handles=d.get("roomHandles") or []
    print("App user %s rooms on %s: %s" % (d.get("username",""), d.get("app"), ", ".join(handles) if handles else "(none)"))
else:
    handles=d.get("defaultRoomHandles") or d.get("default_room_handles") or []
    print("Skin user %s default rooms: %s" % (d.get("username",""), ", ".join(handles) if handles else "(none)"))
w=d.get("waitingForReset") or {}
if w.get("rooms"):
    print("Waiting: everything else works now, but they can'"'"'t use room %s until they choose their own password, because you set this login up: first sign-in with a temporary password, an invite (%s), or your App'"'"'s forgot-password flow." % (", ".join(w["rooms"]), w.get("ask")))'
                    ;;
                role)
                    local username="${1:-}" role="${2:-}"
                    [[ -z "$username" || -z "$role" ]] && { echo "Usage: k2 app user role <username> <role>" >&2; exit 1; }
                    local body raw
                    body=$(K2_SKIN_USER="$username" K2_SKIN_ROLE="$role" python3 -c 'import json,os; print(json.dumps({"username": os.environ["K2_SKIN_USER"], "role": os.environ["K2_SKIN_ROLE"]}))')
                    raw=$(cli_post_json "/cli/skin/roles/assign" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Skin user %s role: %s" % (d.get("username",""), d.get("roleName") or d.get("role_name") or "—"))
w=d.get("waitingForReset") or {}
if w.get("rooms"):
    print("Waiting: everything else works now, but they can'"'"'t use room %s until they choose their own password, because you set this login up: first sign-in with a temporary password, an invite (%s), or your App'"'"'s forgot-password flow." % (", ".join(w["rooms"]), w.get("ask")))'
                    ;;
                unassign)
                    local username="${1:-}"
                    [[ -z "$username" ]] && { echo "Usage: k2 app user unassign <username>" >&2; exit 1; }
                    local body raw
                    body=$(K2_SKIN_USER="$username" python3 -c 'import json,os; print(json.dumps({"username": os.environ["K2_SKIN_USER"]}))')
                    raw=$(cli_post_json "/cli/skin/roles/unassign" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Skin user %s unassigned." % d.get("username",""))'
                    ;;
                *)
                    echo "Usage: k2 app user <add|list|remove|rooms|password|email|full-name|agent|role|unassign|unlock>" >&2
                    exit 1 ;;
            esac
            ;;
        role|roles)
            local _a
            for _a in "$@"; do
                if [[ "$_a" == "--user" || "$_a" == "--username" ]]; then
                    echo "Error: use k2 app user role <username> <role>, not k2 app role --user" >&2
                    exit 2
                fi
            done
            local action="${1:-list}"
            shift || true
            case "$action" in
                create)
                    local name="" caps="" agents=() app=""
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --caps|--cap) caps="${2:-}"; shift 2 ;;
                            --agent|--agents) agents+=("${2:-}"); shift 2 ;;
                            --app) app="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --app=*) app="${1#--app=}"; shift ;;
                            --help|-h) echo "Usage: k2 app role create <name> [--agent sales] [--app <app>]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$name" ]] && name="$1"; shift ;;
                        esac
                    done
                    [[ -z "$name" ]] && { echo "Usage: k2 app role create <name> [--agent sales]" >&2; exit 1; }
                    if [[ -n "$caps" ]]; then
                        echo "Error: use k2 app role room <role> <handle> [--caps …], not --caps with --agent" >&2
                        exit 2
                    fi
                    local agents_nl="" a
                    if [ ${#agents[@]} -gt 0 ]; then
                        for a in "${agents[@]}"; do
                            agents_nl="${agents_nl}${a}"$'\n'
                        done
                    fi
                    local body raw
                    body=$(K2_SKIN_NAME="$name" K2_SKIN_AGENTS="$agents_nl" K2_SKIN_APP="$app" python3 -c '
import json,os
b={"name": os.environ.get("K2_SKIN_NAME","")}
rooms=[s.strip() for s in os.environ.get("K2_SKIN_AGENTS","").splitlines() if s.strip()]
if rooms:
    b["rooms"]=rooms
app=os.environ.get("K2_SKIN_APP","").strip()
if app:
    b["app"]=app
print(json.dumps(b))')
                    raw=$(cli_post_json "/cli/skin/roles" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
access=d.get("roomAccess") or d.get("room_access") or []
if not access:
    print("Skin role created: %s  (none)" % (d.get("name",""),))
    sys.exit(0)
print("Skin role created: %s" % d.get("name",""))
for a in access:
    caps=", ".join(a.get("caps") or []) or "thread-only"
    print("  %s  %s" % (a.get("handle") or "", caps))'
                    ;;
                list|"")
                    local raw
                    raw=$(cli_request "/cli/skin/roles")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
roles=d.get("roles",[])
if not roles:
    print("No skin roles."); sys.exit(0)
for r in roles:
    print(r.get("name") or "")
    access=r.get("roomAccess") or r.get("room_access") or []
    if not access:
        print("  (none)")
        continue
    for a in access:
        caps=",".join(a.get("caps") or []) or "thread-only"
        print("  %s  %s" % (a.get("handle") or "", caps))
        f=a.get("folders") or {}
        if f.get("read"):
            print("      read:   %s" % ", ".join(f["read"]))
        if f.get("write"):
            print("      write:  %s" % ", ".join(f["write"]))
        if f.get("uploadDir"):
            print("      upload: %s" % f["uploadDir"])'
                    ;;
                room)
                    local role="" handle="" caps="" clear=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --caps|--cap) caps="${2:-}"; shift 2 ;;
                            --clear) clear=1; shift ;;
                            --help|-h) echo "Usage: k2 app role room <role> <handle> [--caps thread:read,thread:post,files:read] | --clear" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *)
                                if [[ -z "$role" ]]; then role="$1"
                                elif [[ -z "$handle" ]]; then handle="$1"
                                fi
                                shift ;;
                        esac
                    done
                    [[ -z "$role" || -z "$handle" ]] && { echo "Usage: k2 app role room <role> <handle> [--caps …] | --clear" >&2; exit 1; }
                    local body raw
                    body=$(K2_SKIN_ROLE="$role" K2_SKIN_HANDLE="$handle" K2_SKIN_CAPS="$caps" K2_SKIN_CLEAR="$clear" python3 -c '
import json,os
b={"name": os.environ.get("K2_SKIN_ROLE",""), "handle": os.environ.get("K2_SKIN_HANDLE","")}
if os.environ.get("K2_SKIN_CLEAR","0") == "1":
    b["clear"]=True
else:
    c=os.environ.get("K2_SKIN_CAPS","").strip()
    if c:
        b["caps"]=[s.strip() for s in c.split(",") if s.strip()]
print(json.dumps(b))')
                    raw=$(cli_post_json "/cli/skin/roles/room" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Skin role %s" % (d.get("name") or ""))
access=d.get("roomAccess") or d.get("room_access") or []
if not access:
    print("  (none)")
    sys.exit(0)
for a in access:
    caps=", ".join(a.get("caps") or []) or "thread-only"
    print("  %s  %s" % (a.get("handle") or "", caps))
    f=a.get("folders") or {}
    if f.get("read") or f.get("write") or f.get("upload") or f.get("download"):
        print("      folders: %s" % ", ".join((f.get("read") or []) + [w + " (write)" for w in (f.get("write") or [])] + [u + " (drop)" for u in (f.get("upload") or [])] + [x + " (save)" for x in (f.get("download") or [])]))'
                    ;;
                paths|folders)
                    # Room-scope R3 (RS-A1..A3): one room line's folders.
                    local role="" handle="" read="" write="" upload="" drop="" download="" nodownload=0 clear=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --read) read="${2:-}"; shift 2 ;;
                            --write) write="${2:-}"; shift 2 ;;
                            --upload) drop="${2:-}"; shift 2 ;;
                            --upload-dir) upload="${2:-}"; shift 2 ;;
                            --download) download="${2:-}"; shift 2 ;;
                            --no-download) nodownload=1; shift ;;
                            --clear) clear=1; shift ;;
                            --help|-h) echo "Usage: k2 app role paths <role> <room> [--read a/,b/] [--write c/] [--upload d/] [--upload-dir c/in] [--download e/,f.pdf | --no-download] | --clear" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *)
                                if [[ -z "$role" ]]; then role="$1"
                                elif [[ -z "$handle" ]]; then handle="$1"
                                else echo "Unexpected argument: $1" >&2; exit 1
                                fi
                                shift ;;
                        esac
                    done
                    [[ -z "$role" || -z "$handle" ]] && { echo "Usage: k2 app role paths <role> <room> [--read a/,b/] [--write c/] [--upload d/] [--upload-dir c/in] [--download e/,f.pdf | --no-download] | --clear" >&2; exit 1; }
                    if [[ "$clear" == "1" && ( -n "$read" || -n "$write" || -n "$upload" || -n "$drop" || -n "$download" || "$nodownload" == "1" ) ]]; then
                        echo "Error: --clear puts the room back to the whole room; don't pass folders with it" >&2
                        exit 2
                    fi
                    if [[ -n "$download" && "$nodownload" == "1" ]]; then
                        echo "Error: --download sets the download entries, --no-download clears them; pass one" >&2
                        exit 2
                    fi
                    if [[ "$clear" == "0" && -z "$read" && -z "$write" && -z "$upload" && -z "$drop" && -z "$download" && "$nodownload" == "0" ]]; then
                        echo "Error: name folders (--read, --write, --upload, --upload-dir, --download) or pass --clear for the whole room" >&2
                        exit 2
                    fi
                    local body raw
                    body=$(K2_SKIN_ROLE="$role" K2_SKIN_HANDLE="$handle" K2_SKIN_READ="$read" K2_SKIN_WRITE="$write" K2_SKIN_DROP="$drop" K2_SKIN_UPLOAD="$upload" K2_SKIN_DOWNLOAD="$download" K2_SKIN_NODOWNLOAD="$nodownload" K2_SKIN_CLEAR="$clear" python3 -c '
import json,os
b={"name": os.environ["K2_SKIN_ROLE"], "handle": os.environ["K2_SKIN_HANDLE"]}
if os.environ.get("K2_SKIN_CLEAR","0") == "1":
    b["clear"]=True
else:
    for key, env in (("read","K2_SKIN_READ"), ("write","K2_SKIN_WRITE"), ("upload","K2_SKIN_DROP")):
        v=[s.strip() for s in os.environ.get(env,"").split(",") if s.strip()]
        if v:
            b[key]=v
    u=os.environ.get("K2_SKIN_UPLOAD","").strip()
    if u:
        b["uploadDir"]=u
    # Download entries (0.46.4): omitted = kept; --no-download clears them.
    d=[s.strip() for s in os.environ.get("K2_SKIN_DOWNLOAD","").split(",") if s.strip()]
    if d:
        b["download"]=d
    elif os.environ.get("K2_SKIN_NODOWNLOAD","0") == "1":
        b["download"]=[]
print(json.dumps(b))')
                    raw=$(cli_post_json "/cli/skin/roles/paths" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | K2_SKIN_HANDLE="$handle" python3 -c '
import json,os,sys
d=json.load(sys.stdin)
want=os.environ.get("K2_SKIN_HANDLE","")
print("Skin role %s" % (d.get("name") or ""))
for a in d.get("roomAccess") or d.get("room_access") or []:
    if a.get("handle") != want:
        continue
    f=a.get("folders") or {}
    caps=", ".join(a.get("caps") or []) or "thread-only"
    print("  %s  %s" % (a.get("handle") or "", caps))
    if not (f.get("read") or f.get("write") or f.get("upload")):
        print("      folders: whole room")
    if f.get("read"):
        print("      read:   %s" % ", ".join(f["read"]))
    if f.get("write"):
        print("      write:  %s" % ", ".join(f["write"]))
    if f.get("upload"):
        print("      drop:   %s (files:upload: add only, never list or read)" % ", ".join(f["upload"]))
    if f.get("uploadDir"):
        print("      upload: %s" % f["uploadDir"])
    if f.get("download"):
        print("      save:   %s (files:download: may save these)" % ", ".join(f["download"]))'
                    ;;
                update)
                    local name="" caps="" clear=0 agents=()
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --caps|--cap) caps="${2:-}"; shift 2 ;;
                            --agent|--agents) agents+=("${2:-}"); shift 2 ;;
                            --clear) clear=1; shift ;;
                            --help|-h) echo "Usage: k2 app role update <name> [--agent … | --clear]" >&2; exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$name" ]] && name="$1"; shift ;;
                        esac
                    done
                    [[ -z "$name" ]] && { echo "Usage: k2 app role update <name> [--agent … | --clear]" >&2; exit 1; }
                    if [[ -n "$caps" ]]; then
                        echo "Error: use k2 app role room <role> <handle> [--caps …], not --caps with --agent" >&2
                        exit 2
                    fi
                    local agents_nl="" a
                    if [ ${#agents[@]} -gt 0 ]; then
                        for a in "${agents[@]}"; do
                            agents_nl="${agents_nl}${a}"$'\n'
                        done
                    fi
                    local body raw
                    body=$(K2_SKIN_NAME="$name" K2_SKIN_AGENTS="$agents_nl" K2_SKIN_CLEAR="$clear" python3 -c '
import json,os
b={"name": os.environ.get("K2_SKIN_NAME","")}
if os.environ.get("K2_SKIN_CLEAR","0") == "1":
    b["roomAccess"]=[]
elif os.environ.get("K2_SKIN_AGENTS","").strip():
    b["rooms"]=[s.strip() for s in os.environ.get("K2_SKIN_AGENTS","").splitlines() if s.strip()]
print(json.dumps(b))')
                    raw=$(cli_post_json "/cli/skin/roles/update" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Skin role updated: %s" % (d.get("name") or ""))
access=d.get("roomAccess") or d.get("room_access") or []
if not access:
    print("  (none)")
    sys.exit(0)
for a in access:
    caps=", ".join(a.get("caps") or []) or "thread-only"
    print("  %s  %s" % (a.get("handle") or "", caps))'
                    ;;
                remove|rm)
                    local name="${1:-}"
                    [[ -z "$name" ]] && { echo "Usage: k2 app role remove <name>" >&2; exit 1; }
                    local body raw
                    body=$(K2_SKIN_NAME="$name" python3 -c 'import json,os; print(json.dumps({"name": os.environ["K2_SKIN_NAME"]}))')
                    raw=$(cli_post_json "/cli/skin/roles/remove" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Skin role removed." if d.get("success") else "No matching skin role.")'
                    ;;
                suggest|suggestions|templates)
                    # app-mobile-access-v1: the suggested role templates.
                    # Nothing is created until `k2 app roles add --from <id>`.
                    local json_out=0
                    [ "${1:-}" = "--json" ] && json_out=1
                    _compute_call GET /cli/skin/roles/suggest ""
                    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
                    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
for t in json.load(sys.stdin).get("templates") or []:
    print("%-8s %s: %s" % (t["id"], t["label"], t["description"]))
    print("         caps: %s" % (", ".join(t.get("caps") or []) or "none (the App decides)"))
    f = t.get("folders")
    if f:
        print("         folders: read %s; write %s; uploads to %s" % (", ".join(f.get("read") or []) or "-", ", ".join(f.get("write") or []) or "-", f.get("uploadDir") or "-"))
    if t.get("note"):
        print("         note: %s" % t["note"])
print("Make one: k2 app roles add --from <id> --app <app> [--name <name>] [--agent <room> …]")'
                    ;;
                add)
                    local from="" app="" name="" agents=()
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --from|--template) from="${2:-}"; shift 2 ;;
                            --app) app="${2:-}"; shift 2 ;;
                            --name) name="${2:-}"; shift 2 ;;
                            --agent|--agents|--room) agents+=("${2:-}"); shift 2 ;;
                            --help|-h) echo "Usage: k2 app roles add --from <template> --app <app> [--name <name>] [--agent <room> …]   (k2 app roles suggest lists the templates)"; exit 0 ;;
                            *) echo "Unknown argument: $1 (k2 app roles add --from <template> --app <app>)" >&2; exit 2 ;;
                        esac
                    done
                    [[ -z "$from" ]] && { echo "Usage: k2 app roles add --from <template> --app <app> [--name <name>] [--agent <room> …]" >&2; exit 2; }
                    local agents_nl="" a
                    if [ ${#agents[@]} -gt 0 ]; then
                        for a in "${agents[@]}"; do agents_nl+="$a"$'\n'; done
                    fi
                    local body raw
                    body=$(K2_RT_FROM="$from" K2_RT_APP="$app" K2_RT_NAME="$name" K2_RT_AGENTS="$agents_nl" python3 -c '
import json, os
b = {"template": os.environ["K2_RT_FROM"]}
for k, e in (("app", "K2_RT_APP"), ("name", "K2_RT_NAME")):
    if os.environ.get(e, "").strip():
        b[k] = os.environ[e].strip()
rooms = [s.strip() for s in os.environ.get("K2_RT_AGENTS", "").splitlines() if s.strip()]
if rooms:
    b["rooms"] = rooms
print(json.dumps(b))')
                    raw=$(cli_post_json "/cli/skin/roles/from-template" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json, sys
d = json.load(sys.stdin)
r = d.get("role") or {}
print("Role %s made from %s (edit it like any role: k2 app role room|paths|update %s)" % (r.get("name"), d.get("template"), r.get("name")))
for line in r.get("roomAccess") or []:
    print("  %-20s %s" % (line.get("handle"), ", ".join(line.get("caps") or [])))'
                    ;;
                --help|-h)
                    echo "Usage: k2 app role <create|list|room|paths|update|remove|suggest|add --from>" >&2
                    exit 0
                    ;;
                *)
                    echo "Usage: k2 app role <create|list|room|paths|update|remove|suggest|add --from>" >&2
                    exit 1 ;;
            esac
            ;;
        grant|grants)
            local action="${1:-list}"
            shift || true
            case "$action" in
                list|"")
                    local raw
                    # An agent (a passport) sees the logins on its own
                    # workspace's Apps; the owner sees every grant.
                    if [ -n "${SCOPED_TOKEN:-}" ]; then
                        raw=$(cli_request "/cli/skin/grants/own")
                    else
                        raw=$(cli_request "/cli/skin/grants")
                    fi
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
grants=d.get("grants") or []
if not grants:
    print("No grants." if not d.get("scope") else "No logins on your Apps.")
    sys.exit(0)
for g in grants:
    if d.get("scope"):
        what=("role " + g["role"]) if g.get("role") else ("rooms " + ",".join(g.get("rooms") or []))
        print("%s  %s on %s  %s%s" % (g.get("id") or "", g.get("username") or g.get("subjectId") or "", g.get("app") or g.get("targetId") or "", what, "" if g.get("enabled", True) else "  [off]"))
        continue
    role=g.get("roleId") or g.get("role_id") or ""
    scope=g.get("scope") or ""
    extra=("role "+role) if role else (("scope "+scope) if scope else "")
    hs=g.get("hookState")
    if hs:
        extra += "  [%s%s]" % (hs, (": " + g["hookError"]) if g.get("hookError") else "")
    print("%s  %s %s  %s %s  %s" % (
        g.get("id") or "",
        g.get("subjectKind") or g.get("subject_kind") or "",
        g.get("subjectId") or g.get("subject_id") or "",
        g.get("kind") or "",
        g.get("targetId") or g.get("target_id") or "",
        extra,
    ))'
                    ;;
                add|create)
                    local subject_kind="" subject="" kind="" target="" role="" scope="" rooms_nl=""
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --subject-kind) subject_kind="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --subject-kind=*) subject_kind="${1#--subject-kind=}"; shift ;;
                            --subject) subject="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --subject=*) subject="${1#--subject=}"; shift ;;
                            --kind) kind="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --kind=*) kind="${1#--kind=}"; shift ;;
                            --target) target="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --target=*) target="${1#--target=}"; shift ;;
                            --role) role="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --role=*) role="${1#--role=}"; shift ;;
                            --scope) scope="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --scope=*) scope="${1#--scope=}"; shift ;;
                            --room) rooms_nl="${rooms_nl}${2-}"$'\n'; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --room=*) rooms_nl="${rooms_nl}${1#--room=}"$'\n'; shift ;;
                            --help|-h)
                                echo "Usage: k2 app grant add --subject-kind principal|workspace --subject <id> --kind app|mailbox|database|cli|chat --target <id> [--role <role> | --room <handle>…] [--scope <text>]" >&2
                                echo "       k2 app grant add --kind chat --subject-kind principal|workspace|connect --subject <name|handle|login|id> [--scope member|guest|coworker|assistant:<person>]" >&2
                                echo "       chat grants create the chat account; roles and channels are set in Sew." >&2
                                exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) echo "Unknown argument: $1" >&2; exit 1 ;;
                        esac
                    done
                    # Sew S27: chat has one target (sew); the daemon fills it in.
                    if [[ "$(printf %s "$kind" | tr "[:upper:]" "[:lower:]")" == "chat" && -z "$target" ]]; then
                        target="sew"
                    fi
                    if [[ -z "$subject_kind" || -z "$subject" || -z "$kind" || -z "$target" ]]; then
                        echo "Usage: k2 app grant add --subject-kind principal|workspace --subject <id> --kind app|mailbox|database|cli|chat --target <id> [--role <role>] [--scope <text>]" >&2
                        exit 1
                    fi
                    local body raw
                    body=$(K2_GRANT_SUBJECT_KIND="$subject_kind" K2_GRANT_SUBJECT="$subject" K2_GRANT_KIND="$kind" K2_GRANT_TARGET="$target" K2_GRANT_ROLE="$role" K2_GRANT_SCOPE="$scope" K2_GRANT_ROOMS="$rooms_nl" python3 -c '
import json,os
b={
  "subjectKind": os.environ.get("K2_GRANT_SUBJECT_KIND",""),
  "subjectId": os.environ.get("K2_GRANT_SUBJECT",""),
  "kind": os.environ.get("K2_GRANT_KIND",""),
  "targetId": os.environ.get("K2_GRANT_TARGET",""),
}
role=os.environ.get("K2_GRANT_ROLE","").strip()
scope=os.environ.get("K2_GRANT_SCOPE","").strip()
rooms=[r.strip() for r in os.environ.get("K2_GRANT_ROOMS","").splitlines() if r.strip()]
if role:
    b["roleId"]=role
if scope:
    b["scope"]=scope
if rooms:
    b["rooms"]=rooms
print(json.dumps(b))')
                    raw=$(cli_post_json "/cli/skin/grants" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Grant %s  %s %s  %s %s%s" % (
    d.get("id") or "",
    d.get("subjectKind") or "",
    d.get("subjectId") or "",
    d.get("kind") or "",
    d.get("targetId") or "",
    ("  scope " + d["scope"]) if d.get("kind") == "chat" and d.get("scope") else "",
))
if d.get("kind") == "chat":
    print("Queued for chat. Check with: k2 app grant hooks")'
                    ;;
                rm|remove|delete|revoke)
                    local subject_kind="" subject="" kind="" target="" gid=""
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --id) gid="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --id=*) gid="${1#--id=}"; shift ;;
                            --subject-kind) subject_kind="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --subject-kind=*) subject_kind="${1#--subject-kind=}"; shift ;;
                            --subject) subject="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --subject=*) subject="${1#--subject=}"; shift ;;
                            --kind) kind="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --kind=*) kind="${1#--kind=}"; shift ;;
                            --target) target="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --target=*) target="${1#--target=}"; shift ;;
                            --help|-h)
                                echo "Usage: k2 app grant rm --id <id>" >&2
                                echo "       k2 app grant rm --subject-kind principal|workspace --subject <id> --kind <kind> --target <id>" >&2
                                exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) echo "Unknown argument: $1" >&2; exit 1 ;;
                        esac
                    done
                    if [[ -z "$gid" && "$(printf %s "$kind" | tr "[:upper:]" "[:lower:]")" == "chat" && -z "$target" ]]; then
                        target="sew"
                    fi
                    if [[ -z "$gid" && ( -z "$subject_kind" || -z "$subject" || -z "$kind" || -z "$target" ) ]]; then
                        echo "Usage: k2 app grant rm --id <id>" >&2
                        echo "       k2 app grant rm --subject-kind principal|workspace --subject <id> --kind <kind> --target <id>" >&2
                        exit 1
                    fi
                    local body raw
                    body=$(K2_GRANT_ID="$gid" K2_GRANT_SUBJECT_KIND="$subject_kind" K2_GRANT_SUBJECT="$subject" K2_GRANT_KIND="$kind" K2_GRANT_TARGET="$target" python3 -c '
import json,os
gid=os.environ.get("K2_GRANT_ID","").strip()
if gid:
    print(json.dumps({"id": gid}))
else:
    print(json.dumps({
        "subjectKind": os.environ.get("K2_GRANT_SUBJECT_KIND",""),
        "subjectId": os.environ.get("K2_GRANT_SUBJECT",""),
        "kind": os.environ.get("K2_GRANT_KIND",""),
        "targetId": os.environ.get("K2_GRANT_TARGET",""),
    }))')
                    raw=$(cli_post_json "/cli/skin/grants/delete" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Grant removed." if d.get("success") else "No matching grant.")
for c in d.get("cascaded") or []:
    print("  also removed %s grant %s (%s %s, scope %s)" % (c.get("kind",""), c.get("id",""), c.get("subjectKind",""), c.get("subjectId",""), c.get("scope","")))'
                    ;;
                set|update)
                    local gid="" scope=""
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --id) gid="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --id=*) gid="${1#--id=}"; shift ;;
                            --scope) scope="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --scope=*) scope="${1#--scope=}"; shift ;;
                            --help|-h)
                                echo "Usage: k2 app grant set --id <grant-id> --scope <scope>   (chat grants only)" >&2
                                exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) echo "Unknown argument: $1" >&2; exit 1 ;;
                        esac
                    done
                    if [[ -z "$gid" || -z "$scope" ]]; then
                        echo "Usage: k2 app grant set --id <grant-id> --scope <scope>   (chat grants only)" >&2
                        exit 1
                    fi
                    local body raw
                    body=$(K2_GRANT_ID="$gid" K2_GRANT_SCOPE="$scope" python3 -c '
import json,os
print(json.dumps({"id": os.environ["K2_GRANT_ID"], "scope": os.environ["K2_GRANT_SCOPE"]}))')
                    raw=$(cli_post_json "/cli/skin/grants/update" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Grant %s scope %s" % (d.get("id") or "", d.get("scope") or ""))'
                    ;;
                role)
                    local gid="" role=""
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --id) gid="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --id=*) gid="${1#--id=}"; shift ;;
                            --role) role="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                            --role=*) role="${1#--role=}"; shift ;;
                            --help|-h)
                                echo "Usage: k2 app grant role <grant-id> --role <role>   (an App login, kind=app; owner only)" >&2
                                echo "  Re-roles the login in place: live logins stay signed in and get the new role's rooms now." >&2
                                exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) if [[ -z "$gid" ]]; then gid="$1"; shift; else echo "Unknown argument: $1" >&2; exit 1; fi ;;
                        esac
                    done
                    if [[ -z "$gid" || -z "$role" ]]; then
                        echo "Usage: k2 app grant role <grant-id> --role <role>   (an App login, kind=app; owner only)" >&2
                        exit 1
                    fi
                    local body raw
                    body=$(K2_GRANT_ID="$gid" K2_GRANT_ROLE="$role" python3 -c '
import json,os
print(json.dumps({"id": os.environ["K2_GRANT_ID"], "role": os.environ["K2_GRANT_ROLE"]}))')
                    raw=$(cli_post_json "/cli/skin/grants/role" "$body")
                    printf '%s' "$raw" | _skin_die_if_err
                    printf '%s' "$raw" | K2_GRANT_ROLE="$role" python3 -c '
import json,os,sys
d=json.load(sys.stdin)
if d.get("changed"):
    print("Grant %s  now role %s (%s on %s). Live logins kept; they have the new role now." % (
        d.get("id") or "", os.environ["K2_GRANT_ROLE"], d.get("subjectId") or "", d.get("targetId") or ""))
else:
    print("Grant %s already has role %s. Nothing changed." % (d.get("id") or "", os.environ["K2_GRANT_ROLE"]))'
                    ;;
                hooks|hook)
                    _app_grant_hooks "$@"
                    ;;
                copy-host|copyhost)
                    local app="" dry_run=0 json_out=0
                    while [[ $# -gt 0 ]]; do
                        case "$1" in
                            --dry-run) dry_run=1; shift ;;
                            --json) json_out=1; shift ;;
                            --help|-h)
                                echo "Usage: k2 app grant copy-host <app> [--dry-run] [--json]   (<app> = App id, service id or <workspace>/<name>)" >&2
                                echo "  The owner: any App, at once. An agent: its own workspace's App, as one held request listing the" >&2
                                echo "  logins (exit 2); the owner approves it from a card on the agent's Thread or Settings → App Approvals." >&2
                                exit 0 ;;
                            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                            *) [[ -z "$app" ]] && app="$1"; shift ;;
                        esac
                    done
                    [[ -z "$app" ]] && { echo "Usage: k2 app grant copy-host <app> [--dry-run] [--json]" >&2; exit 1; }
                    local dry_b=""
                    [ "$dry_run" = 1 ] && dry_b=1
                    # 0.46.4: the passport-aware door (the owner's older
                    # /cli/skin/grants/copy-host stays for scripts).
                    _app_rt_post /cli/app/grant/copy-host "$(_compute_json K2_CJ_app="$app" K2_CJ_dryRun_b="$dry_b")"
                    # An agent's copy is one held request: a card for the owner.
                    _app_grace_done "$json_out"
                    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
                    if [ "$json_out" = 1 ]; then
                        printf '%s\n' "$_CP_BODY"
                        return 0
                    fi
                    printf '%s' "$_CP_BODY" | python3 -c '
import json,sys
d=json.load(sys.stdin)
verb="Would copy" if d.get("dryRun") else "Copied"
added=d.get("added") or []
a=d.get("appInfo") or {}
n=len(added) if "added" in d else int(d.get("addedCount") or 0)
print("%s the box login onto App %s for %d user(s)%s" % (verb, a.get("handle") or d.get("app",""), n, (": " + ", ".join(added)) if added else ""))
for k in ("keptExistingCount","skippedOtherAppRoleCount","skippedOtherWorkspaceAgentCount"):
    if d.get(k):
        print("  %s: %d" % ({"keptExistingCount":"kept (an App grant already exists)","skippedOtherAppRoleCount":"skipped (role tagged for another App)","skippedOtherWorkspaceAgentCount":"skipped (another workspace'"'"'s agent added it)"}[k], d[k]))
if d.get("note"):
    print("  " + d["note"])
for k in d.get("keptExisting") or []:
    print("  kept %s: an App grant already exists (%s)" % (k.get("username",""), "enabled" if k.get("enabled") else "disabled"))
for u in d.get("skippedOtherAppRole") or []:
    print("  skipped %s: the box-login role is tagged for another App" % u)
for u in d.get("skippedOtherWorkspaceAgent") or []:
    print("  skipped %s: another workspace'"'"'s agent added it" % u)'
                    ;;
                --help|-h)
                    echo "Usage: k2 app grant <list|add|set|rm|copy-host|hooks>" >&2
                    exit 0
                    ;;
                *)
                    echo "Usage: k2 app grant <list|add|set|rm|copy-host|hooks>" >&2
                    exit 1 ;;
            esac
            ;;
        front-door|frontdoor)
            local json_out=0 mode="" url="" hint="" ui_port=""
            while [[ $# -gt 0 ]]; do
                case "$1" in
                    --json) json_out=1; shift ;;
                    --url) url="${2:-}"; shift 2 ;;
                    --hint) hint="${2:-}"; shift 2 ;;
                    --ui-port|--uiPort) ui_port="${2:-}"; shift 2 ;;
                    connect|direct)
                        if [ -n "$mode" ]; then
                            echo "Usage: k2 app front-door [connect|direct] [--json]" >&2
                            exit 1
                        fi
                        mode="$1"; shift ;;
                    --help|-h)
                        echo "Usage: k2 app front-door [connect|direct] [--url URL] [--hint TEXT] [--ui-port N] [--json]" >&2
                        exit 0 ;;
                    *) echo "Unknown flag: $1" >&2; exit 1 ;;
                esac
            done
            if [ -z "$mode" ]; then
                local raw
                raw=$(cli_request "/cli/skin/front-door")
                printf '%s' "$raw" | _skin_die_if_err
                if [ "$json_out" = 1 ]; then
                    printf '%s\n' "$raw"
                    return 0
                fi
                printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
err=d.get("error")
print("mode:     %s" % (d.get("mode") or "-"))
print("url:      %s" % (d.get("url") or d.get("connectUrl") or "(none)"))
print("listen:   %s" % (d.get("listen") or "-"))
print("applied:  %s" % d.get("applied"))
c=d.get("caddy") or {}
if c.get("missing"):
    print("caddy:    missing — brew install caddy  or  sudo apt install caddy")
else:
    print("caddy:    running=%s pid=%s" % (c.get("running"), c.get("pid")))
n=d.get("nested") or {}
print("nested:   %s registered=%s" % (n.get("host") or n.get("label") or "-", n.get("registered")))
if d.get("hint"):
    print("hint:     %s" % d["hint"])
if isinstance(err,str) and err.strip():
    print("error:    %s" % err, file=sys.stderr)
'
                return 0
            fi
            local body
            body=$(K2_SKIN_MODE="$mode" K2_SKIN_URL="$url" K2_SKIN_HINT="$hint" K2_SKIN_UI="$ui_port" python3 -c '
import json,os
b={"mode": os.environ.get("K2_SKIN_MODE",""), "apply": True}
u=os.environ.get("K2_SKIN_URL","").strip()
if u: b["url"]=u
h=os.environ.get("K2_SKIN_HINT","").strip()
if h: b["hint"]=h
p=os.environ.get("K2_SKIN_UI","").strip()
if p:
    try: b["uiPort"]=int(p)
    except ValueError: b["uiPort"]=p
print(json.dumps(b))')
            local raw
            raw=$(cli_post_json "/cli/skin/front-door" "$body")
            if [ "$json_out" = 1 ]; then
                printf '%s\n' "$raw"
                printf '%s' "$raw" | python3 -c '
import json,sys
try:
    d=json.load(sys.stdin)
except Exception:
    sys.exit(1)
err=d.get("error")
if isinstance(err, dict):
    code=(err.get("code") or "")
    sys.exit(3 if code in ("owner_only","forbidden","gated") else 1)
if isinstance(err,str) and err.strip() and not d.get("applied"):
    low=err.lower()
    sys.exit(3 if ("token" in low or "forbidden" in low or "owner" in low) else 1)
' || exit $?
                return 0
            fi
            printf '%s' "$raw" | python3 -c '
import json,sys
try:
    d=json.load(sys.stdin)
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)
err=d.get("error")
if isinstance(err, dict):
    code=err.get("code") or "error"
    hint=err.get("hint") or err.get("message") or code
    if code in ("owner_only","forbidden","gated") and "ask your human" not in hint:
        hint=(hint+" — " if hint else "")+"requires owner/admin — ask your human"
    print("Error: %s" % hint, file=sys.stderr)
    sys.exit(3 if code in ("owner_only","forbidden","gated") else 1)
if isinstance(err,str) and err.strip() and not d.get("applied") and not d.get("mode"):
    print("Error: %s" % err, file=sys.stderr)
    low=err.lower()
    sys.exit(3 if ("token" in low or "forbidden" in low or "owner" in low) else 1)
if isinstance(err,str) and err.strip() and (
    "caddy_missing" in err or "bind_failed" in err or "pro_required" in err or "no_token" in err
) and not d.get("applied"):
    print("Error: %s" % err, file=sys.stderr); sys.exit(1)
print("Front door: %s  %s" % (d.get("mode","?"), d.get("url") or d.get("connectUrl") or "(no url yet)"))
print("listen:  %s  applied=%s" % (d.get("listen") or "-", d.get("applied")))
if d.get("hint"):
    print(d["hint"])
if isinstance(err,str) and err.strip():
    print(err, file=sys.stderr)
    if not d.get("applied"):
        sys.exit(1)
'
            ;;
        hydra)
            local json_out=0 action=""
            while [[ $# -gt 0 ]]; do
                case "$1" in
                    --json) json_out=1; shift ;;
                    --help|-h)
                        echo "Usage: k2 app hydra [on|off] [--json]" >&2
                        exit 0 ;;
                    on|off)
                        if [ -n "$action" ]; then
                            echo "Usage: k2 app hydra [on|off] [--json]" >&2
                            exit 2
                        fi
                        action="$1"; shift ;;
                    *)
                        echo "Usage: k2 app hydra [on|off] [--json]" >&2
                        exit 2 ;;
                esac
            done
            local raw
            if [ -z "$action" ]; then
                raw=$(cli_request "/cli/skin/hydra")
            else
                local body
                if [ "$action" = "on" ]; then
                    body='{"enabled":true,"apply":true}'
                else
                    body='{"enabled":false,"apply":true}'
                fi
                raw=$(cli_post_json "/cli/skin/hydra" "$body")
            fi
            printf '%s' "$raw" | _skin_die_if_err
            if [ "$json_out" = 1 ]; then
                printf '%s\n' "$raw"
                return 0
            fi
            printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("supported: %s" % d.get("supported"))
print("enabled:   %s" % d.get("enabled"))
print("running:   %s" % d.get("running"))
print("public:    %s" % (d.get("publicUrl") or ""))
print("admin:     %s" % (d.get("adminUrl") or ""))
if d.get("hint"):
    print("hint:      %s" % d["hint"])
'
            ;;
        --help|-h|"")
            _skin_usage
            [[ -z "$sub" ]] && exit 1
            exit 0
            ;;
        *)
            _skin_usage
            exit 1 ;;
    esac
}

cmd_skin_token() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        create)
            local name="" caps="" agents=()
            while [[ $# -gt 0 ]]; do
                case "$1" in
                    --name) name="${2:-}"; shift 2 ;;
                    --caps|--cap) caps="${2:-}"; shift 2 ;;
                    --agent|--agents) agents+=("${2:-}"); shift 2 ;;
                    --user|--username)
                        echo "Error: use --name (platform label), not --user/--username" >&2
                        echo "Usage: k2 app token create --name <label> --agent sales [--caps thread:read,thread:post,files:read,files:write]" >&2
                        exit 2 ;;
                    --help|-h) echo "Usage: k2 app token create --name <label> --agent sales [--caps thread:read,thread:post,files:read,files:write]" >&2; exit 0 ;;
                    -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                    *)
                        echo "Error: use --name (platform label), not a positional username" >&2
                        echo "Usage: k2 app token create --name <label> --agent sales [--caps thread:read,thread:post,files:read,files:write]" >&2
                        exit 2 ;;
                esac
            done
            [[ -z "$name" ]] && { echo "Error: use --name (platform label), not a positional username" >&2; echo "Usage: k2 app token create --name <label> --agent sales [--caps thread:read,thread:post,files:read,files:write]" >&2; exit 2; }
            [[ ${#agents[@]} -eq 0 ]] && { echo "Error: k2 app token create needs ≥1 --agent <handle> (rooms). Empty rooms = Thread dark." >&2; echo "Usage: k2 app token create --name <label> --agent sales [--caps thread:read,thread:post,files:read,files:write]" >&2; exit 2; }
            local agents_nl="" a
            for a in "${agents[@]}"; do
                agents_nl="${agents_nl}${a}"$'\n'
            done
            local body raw
            body=$(K2_SKIN_NAME="$name" K2_SKIN_CAPS="$caps" K2_SKIN_AGENTS="$agents_nl" python3 -c '
import json,os
b={"name": os.environ.get("K2_SKIN_NAME","")}
c=os.environ.get("K2_SKIN_CAPS","").strip()
if c:
    b["caps"]=[s.strip() for s in c.split(",") if s.strip()]
b["rooms"]=[s.strip() for s in os.environ.get("K2_SKIN_AGENTS","").splitlines() if s.strip()]
print(json.dumps(b))')
            raw=$(cli_post_json "/cli/skin-tokens" "$body")
            printf '%s' "$raw" | _skin_die_if_err
            printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Skin token created (id: %s) name %s." % (d.get("id",""), d.get("name") or d.get("username","")))
caps=d.get("caps") or []
print("Capabilities: %s" % (", ".join(caps) if caps else "(none)"))
handles=d.get("roomHandles") or d.get("room_handles") or []
print("Rooms: %s" % (", ".join(handles) if handles else "(none)"))
print("")
print("  %s" % d.get("token",""))
print("")
print("STORE THIS KEY NOW — it is shown only once and cannot be retrieved again.")'
            ;;
        list)
            local raw
            raw=$(cli_request "/cli/skin-tokens")
            printf '%s' "$raw" | _skin_die_if_err
            printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
tokens=d.get("tokens",[])
if not tokens:
    print("No App tokens."); sys.exit(0)
for t in tokens:
    state="revoked" if t.get("revokedAt") else "active"
    caps=",".join(t.get("caps") or []) or "none"
    handles=t.get("roomHandles") or t.get("room_handles") or []
    rooms=",".join(handles) if handles else "(none)"
    print("%s  [%s]  %s  %s  caps=%s  rooms=%s" % (t.get("id",""), state, t.get("name") or t.get("username",""), t.get("prefix",""), caps, rooms))'
            ;;
        rooms)
            local id="" clear=0 agents=()
            while [[ $# -gt 0 ]]; do
                case "$1" in
                    --agent|--agents) agents+=("${2:-}"); shift 2 ;;
                    --clear) clear=1; shift ;;
                    --help|-h) echo "Usage: k2 app token rooms <id> --agent sales  |  --clear" >&2; exit 0 ;;
                    -*) echo "Unknown flag: $1" >&2; exit 1 ;;
                    *) [[ -z "$id" ]] && id="$1"; shift ;;
                esac
            done
            [[ -z "$id" ]] && { echo "Usage: k2 app token rooms <id> --agent sales  |  --clear" >&2; exit 1; }
            if [ "$clear" = 1 ]; then
                agents=()
            fi
            local agents_nl="" a
            if [ ${#agents[@]} -gt 0 ]; then
                for a in "${agents[@]}"; do
                    agents_nl="${agents_nl}${a}"$'\n'
                done
            fi
            local body raw
            body=$(K2_SKIN_ID="$id" K2_SKIN_AGENTS="$agents_nl" python3 -c '
import json,os
b={"id": os.environ.get("K2_SKIN_ID","")}
b["rooms"]=[s.strip() for s in os.environ.get("K2_SKIN_AGENTS","").splitlines() if s.strip()]
print(json.dumps(b))')
            raw=$(cli_post_json "/cli/skin-tokens/rooms" "$body")
            printf '%s' "$raw" | _skin_die_if_err
            printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
handles=d.get("roomHandles") or d.get("room_handles") or []
print("Skin token %s rooms: %s" % (d.get("id",""), ", ".join(handles) if handles else "(none)"))'
            ;;
        revoke)
            local id="${1:-}"
            [[ -z "$id" ]] && { echo "Usage: k2 app token revoke <id>" >&2; exit 1; }
            local body raw
            body=$(K2_SKIN_ID="$id" python3 -c 'import json,os; print(json.dumps({"id": os.environ["K2_SKIN_ID"]}))')
            raw=$(cli_post_json "/cli/skin-tokens/revoke" "$body")
            printf '%s' "$raw" | _skin_die_if_err
            printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Skin token revoked." if d.get("success") else "No matching active token (unknown or already revoked).")'
            ;;
        --help|-h|"")
            echo "Usage: k2 app token <create|list|revoke|rooms>" >&2
            echo "  create --name <label> --agent sales [--caps thread:read,thread:post,files:read,files:write]" >&2
            echo "  list" >&2
            echo "  rooms <id> --agent sales" >&2
            echo "  rooms <id> --clear" >&2
            echo "  revoke <id>" >&2
            [[ -z "$sub" ]] && exit 1
            exit 0
            ;;
        *)
            echo "Usage: k2 app token <create|list|revoke|rooms>" >&2
            exit 1 ;;
    esac
}

