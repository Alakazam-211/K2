# ── k2 hostchat additions (S30 K19/K21): people, approvals, agents, on-behalf, log

# One person-confirmed POST: send once; on 403 confirm_required get a
# vault confirm for the action/secretId the daemon names, then resend with
# confirmToken. Inside a K2 terminal the agent can't confirm: exit 3.
_hostchat_confirmed_post() {
    local path="$1" payload="$2" label="$3" raw
    raw="$(_compute_send POST "$path" "$payload")"
    _compute_split "$raw"
    [ "$_CP_STATUS" = "200" ] && return 0
    local code action secret
    code="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
try: print((json.load(sys.stdin).get("error") or {}).get("code") or "")
except Exception: print("")' 2>/dev/null)"
    if [ "$_CP_STATUS" != "403" ] || [ "$code" != "confirm_required" ]; then
        _chat_fail "$_CP_STATUS" "$_CP_BODY"
    fi
    if _compute_in_session; then
        echo '{"error":"confirm_required","hint":"a person confirms this from a terminal outside K2 (Touch ID or the vault password), never an agent"}' >&2
        exit 3
    fi
    action="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(((json.load(sys.stdin).get("error") or {}).get("details") or {}).get("action") or "")')"
    secret="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(((json.load(sys.stdin).get("error") or {}).get("details") or {}).get("secretId") or "")')"
    [ -n "$action" ] || _chat_fail "$_CP_STATUS" "$_CP_BODY"
    local tok
    tok="$(_vault_confirm_token "$action" "$secret" "$label")" || exit 3
    payload="$(printf '%s' "$payload" | python3 -c 'import json,sys; v=json.load(sys.stdin); v["confirmToken"]=sys.argv[1]; print(json.dumps(v))' "$tok")"
    _chat_call POST "$path" "$payload" tcp
}

_hostchat_people() {
    local sub="${1:-list}"; [ $# -gt 0 ] && shift
    local as_json=0 guest="" pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --guest)
                case "${2:-}" in
                    on|true|yes) guest=1; shift 2 ;;
                    off|false|no) guest=0; shift 2 ;;
                    *) guest=1; shift ;;
                esac ;;
            -h|--help) _hostchat_usage; return 0 ;;
            -*) _chat_usage_err "unknown flag $1 (k2 hostchat --help)" ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local user="${pos[0]:-}"; user="${user#@}"
    case "$sub" in
        list)
            _chat_call GET /cli/sew/admin/people ""
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("people") or []
if not rows:
    print("(no people have chat here)")
for p in rows:
    print("%-24s %-8s %-6s%s" % (p.get("user"), p.get("subjectKind"), p.get("scope"), "" if p.get("enabled", True) else "  (off)"))
' ;;
        add|grant)
            [ -n "$user" ] || _chat_usage_err "k2 hostchat people add <user> [--guest]"
            _chat_call POST /cli/sew/admin/people/grant "$(_compute_json K2_CJ_user="$user" K2_CJ_guest_b="$guest")"
            _chat_print people "$as_json" ;;
        set|update)
            [ -n "$user" ] && [ -n "$guest" ] || _chat_usage_err "k2 hostchat people set <user> --guest on|off"
            _chat_call POST /cli/sew/admin/people/update "$(_compute_json K2_CJ_user="$user" K2_CJ_guest_b="$guest")"
            _chat_print people "$as_json" ;;
        remove|revoke)
            [ -n "$user" ] || _chat_usage_err "k2 hostchat people remove <user>"
            _chat_call POST /cli/sew/admin/people/revoke "$(_compute_json K2_CJ_user="$user")"
            _chat_print people "$as_json" ;;
        *) _chat_usage_err "k2 hostchat people list|add|remove|set …" ;;
    esac
}

_hostchat_approvals() {
    local sub="${1:-list}"; [ $# -gt 0 ] && shift
    case "$sub" in out_*) set -- "$sub" "$@"; sub="list" ;; esac
    local as_json=0 note="" ws="" pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --note) [ -n "${2:-}" ] || _chat_usage_err "--note \"why\""; note="$2"; shift 2 ;;
            --workspace) [ -n "${2:-}" ] || _chat_usage_err "--workspace <workspace>"; ws="$2"; shift 2 ;;
            -h|--help) _hostchat_usage; return 0 ;;
            -*) _chat_usage_err "unknown flag $1 (k2 hostchat --help)" ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    case "$sub" in
        list)
            local q=""; [ -n "$ws" ] && q="workspace=$(urlencode "$ws")"
            _chat_call GET /cli/sew/approvals "$q"
            _chat_print approvals "$as_json" ;;
        approve)
            [ ${#pos[@]} -ge 1 ] || _chat_usage_err "k2 hostchat approvals approve <out_…> [<out_…>…] [--note \"…\"]"
            if [ ${#pos[@]} -eq 1 ]; then
                _hostchat_confirmed_post /cli/sew/approvals/approve "$(_compute_json K2_CJ_id="${pos[0]}" K2_CJ_note="$note")" \
                    "Approve a queued chat post"
            else
                local ids
                ids="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${pos[@]}")"
                _hostchat_confirmed_post /cli/sew/approvals/approve-batch "$(_compute_json K2_CJ_ids_j="$ids" K2_CJ_note="$note")" \
                    "Approve ${#pos[@]} queued chat posts"
            fi
            _chat_print approve "$as_json" ;;
        deny)
            [ -n "${pos[0]:-}" ] && [ -n "$note" ] || _chat_usage_err "k2 hostchat approvals deny <out_…> --note \"why\" (the agent sees it)"
            _chat_call POST /cli/sew/approvals/deny "$(_compute_json K2_CJ_id="${pos[0]}" K2_CJ_note="$note")"
            _chat_print deny "$as_json" ;;
        *) _chat_usage_err "k2 hostchat approvals [list] | approve <out_…>… | deny <out_…> --note \"…\"" ;;
    esac
}

_hostchat_on_behalf() {
    local sub="${1:-list}"; [ $# -gt 0 ] && shift
    case "$sub" in -*) set -- "$sub" "$@"; sub="list" ;; esac
    local as_json=0 state="" limit="" note="" pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --state) [ -n "${2:-}" ] || _chat_usage_err "--state pending|approved|declined|all"; state="$2"; shift 2 ;;
            --limit) [ -n "${2:-}" ] || _chat_usage_err "--limit N"; limit="$2"; shift 2 ;;
            --note) [ -n "${2:-}" ] || _chat_usage_err "--note \"…\""; note="$2"; shift 2 ;;
            -h|--help) _hostchat_usage; return 0 ;;
            -*) _chat_usage_err "unknown flag $1 (k2 hostchat --help)" ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    case "$sub" in
        list)
            local q=""
            [ -n "$state" ] && q="state=$(urlencode "$state")"
            [ -n "$limit" ] && q="${q:+$q&}limit=$(urlencode "$limit")"
            _chat_call GET /cli/sew/on-behalf "$q"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
rows = v.get("items") or []
print("person: %s" % v.get("person"))
if not rows:
    print("(no posts made on your behalf)")
for r in rows:
    s = "%s  %s  %s  %s  by %s" % (r.get("write"), r.get("messageId") or "-", r.get("action"), r.get("state"), r.get("workspace"))
    if r.get("decidedBy"):
        s += "  reviewed by %s (%s)" % (r["decidedBy"], r.get("confirmMethod") or r.get("decidedKind"))
    if r.get("note"):
        s += "  note: " + str(r["note"])
    print(s)
' ;;
        approve)
            [ -n "${pos[0]:-}" ] || _chat_usage_err "k2 hostchat on-behalf approve <act_…|msg_…> [--note \"…\"]"
            _compute_in_session && { echo '{"error":"person_only","hint":"only the person an assistant spoke for approves it, from a terminal outside K2"}' >&2; exit 3; }
            _hostchat_confirmed_post /cli/sew/on-behalf/review "$(_compute_json K2_CJ_id="${pos[0]}" K2_CJ_decision=approve K2_CJ_note="$note")" \
                "Approve what an assistant said on your behalf"
            _chat_print on-behalf "$as_json" ;;
        decline)
            [ -n "${pos[0]:-}" ] || _chat_usage_err "k2 hostchat on-behalf decline <act_…|msg_…> [--note \"…\"]"
            _chat_call POST /cli/sew/on-behalf/review "$(_compute_json K2_CJ_id="${pos[0]}" K2_CJ_decision=decline K2_CJ_note="$note")"
            _chat_print on-behalf "$as_json" ;;
        *) _chat_usage_err "k2 hostchat on-behalf [list] | approve <act_…|msg_…> | decline <act_…|msg_…>" ;;
    esac
}

_hostchat_log() {
    local as_json=0 limit=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --limit) [ -n "${2:-}" ] || _chat_usage_err "--limit N"; limit="$2"; shift 2 ;;
            -h|--help) _hostchat_usage; return 0 ;;
            *) _chat_usage_err "k2 hostchat log [--limit N]" ;;
        esac
    done
    local q=""; [ -n "$limit" ] && q="limit=$(urlencode "$limit")"
    _chat_call GET /cli/sew/admin/log "$q"
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
rows = json.load(sys.stdin).get("entries") or []
if not rows:
    print("(no chat administration yet)")
for e in rows:
    try:
        t = time.strftime("%Y-%m-%d %H:%M", time.localtime(int(e.get("at"))))
    except Exception:
        t = str(e.get("at"))
    print("%s  %-8s %-22s %-24s %s %s %s%s" % (t, e.get("outcome"), e.get("actor"), e.get("op"), e.get("workspace") or "",
          e.get("space") or "", e.get("target") or "", ("  (" + e["reason"] + ")") if e.get("reason") else ""))
'
}

cmd_hostmail() {
    # C18: group help only for `k2 hostmail` / `k2 hostmail --help`.
    # Leaf `--help` must reach the subcommand (domain/enable/delete/…).
    if [ $# -eq 0 ]; then cmd_help_hostmail; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        status)  cmd_mail_status "$@" ;;
        enable)  cmd_hostmail_enable "$@" ;;
        disable) cmd_hostmail_disable "$@" ;;
        uninstall) cmd_hostmail_uninstall "$@" ;;
        upgrade) cmd_hostmail_upgrade "$@" ;;
        backup)  cmd_hostmail_backup "$@" ;;
        rotate-admin) cmd_hostmail_rotate_admin "$@" ;;
        password) cmd_hostmail_password "$@" ;;
        app-password) cmd_hostmail_app_password "$@" ;;
        person) cmd_hostmail_person "$@" ;;
        domain)  cmd_mail_domain "$@" ;;
        create)  cmd_mail_create "$@" ;;
        delete)  cmd_hostmail_delete "$@" ;;
        doctor)  cmd_mail_doctor "$@" ;;
        config)  cmd_mail_config "$@" ;;
        approvals) cmd_mail_approvals "$@" ;;
        import)  cmd_hostmail_import "$@" ;;
        quota)   cmd_hostmail_quota "$@" ;;
        catchall) cmd_hostmail_catchall "$@" ;;
        alias)   cmd_hostmail_alias "$@" ;;
        forward) cmd_hostmail_forward "$@" ;;
        ooo)     cmd_hostmail_ooo "$@" ;;
        footer)  cmd_hostmail_footer "$@" ;;
        cert)    cmd_hostmail_cert "$@" ;;
        list)    cmd_hostmail_list "$@" ;;
        spam)    cmd_hostmail_spam "$@" ;;
        queue)   cmd_hostmail_queue "$@" ;;
        acl)     cmd_hostmail_acl "$@" ;;
        autoconfig) cmd_hostmail_autoconfig "$@" ;;
        dkim)    cmd_hostmail_dkim "$@" ;;
        dmarc)   cmd_hostmail_dmarc "$@" ;;
        ptr)     cmd_hostmail_ptr "$@" ;;
        outbound) cmd_hostmail_outbound "$@" ;;
        bans)    cmd_hostmail_bans "$@" ;;
        allowlist) cmd_hostmail_allowlist "$@" ;;
        calendar) cmd_hostmail_calendar "$@" ;;
        group)   cmd_hostmail_group "$@" ;;
        profile) cmd_hostmail_profile "$@" ;;
        audit-sieve) cmd_hostmail_audit_sieve "$@" ;;
        --help|-h) cmd_help_hostmail; exit 0 ;;
        *)
            _mail_err "usage" "unknown hostmail command '$sub' — usage: k2 hostmail <status|enable|disable|uninstall|upgrade|rotate-admin|password|app-password|person|domain|create|delete|doctor|backup|config|approvals|import|quota|catchall|alias|forward|ooo|footer|cert|list|spam|queue|acl|autoconfig|dkim|dmarc|ptr|outbound|bans|allowlist|calendar|group|profile|audit-sieve>"
            exit 2 ;;
    esac
}

cmd_hostmail_enable() {
    if _wants_help "$@"; then cmd_help_hostmail_enable; exit 0; fi
    local as_json=false hostname=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --hostname)
                [ $# -ge 2 ] || { _mail_err "usage" "--hostname requires a value — usage: k2 hostmail enable --hostname <name> [--json]"; exit 2; }
                hostname="$2"; shift 2 ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail enable --hostname <name> [--json]"; exit 2 ;;
        esac
    done
    [ -n "$hostname" ] || { _mail_err "usage" "k2 hostmail enable --hostname <name> (e.g. mail.acme.dev)"; exit 2; }
    local M_VERB="server_enable" M_JSON="$as_json" M_HOSTNAME="$hostname"
    _mail_py
}

cmd_hostmail_disable() {
    if _wants_help "$@"; then cmd_help_hostmail_disable; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail disable [--json]"; exit 2 ;;
        esac
    done
    local M_VERB="server_disable" M_JSON="$as_json"
    _mail_py
}

cmd_hostmail_uninstall() {
    if _wants_help "$@"; then cmd_help_hostmail_uninstall; exit 0; fi
    local as_json=false purge=false hostname=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --purge) purge=true; shift ;;
            --confirm-hostname)
                [ $# -ge 2 ] || { _mail_err "usage" "--confirm-hostname requires a value — usage: k2 hostmail uninstall --purge --confirm-hostname <host>"; exit 2; }
                hostname="$2"; shift 2 ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail uninstall --purge --confirm-hostname <host> [--json]"; exit 2 ;;
        esac
    done
    if [ "$purge" != true ] || [ -z "$hostname" ]; then
        _mail_err "usage" "k2 hostmail uninstall --purge --confirm-hostname <host> (both flags required; nothing was sent)"
        exit 2
    fi
    local M_VERB="server_uninstall" M_JSON="$as_json" M_HOSTNAME="$hostname"
    _mail_py
}

cmd_hostmail_upgrade() {
    if _wants_help "$@"; then cmd_help_hostmail_upgrade; exit 0; fi
    local as_json=false dry_run=false ack=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --dry-run) dry_run=true; shift ;;
            --acknowledge-failed) ack=true; shift ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail upgrade [--dry-run] [--json] | k2 hostmail upgrade --acknowledge-failed [--json]"; exit 2 ;;
        esac
    done
    if [ "$dry_run" = true ] && [ "$ack" = true ]; then
        _mail_err "usage" "--dry-run and --acknowledge-failed are separate calls (nothing was sent)"
        exit 2
    fi
    local M_VERB="server_upgrade" M_JSON="$as_json" M_DRY_RUN="$dry_run" M_ACK_FAILED="$ack"
    _mail_py
}

cmd_hostmail_rotate_admin() {
    if _wants_help "$@"; then cmd_help_hostmail_rotate_admin; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail rotate-admin [--json]"; exit 2 ;;
        esac
    done
    local M_VERB="rotate_admin" M_JSON="$as_json"
    _mail_py
}

cmd_hostmail_password() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_password; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        rotate) cmd_hostmail_password_rotate "$@" ;;
        keep) cmd_hostmail_password_keep "$@" ;;
        --help|-h) cmd_help_hostmail_password; exit 0 ;;
        *)
            _mail_err "usage" "unknown password command '$sub' — usage: k2 hostmail password <rotate|keep> <addr> [--json]"
            exit 2 ;;
    esac
}

# Owner only: mark a mailbox password as reviewed (you know who has it),
# so `k2 hostmail doctor` stops flagging it. Never shows a secret.
cmd_hostmail_password_keep() {
    if _wants_help "$@"; then cmd_help_hostmail_password_keep; exit 0; fi
    local address="" as_json=false all_existing=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --all-existing) all_existing=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail password keep <addr>|--all-existing [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one <addr> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ "$all_existing" = true ]; then
        if [ -n "$address" ]; then
            _mail_err "usage" "--all-existing covers every mailbox — drop '$address'"
            exit 2
        fi
        local M_VERB="credentials_keep_all_mailbox" M_JSON="$as_json"
        _mail_py
        return
    fi
    if [ -z "$address" ]; then
        _mail_err "usage" "k2 hostmail password keep <addr>|--all-existing (hosted address)"
        exit 2
    fi
    local M_VERB="credentials_keep" M_JSON="$as_json" M_ADDRESS="$address" M_ID=""
    _mail_py
}

cmd_hostmail_password_rotate() {
    if _wants_help "$@"; then cmd_help_hostmail_password_rotate; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail password rotate <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one <addr> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$address" ]; then
        _mail_err "usage" "k2 hostmail password rotate <addr> (hosted address)"
        exit 2
    fi
    local M_VERB="password_rotate" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}

cmd_hostmail_app_password() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_app_password; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        add) cmd_hostmail_app_password_add "$@" ;;
        list) cmd_hostmail_app_password_list "$@" ;;
        revoke) cmd_hostmail_app_password_revoke "$@" ;;
        keep) cmd_hostmail_app_password_keep "$@" ;;
        --help|-h) cmd_help_hostmail_app_password; exit 0 ;;
        *)
            _mail_err "usage" "unknown app-password command '$sub' — usage: k2 hostmail app-password <add|list|revoke|keep>"
            exit 2 ;;
    esac
}

cmd_hostmail_app_password_add() {
    if _wants_help "$@"; then cmd_help_hostmail_app_password_add; exit 0; fi
    local address="" label="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --label)
                [ $# -ge 2 ] || { _mail_err "usage" "--label requires a value"; exit 2; }
                label="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail app-password add <addr> [--label <s>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one <addr> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail app-password add <addr> [--label <s>] [--json]"; exit 2; }
    local M_VERB="app_password_add" M_JSON="$as_json" M_ADDRESS="$address" M_LABEL="$label"
    _mail_py
}

cmd_hostmail_app_password_list() {
    if _wants_help "$@"; then cmd_help_hostmail_app_password_list; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail app-password list <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one <addr> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail app-password list <addr> [--json]"; exit 2; }
    local M_VERB="app_password_list" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}

cmd_hostmail_app_password_revoke() {
    if _wants_help "$@"; then cmd_help_hostmail_app_password_revoke; exit 0; fi
    local address="" id="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail app-password revoke <addr> <id> [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"
                elif [ -z "$id" ]; then id="$1"
                else
                    _mail_err "usage" "unexpected extra argument '$1' — <addr> <id> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$address" ] || [ -z "$id" ]; then
        _mail_err "usage" "k2 hostmail app-password revoke <addr> <id> [--json]"
        exit 2
    fi
    local M_VERB="app_password_revoke" M_JSON="$as_json" M_ADDRESS="$address" M_ID="$id"
    _mail_py
}

# Owner only: mark an older app password as one you handed out, so
# `k2 hostmail doctor` stops flagging it. Never creates or shows a secret.
cmd_hostmail_app_password_keep() {
    if _wants_help "$@"; then cmd_help_hostmail_app_password_keep; exit 0; fi
    local address="" id="" as_json=false all_existing=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --all-existing) all_existing=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail app-password keep <addr> <id>|--all-existing [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"
                elif [ -z "$id" ]; then id="$1"
                else
                    _mail_err "usage" "unexpected extra argument '$1' — <addr> <id> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ "$all_existing" = true ]; then
        if [ -n "$address" ]; then
            _mail_err "usage" "--all-existing covers every mailbox — drop '$address'"
            exit 2
        fi
        local M_VERB="credentials_keep_all_app_password" M_JSON="$as_json"
        _mail_py
        return
    fi
    if [ -z "$address" ] || [ -z "$id" ]; then
        _mail_err "usage" "k2 hostmail app-password keep <addr> <id> [--json]"
        exit 2
    fi
    local M_VERB="credentials_keep" M_JSON="$as_json" M_ADDRESS="$address" M_ID="$id"
    _mail_py
}

# Mark a hosted mailbox as a person's (on) or an agent's (off). Owner or
# an IT agent (mail-manage); an IT agent can't turn it on for its own
# workspace's mailbox.
cmd_hostmail_person() {
    # 0.46.7: `person link <addr> <login|owner|--unlink>` (which box login owns
    # a person mailbox; outside forwards).
    if [ "${1:-}" = link ]; then shift; cmd_hostmail_person_link "$@"; return; fi
    if [ $# -eq 0 ] || _wants_help "$@"; then cmd_help_hostmail_person; exit 0; fi
    local address="" state="" as_json=false all=false workspace=""
    local usage="k2 hostmail person <addr> on|off [--json] | k2 hostmail person --all --workspace <ws> on|off [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --all) all=true; shift ;;
            --workspace)
                [ $# -ge 2 ] || { _mail_err "usage" "--workspace requires a value"; exit 2; }
                workspace="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: $usage"
                exit 2 ;;
            *)
                # With --all the only positional is on|off.
                if [ "$all" = true ] && [ -z "$state" ] && { [ "$1" = on ] || [ "$1" = off ]; }; then
                    state="$1"
                elif [ -z "$address" ] && [ "$all" = false ]; then address="$1"
                elif [ -z "$state" ]; then state="$1"
                else
                    _mail_err "usage" "unexpected extra argument '$1' — usage: $usage"
                    exit 2
                fi
                shift ;;
        esac
    done
    case "$state" in
        on|off) ;;
        *)
            _mail_err "usage" "$usage"
            exit 2 ;;
    esac
    if [ "$all" = true ]; then
        if [ -n "$address" ] || [ -z "$workspace" ]; then
            _mail_err "usage" "--all needs --workspace <ws> and no <addr> — usage: $usage"
            exit 2
        fi
    elif [ -z "$address" ] || [ -n "$workspace" ]; then
        _mail_err "usage" "$usage"
        exit 2
    fi
    local M_VERB="person_set" M_JSON="$as_json" M_ADDRESS="$address" M_PERSON="$state" \
          M_ALL="$all" M_WORKSPACE="$workspace"
    _mail_py
}

cmd_hostmail_cert() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_cert; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        renew) cmd_hostmail_cert_renew "$@" ;;
        names) cmd_hostmail_cert_names "$@" ;;
        owner) cmd_hostmail_cert_owner "$@" ;;
        --help|-h) cmd_help_hostmail_cert; exit 0 ;;
        *)
            _mail_err "usage" "unknown cert command '$sub' — usage: k2 hostmail cert <renew|names|owner>"
            exit 2 ;;
    esac
}

# 0.45.1: the outbound settings K2 manages. show = GET /cli/mail/outbound;
# `dane on` = POST {dane: on} (turn DANE back on after a hand fix or after
# the 0.16.20 upgrade; refused below 0.16.20).
cmd_hostmail_outbound() {
    if _wants_help "$@"; then cmd_help_hostmail_outbound; exit 0; fi
    local action="show" as_json=false
    local usage="usage: k2 hostmail outbound [show] [--json] | k2 hostmail outbound dane on [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            show) shift ;;
            dane)
                shift
                [ "${1:-}" = "on" ] || { _mail_err "usage" "$usage"; exit 2; }
                action="dane-on"; shift ;;
            *) _mail_err "usage" "unexpected argument '$1' — $usage"; exit 2 ;;
        esac
    done
    local M_VERB="outbound" M_JSON="$as_json" M_OUTBOUND_ACTION="$action"
    _mail_py
}

# Who owns the mail certificate. GET /cli/mail/cert/owner; --stalwart-acme
# = POST {action: stalwart-acme} (hand it back to Stalwart's ACME, mail
# host only; refused while K2 owns it).
cmd_hostmail_cert_owner() {
    if _wants_help "$@"; then cmd_help_hostmail_cert_owner; exit 0; fi
    local action="show" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --stalwart-acme) action="stalwart-acme"; shift ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail cert owner [--stalwart-acme] [--json]"; exit 2 ;;
        esac
    done
    local M_VERB="cert_owner" M_JSON="$as_json" M_CERT_ACTION="$action"
    _mail_py
}

# CAL44: one non-default certificate per extra mail-family name
# (autoconfig/autodiscover/mta-sts/ua-auto-config). list = GET,
# issue|renew = POST /cli/mail/cert/names.
cmd_hostmail_cert_names() {
    if _wants_help "$@"; then cmd_help_hostmail_cert_names; exit 0; fi
    local action="list" name="" as_json=false
    case "${1:-}" in
        list|issue|renew) action="$1"; shift ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --name)
                [ "$action" != "list" ] || { _mail_err "usage" "--name goes with issue|renew — usage: k2 hostmail cert names [list|issue|renew] [--name <host>] [--json]"; exit 2; }
                [ $# -ge 2 ] || { _mail_err "usage" "--name requires a host (e.g. autoconfig.example.com)"; exit 2; }
                name="$2"; shift 2 ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail cert names [list|issue|renew] [--name <host>] [--json]"; exit 2 ;;
        esac
    done
    local M_VERB="cert_names" M_JSON="$as_json" M_CERT_ACTION="$action" M_NAME="$name"
    _mail_py
}

cmd_hostmail_cert_renew() {
    if _wants_help "$@"; then cmd_help_hostmail_cert_renew; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail cert renew [--json]"; exit 2 ;;
        esac
    done
    local M_VERB="cert_renew" M_JSON="$as_json"
    _mail_py
}

# S8 B1 hosted-mail backup: the per-box choice. status = GET
# /cli/mail/backup; plan = GET /cli/mail/backup/plan (dry-run, changes
# nothing); set = POST /cli/mail/backup/set. B1 runs no backups.
cmd_hostmail_backup() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_backup; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        status) cmd_hostmail_backup_status "$@" ;;
        plan)   cmd_hostmail_backup_plan "$@" ;;
        set)    cmd_hostmail_backup_set "$@" ;;
        --help|-h) cmd_help_hostmail_backup; exit 0 ;;
        *)
            _mail_err "usage" "unknown backup command '$sub' — usage: k2 hostmail backup <status|plan|set>"
            exit 2 ;;
    esac
}

_hostmail_backup_count() {
    # $1 flag name, $2 value: a whole number or exit 2 before any request.
    case "$2" in
        ''|*[!0-9]*) _mail_err "usage" "$1 must be a whole number, got '$2'"; exit 2 ;;
    esac
}

cmd_hostmail_backup_status() {
    if _wants_help "$@"; then cmd_help_hostmail_backup_status; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail backup status [--json]"; exit 2 ;;
        esac
    done
    local M_VERB="backup_status" M_JSON="$as_json"
    _mail_py
}

cmd_hostmail_backup_plan() {
    if _wants_help "$@"; then cmd_help_hostmail_backup_plan; exit 0; fi
    local as_json=false mbps="" kd="" kw="" gb=""
    local usage="usage: k2 hostmail backup plan [--bandwidth-mbps N] [--keep-daily N] [--keep-weekly N] [--max-local-gb G] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --bandwidth-mbps|--keep-daily|--keep-weekly|--max-local-gb)
                [ $# -ge 2 ] || { _mail_err "usage" "$1 requires a value — $usage"; exit 2; }
                case "$1" in
                    --bandwidth-mbps) mbps="$2" ;;
                    --keep-daily) _hostmail_backup_count "$1" "$2"; kd="$2" ;;
                    --keep-weekly) _hostmail_backup_count "$1" "$2"; kw="$2" ;;
                    --max-local-gb) gb="$2" ;;
                esac
                shift 2 ;;
            *) _mail_err "usage" "unexpected argument '$1' — $usage"; exit 2 ;;
        esac
    done
    local M_VERB="backup_plan" M_JSON="$as_json" M_BK_BANDWIDTH="$mbps" \
          M_BK_KEEP_DAILY="$kd" M_BK_KEEP_WEEKLY="$kw" M_BK_MAX_LOCAL_GB="$gb"
    _mail_py
}

cmd_hostmail_backup_set() {
    if _wants_help "$@"; then cmd_help_hostmail_backup_set; exit 0; fi
    local as_json=false mode="" reason="" kd="" kw="" okd="" okw="" okm="" window="" gb="" confirm=""
    local usage="usage: k2 hostmail backup set --mode local|offsite-only|both|none [--reason <text>] [--keep-daily N --keep-weekly N] [--offsite-keep-daily N --offsite-keep-weekly N --offsite-keep-monthly N] [--window HH:MM-HH:MM] [--max-local-gb G] --confirm-plan <hash> [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --mode|--reason|--keep-daily|--keep-weekly|--offsite-keep-daily|--offsite-keep-weekly|--offsite-keep-monthly|--window|--max-local-gb|--confirm-plan)
                [ $# -ge 2 ] || { _mail_err "usage" "$1 requires a value — $usage"; exit 2; }
                case "$1" in
                    --mode) mode="$2" ;;
                    --reason) reason="$2" ;;
                    --keep-daily) _hostmail_backup_count "$1" "$2"; kd="$2" ;;
                    --keep-weekly) _hostmail_backup_count "$1" "$2"; kw="$2" ;;
                    --offsite-keep-daily) _hostmail_backup_count "$1" "$2"; okd="$2" ;;
                    --offsite-keep-weekly) _hostmail_backup_count "$1" "$2"; okw="$2" ;;
                    --offsite-keep-monthly) _hostmail_backup_count "$1" "$2"; okm="$2" ;;
                    --window) window="$2" ;;
                    --max-local-gb) gb="$2" ;;
                    --confirm-plan) confirm="$2" ;;
                esac
                shift 2 ;;
            *) _mail_err "usage" "unexpected argument '$1' — $usage"; exit 2 ;;
        esac
    done
    case "$mode" in
        local|offsite-only|both|none) ;;
        '') _mail_err "usage" "--mode is required — $usage"; exit 2 ;;
        *) _mail_err "usage" "--mode must be local, offsite-only, both or none (got '$mode')"; exit 2 ;;
    esac
    if [ "$mode" = "none" ] && [ -z "$reason" ]; then
        _mail_err "usage" "--mode none needs --reason \"<why>\" (e.g. \"provider snapshots daily\")"
        exit 2
    fi
    if [ -z "$confirm" ]; then
        _mail_err "usage" "--confirm-plan <hash> is required — run k2 hostmail backup plan (same retention flags) and pass its planHash"
        exit 2
    fi
    local M_VERB="backup_set" M_JSON="$as_json" M_BK_MODE="$mode" M_REASON="$reason" \
          M_BK_KEEP_DAILY="$kd" M_BK_KEEP_WEEKLY="$kw" M_BK_OFF_DAILY="$okd" \
          M_BK_OFF_WEEKLY="$okw" M_BK_OFF_MONTHLY="$okm" M_BK_WINDOW="$window" \
          M_BK_MAX_LOCAL_GB="$gb" M_BK_CONFIRM="$confirm"
    _mail_py
}

cmd_hostmail_import() {
    if _wants_help "$@"; then cmd_help_hostmail_import; exit 0; fi
    local address="" maildir="" imap_host="" imap_user="" imap_pass="" imap_port="" as_json=false skip_inbox=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            --maildir)
                [ $# -ge 2 ] || { _mail_err "usage" "--maildir requires a path"; exit 2; }
                maildir="$2"; shift 2 ;;
            --imapsync)
                [ $# -ge 2 ] || { _mail_err "usage" "--imapsync requires from=<host>"; exit 2; }
                imap_host="${2#from=}"; shift 2 ;;
            --user)
                [ $# -ge 2 ] || { _mail_err "usage" "--user requires a value"; exit 2; }
                imap_user="$2"; shift 2 ;;
            --pass-stdin)
                imap_pass="$(cat)"; shift
                if [ -z "$imap_pass" ]; then
                    _mail_err "usage" "--pass-stdin: no password on stdin"
                    exit 2
                fi ;;
            --port)
                [ $# -ge 2 ] || { _mail_err "usage" "--port requires a port number"; exit 2; }
                imap_port="$2"; shift 2 ;;
            --skip-inbox) skip_inbox=true; shift ;;
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail import --address <a@domain> (--maildir <path> | --imapsync from=<host> --user <u> --pass-stdin) [--skip-inbox] [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "import requires --address <a@domain>"; exit 2; }
    if [ -z "$maildir" ] && [ -z "$imap_host" ]; then
        _mail_err "usage" "import requires --maildir <path> and/or --imapsync from=<host>"
        exit 2
    fi
    local M_VERB="import" M_JSON="$as_json" M_ADDRESS="$address" \
          M_MAILDIR="$maildir" M_IMAPSYNC_HOST="$imap_host" \
          M_IMAPSYNC_USER="$imap_user" M_IMAPSYNC_PASS="$imap_pass" \
          M_IMAPSYNC_PORT="$imap_port" M_SKIP_INBOX="$skip_inbox"
    _mail_py
}

cmd_hostmail_quota() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_quota; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        get) cmd_hostmail_quota_get "$@" ;;
        set) cmd_hostmail_quota_set "$@" ;;
        --help|-h) cmd_help_hostmail_quota; exit 0 ;;
        *)
            _mail_err "usage" "unknown quota command '$sub' — usage: k2 hostmail quota <get|set>"
            exit 2 ;;
    esac
}

cmd_hostmail_quota_get() {
    if _wants_help "$@"; then cmd_help_hostmail_quota_get; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail quota get <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one address only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail quota get <addr>"; exit 2; }
    local M_VERB="quota_get" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}

cmd_hostmail_quota_set() {
    if _wants_help "$@"; then cmd_help_hostmail_quota_set; exit 0; fi
    local address="" bytes="" gb="" messages="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --bytes)
                [ $# -ge 2 ] || { _mail_err "usage" "--bytes requires a number (0 = unlimited)"; exit 2; }
                bytes="$2"; shift 2 ;;
            --gb)
                [ $# -ge 2 ] || { _mail_err "usage" "--gb requires a number (0 = unlimited)"; exit 2; }
                gb="$2"; shift 2 ;;
            --messages)
                [ $# -ge 2 ] || { _mail_err "usage" "--messages requires a number (0 = unlimited)"; exit 2; }
                messages="$2"; shift 2 ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail quota set <addr> --bytes N|--gb N [--messages N]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one address only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail quota set <addr> --bytes N|--gb N [--messages N]"; exit 2; }
    if [ -n "$bytes" ] && [ -n "$gb" ]; then
        _mail_err "usage" "give either --bytes or --gb, not both"
        exit 2
    fi
    if [ -z "$bytes" ] && [ -z "$gb" ] && [ -z "$messages" ]; then
        _mail_err "usage" "give --bytes N or --gb N and/or --messages N (0 = unlimited)"
        exit 2
    fi
    if [ -n "$bytes" ] && ! _mail_is_uint "$bytes"; then
        _mail_err "usage" "--bytes must be a non-negative number (0 = unlimited)"
        exit 2
    fi
    if [ -n "$gb" ] && ! _mail_is_uint "$gb"; then
        _mail_err "usage" "--gb must be a non-negative number (0 = unlimited)"
        exit 2
    fi
    if [ -n "$messages" ] && ! _mail_is_uint "$messages"; then
        _mail_err "usage" "--messages must be a non-negative number (0 = unlimited)"
        exit 2
    fi
    local M_VERB="quota_set" M_JSON="$as_json" M_ADDRESS="$address" \
          M_QUOTA_BYTES="$bytes" M_QUOTA_GB="$gb" M_QUOTA_MESSAGES="$messages"
    _mail_py
}

cmd_hostmail_catchall() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_catchall; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        set) cmd_hostmail_catchall_set "$@" ;;
        unset) cmd_hostmail_catchall_unset "$@" ;;
        show) cmd_hostmail_catchall_show "$@" ;;
        --help|-h) cmd_help_hostmail_catchall; exit 0 ;;
        *)
            _mail_err "usage" "unknown catchall command '$sub' — usage: k2 hostmail catchall <set|unset|show>"
            exit 2 ;;
    esac
}


cmd_hostmail_catchall_set() {
    if _wants_help "$@"; then cmd_help_hostmail_catchall_set; exit 0; fi
    local domain="" address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a minted mailbox"; exit 2; }
                address="$2"; shift 2 ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail catchall set <domain> --address <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] || { _mail_err "usage" "k2 hostmail catchall set <domain> --address <addr>"; exit 2; }
    [ -n "$address" ] || { _mail_err "usage" "catchall set requires --address <minted mailbox on that domain>"; exit 2; }
    local M_VERB="catchall_set" M_JSON="$as_json" M_DOMAIN="$domain" M_ADDRESS="$address"
    _mail_py
}


cmd_hostmail_catchall_unset() {
    if _wants_help "$@"; then cmd_help_hostmail_catchall_unset; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail catchall unset <domain> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] || { _mail_err "usage" "k2 hostmail catchall unset <domain>"; exit 2; }
    local M_VERB="catchall_set" M_JSON="$as_json" M_DOMAIN="$domain" M_ADDRESS=""
    _mail_py
}


cmd_hostmail_catchall_show() {
    if _wants_help "$@"; then cmd_help_hostmail_catchall_show; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail catchall show <domain> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] || { _mail_err "usage" "k2 hostmail catchall show <domain>"; exit 2; }
    local M_VERB="catchall_get" M_JSON="$as_json" M_DOMAIN="$domain"
    _mail_py
}


cmd_hostmail_alias() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_alias; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        add) cmd_hostmail_alias_add "$@" ;;
        remove) cmd_hostmail_alias_remove "$@" ;;
        list) cmd_hostmail_alias_list "$@" ;;
        --help|-h) cmd_help_hostmail_alias; exit 0 ;;
        *)
            _mail_err "usage" "unknown alias command '$sub' — usage: k2 hostmail alias <add|remove|list>"
            exit 2 ;;
    esac
}


cmd_hostmail_alias_add() {
    if _wants_help "$@"; then cmd_help_hostmail_alias_add; exit 0; fi
    local mailbox="" alias="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail alias add <mailbox> <alias> [--json]"
                exit 2 ;;
            *)
                if [ -z "$mailbox" ]; then mailbox="$1"
                elif [ -z "$alias" ]; then alias="$1"
                else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$mailbox" ] && [ -n "$alias" ] || {
        _mail_err "usage" "k2 hostmail alias add <mailbox> <alias> — extra address on an existing mailbox (not a mint; k2 hostmail create mints)"
        exit 2
    }
    local M_VERB="alias_add" M_JSON="$as_json" M_MAILBOX="$mailbox" M_ALIAS="$alias"
    _mail_py
}


cmd_hostmail_alias_remove() {
    if _wants_help "$@"; then cmd_help_hostmail_alias_remove; exit 0; fi
    local mailbox="" alias="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail alias remove <mailbox> <alias> [--json]"
                exit 2 ;;
            *)
                if [ -z "$mailbox" ]; then mailbox="$1"
                elif [ -z "$alias" ]; then alias="$1"
                else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$mailbox" ] && [ -n "$alias" ] || {
        _mail_err "usage" "k2 hostmail alias remove <mailbox> <alias>"
        exit 2
    }
    local M_VERB="alias_remove" M_JSON="$as_json" M_MAILBOX="$mailbox" M_ALIAS="$alias"
    _mail_py
}


cmd_hostmail_alias_list() {
    if _wants_help "$@"; then cmd_help_hostmail_alias_list; exit 0; fi
    local mailbox="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail alias list <mailbox> [--json]"
                exit 2 ;;
            *)
                if [ -z "$mailbox" ]; then mailbox="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$mailbox" ] || { _mail_err "usage" "k2 hostmail alias list <mailbox>"; exit 2; }
    local M_VERB="alias_list" M_JSON="$as_json" M_MAILBOX="$mailbox"
    _mail_py
}


cmd_hostmail_forward() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_forward; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        set) cmd_hostmail_forward_set set "$@" ;;
        add) cmd_hostmail_forward_set add "$@" ;;
        rm) cmd_hostmail_forward_set rm "$@" ;;
        unset) cmd_hostmail_forward_simple unset "$@" ;;
        show) cmd_hostmail_forward_simple show "$@" ;;
        pause) cmd_hostmail_forward_simple pause "$@" ;;
        resume) cmd_hostmail_forward_simple resume "$@" ;;
        list) cmd_hostmail_forward_list "$@" ;;
        --help|-h) cmd_help_hostmail_forward; exit 0 ;;
        *)
            _mail_err "usage" "unknown forward command '$sub' — usage: k2 hostmail forward <set|add|rm|show|unset|pause|resume|list>"
            exit 2 ;;
    esac
}

# Shared by `k2 hostmail forward set|add|rm` (admin routes, <addr> required)
# and `k2 mail forward set|add|rm` (person routes, --address optional). keep
# rides only when --keep / --no-keep is given (M_KEEP_ARG). --to repeats
# (_fw_to: one address a line).
_forward_set_parse() {
    local family="$1" op="$2"; shift 2
    local usage what
    if [ "$op" = rm ]; then what="--to <dest> [--to <dest>…]"; else what="--to <dest> [--to <dest>…] [--keep|--no-keep] [--restart-stopped]"; fi
    if [ "$family" = hostmail ]; then
        usage="k2 hostmail forward $op <addr> $what [--json]"
    else
        usage="k2 mail forward $op $what [--address <addr>] [--json]"
    fi
    _fw_address="" _fw_to="" _fw_keep="" _fw_json=false _fw_restart=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _fw_json=true; shift ;;
            --keep|--no-keep|--restart-stopped)
                [ "$op" != rm ] || { _mail_err "usage" "unknown flag '$1' — usage: $usage"; exit 2; }
                case "$1" in
                    --keep)
                        [ "$_fw_keep" != false ] || { _mail_err "usage" "give --keep or --no-keep, not both — usage: $usage"; exit 2; }
                        _fw_keep=true ;;
                    --no-keep)
                        [ "$_fw_keep" != true ] || { _mail_err "usage" "give --keep or --no-keep, not both — usage: $usage"; exit 2; }
                        _fw_keep=false ;;
                    *) _fw_restart=true ;;
                esac
                shift ;;
            --to)
                [ $# -ge 2 ] || { _mail_err "usage" "--to requires an address — usage: $usage"; exit 2; }
                _fw_to="${_fw_to:+$_fw_to
}$2"; shift 2 ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                _fw_address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: $usage"
                exit 2 ;;
            *)
                if [ "$family" = hostmail ] && [ -z "$_fw_address" ]; then _fw_address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — usage: $usage"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ "$family" = hostmail ] && [ -z "$_fw_address" ]; then _mail_err "usage" "$usage"; exit 2; fi
    [ -n "$_fw_to" ] || { _mail_err "usage" "forward $op requires --to <dest> (a mailbox here, or an outside address) — usage: $usage"; exit 2; }
}

cmd_hostmail_forward_set() {
    local op="$1"; shift
    if _wants_help "$@"; then "cmd_help_hostmail_forward_$op"; exit 0; fi
    local _fw_address _fw_to _fw_keep _fw_json _fw_restart
    _forward_set_parse hostmail "$op" "$@"
    local M_VERB="forward_$op" M_JSON="$_fw_json" M_ADDRESS="$_fw_address" M_TO="$_fw_to" M_KEEP_ARG="$_fw_keep" M_RESTART="$_fw_restart"
    _mail_py
}

# show | unset | pause | resume <addr> [--json] (admin routes).
cmd_hostmail_forward_simple() {
    local op="$1"; shift
    if _wants_help "$@"; then "cmd_help_hostmail_forward_$op"; exit 0; fi
    local address="" as_json=false to=""
    local usage="k2 hostmail forward $op <addr> [--json]"
    case "$op" in pause|resume) usage="k2 hostmail forward $op <addr> [--to <dest>] [--json]" ;; esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            --to)
                case "$op" in pause|resume) ;; *) _mail_err "usage" "unknown flag '--to' — usage: $usage"; exit 2 ;; esac
                [ $# -ge 2 ] || { _mail_err "usage" "--to requires an address — usage: $usage"; exit 2; }
                to="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: $usage"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — usage: $usage"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "$usage"; exit 2; }
    local verb="forward_$op"
    [ "$op" = show ] && verb="forward_get"
    local M_VERB="$verb" M_JSON="$as_json" M_ADDRESS="$address" M_TO="$to"
    _mail_py
}

cmd_hostmail_forward_list() {
    if _wants_help "$@"; then cmd_help_hostmail_forward_list; exit 0; fi
    local state="" kind="" as_json=false
    local usage="k2 hostmail forward list [--paused|--active|--pending] [--external] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --paused) state=paused; shift ;;
            --active) state=active; shift ;;
            --pending) state=pending_verify; shift ;;
            --external) kind=external; shift ;;
            *)
                _mail_err "usage" "unknown argument '$1' — usage: $usage"
                exit 2 ;;
        esac
    done
    local M_VERB="forward_list" M_JSON="$as_json" M_STATE="$state" M_KIND="$kind"
    _mail_py
}

# k2 mail forward — a person's OWN mailbox (person routes,
# /cli/mail/self/forward*). The person comes from the token: a box login,
# or the owner. Agent tokens get person_only (exit 3) and use
# `k2 hostmail forward` instead.
cmd_mail_forward() {
    if [ $# -eq 0 ]; then cmd_help_mail_forward; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        set|add|rm)
            if _wants_help "$@"; then cmd_help_mail_forward; exit 0; fi
            local _fw_address _fw_to _fw_keep _fw_json _fw_restart
            _forward_set_parse mail "$sub" "$@"
            local M_VERB="self_forward_$sub" M_JSON="$_fw_json" M_ADDRESS="$_fw_address" M_TO="$_fw_to" M_KEEP_ARG="$_fw_keep" M_RESTART="$_fw_restart"
            _mail_py ;;
        show|unset|pause|resume)
            if _wants_help "$@"; then cmd_help_mail_forward; exit 0; fi
            local address="" as_json=false to=""
            local usage="k2 mail forward $sub [--address <addr>] [--json]"
            case "$sub" in pause|resume) usage="k2 mail forward $sub [--to <dest>] [--address <addr>] [--json]" ;; esac
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --address)
                        [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                        address="$2"; shift 2 ;;
                    --to)
                        case "$sub" in pause|resume) ;; *) _mail_err "usage" "unknown argument '--to' — usage: $usage"; exit 2 ;; esac
                        [ $# -ge 2 ] || { _mail_err "usage" "--to requires an address — usage: $usage"; exit 2; }
                        to="$2"; shift 2 ;;
                    *)
                        _mail_err "usage" "unknown argument '$1' — usage: $usage"
                        exit 2 ;;
                esac
            done
            local verb="self_forward_$sub"
            [ "$sub" = show ] && verb="self_forward_get"
            local M_VERB="$verb" M_JSON="$as_json" M_ADDRESS="$address" M_TO="$to"
            _mail_py ;;
        --help|-h) cmd_help_mail_forward; exit 0 ;;
        *)
            _mail_err "usage" "unknown forward command '$sub' — usage: k2 mail forward <set|add|rm|show|unset|pause|resume>"
            exit 2 ;;
    esac
}

# k2 hostmail person link <addr> <login|owner> | <addr> --unlink
cmd_hostmail_person_link() {
    if _wants_help "$@"; then cmd_help_hostmail_person_link; exit 0; fi
    local address="" person="" unlink=false as_json=false
    local usage="k2 hostmail person link <addr> <login|owner> [--json] | k2 hostmail person link <addr> --unlink [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --unlink) unlink=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: $usage"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"
                elif [ -z "$person" ]; then person="$1"
                else
                    _mail_err "usage" "unexpected extra argument '$1' — usage: $usage"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "$usage"; exit 2; }
    if [ "$unlink" = true ] && [ -n "$person" ]; then
        _mail_err "usage" "give a login or --unlink, not both — usage: $usage"; exit 2
    fi
    if [ "$unlink" = false ] && [ -z "$person" ]; then
        _mail_err "usage" "name the login (k2 users list) or owner, or --unlink — usage: $usage"; exit 2
    fi
    local M_VERB="person_link" M_JSON="$as_json" M_ADDRESS="$address" M_PERSON="$person"
    _mail_py
}

