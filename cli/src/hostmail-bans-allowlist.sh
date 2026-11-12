# ── Calendars S4: groups + calendar sharing (mail-manage surface) ─────

cmd_hostmail_group() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_group; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        create|add|remove|list|show|delete) ;;
        --help|-h) cmd_help_hostmail_group; exit 0 ;;
        *)
            _mail_err "usage" "unknown group command '$sub' — usage: k2 hostmail group <create|add|remove|list|show|delete>"
            exit 2 ;;
    esac
    if _wants_help "$@"; then cmd_help_hostmail_group; exit 0; fi
    local as_json=false mail=false domain="" positional=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --mail)
                [ "$sub" = "create" ] || { _mail_err "usage" "--mail goes with 'group create'"; exit 2; }
                mail=true; shift ;;
            --domain)
                [ "$sub" = "create" ] || { _mail_err "usage" "--domain goes with 'group create'"; exit 2; }
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — see k2 hostmail group --help"
                exit 2 ;;
            *) positional+=("$1"); shift ;;
        esac
    done
    local n=${#positional[@]}
    case "$sub" in
        create)
            [ "$n" -eq 1 ] || { _mail_err "usage" "k2 hostmail group create <name> [--domain <apex>] [--mail]"; exit 2; }
            local M_VERB="group_create" M_JSON="$as_json" M_NAME="${positional[0]}" \
                  M_DOMAIN="$domain" M_GROUP_MAIL="$mail"
            _mail_py ;;
        add|remove)
            [ "$n" -ge 2 ] || { _mail_err "usage" "k2 hostmail group $sub <group> <addr> [<addr>…]"; exit 2; }
            local members="" i
            for (( i=1; i<n; i++ )); do
                if [ -n "$members" ]; then members="$members
${positional[$i]}"; else members="${positional[$i]}"; fi
            done
            local M_VERB="group_members" M_JSON="$as_json" M_GROUP="${positional[0]}" \
                  M_LIST_OP="$sub" M_MEMBERS="$members"
            _mail_py ;;
        list)
            [ "$n" -eq 0 ] || { _mail_err "usage" "k2 hostmail group list [--json]"; exit 2; }
            local M_VERB="group_list" M_JSON="$as_json"
            _mail_py ;;
        show|delete)
            [ "$n" -eq 1 ] || { _mail_err "usage" "k2 hostmail group $sub <group> [--json]"; exit 2; }
            local M_VERB="group_$sub" M_JSON="$as_json" M_GROUP="${positional[0]}"
            _mail_py ;;
    esac
}


# k2 hostmail calendar create|rename|share|unshare|shares|sharing
cmd_hostmail_calendar_share() {
    local sub="$1"; shift
    if _wants_help "$@"; then cmd_help_hostmail_calendar_share; exit 0; fi
    local as_json=false calendar="" with="" level="" color="" max_shares="" positional=()
    local allowed
    case "$sub" in
        create)  allowed=" --color --json " ;;
        rename)  allowed=" --color --json " ;;
        share)   allowed=" --calendar --with --read --write --editor --freebusy --json " ;;
        unshare) allowed=" --calendar --with --json " ;;
        shares)  allowed=" --json " ;;
        sharing) allowed=" --max-shares --json " ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --*)
                case "$allowed" in
                    *" $1 "*) ;;
                    *)
                        _mail_err "usage" "'k2 hostmail calendar $sub' does not take '$1' — see k2 hostmail calendar --help"
                        exit 2 ;;
                esac
                case "$1" in
                    --json) as_json=true; shift; continue ;;
                    --read|--write|--editor|--freebusy)
                        if [ -n "$level" ]; then
                            _mail_err "usage" "give ONE of --freebusy --read --write --editor"
                            exit 2
                        fi
                        level="${1#--}"; shift; continue ;;
                esac
                [ $# -ge 2 ] || { _mail_err "usage" "$1 requires a value — see k2 hostmail calendar --help"; exit 2; }
                case "$1" in
                    --calendar) calendar="$2" ;;
                    --with) with="$2" ;;
                    --color) color="$2" ;;
                    --max-shares) max_shares="$2" ;;
                esac
                shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — see k2 hostmail calendar --help"
                exit 2 ;;
            *) positional+=("$1"); shift ;;
        esac
    done
    local n=${#positional[@]}
    case "$sub" in
        create)
            [ "$n" -eq 2 ] || { _mail_err "usage" "k2 hostmail calendar create <owner-addr> <name> [--color <c>]"; exit 2; }
            local M_VERB="cal_manage" M_JSON="$as_json" M_CAL_ACTION="create" \
                  M_ADDRESS="${positional[0]}" M_NAME="${positional[1]}" M_COLOR="$color"
            _mail_py ;;
        rename)
            [ "$n" -eq 3 ] || { _mail_err "usage" "k2 hostmail calendar rename <owner-addr> <calendar> <new-name> [--color <c>]"; exit 2; }
            local M_VERB="cal_manage" M_JSON="$as_json" M_CAL_ACTION="rename" \
                  M_ADDRESS="${positional[0]}" M_CALENDAR="${positional[1]}" \
                  M_NAME="${positional[2]}" M_COLOR="$color"
            _mail_py ;;
        share|unshare)
            [ "$n" -eq 1 ] || { _mail_err "usage" "k2 hostmail calendar $sub <owner-addr> [--calendar <id|name>] --with <addr|group>"; exit 2; }
            [ -n "$with" ] || { _mail_err "usage" "$sub needs --with <addr|group>"; exit 2; }
            if [ "$sub" = "share" ] && [ -z "$level" ]; then
                _mail_err "usage" "share needs one of --freebusy --read --write --editor"
                exit 2
            fi
            local M_VERB="cal_$sub" M_JSON="$as_json" M_ADDRESS="${positional[0]}" \
                  M_CALENDAR="$calendar" M_WITH="$with" M_SHARE_LEVEL="$level"
            _mail_py ;;
        shares)
            [ "$n" -eq 1 ] || { _mail_err "usage" "k2 hostmail calendar shares <owner-addr> [--json]"; exit 2; }
            local M_VERB="cal_shares" M_JSON="$as_json" M_ADDRESS="${positional[0]}"
            _mail_py ;;
        sharing)
            [ "$n" -eq 0 ] || { _mail_err "usage" "k2 hostmail calendar sharing [--max-shares N] [--json]"; exit 2; }
            if [ -n "$max_shares" ] && ! _mail_is_uint "$max_shares"; then
                _mail_err "usage" "--max-shares must be a number from 1 to 1000"
                exit 2
            fi
            local M_VERB="cal_sharing" M_JSON="$as_json" M_MAX_SHARES="$max_shares"
            _mail_py ;;
    esac
}


cmd_hostmail_ptr() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_ptr; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        show) cmd_hostmail_ptr_show "$@" ;;
        set) cmd_hostmail_ptr_set "$@" ;;
        --help|-h) cmd_help_hostmail_ptr; exit 0 ;;
        *)
            _mail_err "usage" "unknown ptr command '$sub' — usage: k2 hostmail ptr <show|set>"
            exit 2 ;;
    esac
}


cmd_hostmail_ptr_show() {
    if _wants_help "$@"; then cmd_help_hostmail_ptr_show; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail ptr show [--json]"
                exit 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail ptr show [--json]"
                exit 2 ;;
        esac
    done
    local M_VERB="ptr_show" M_JSON="$as_json"
    _mail_py
}


cmd_hostmail_ptr_set() {
    if _wants_help "$@"; then cmd_help_hostmail_ptr_set; exit 0; fi
    local hostname="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --hostname)
                [ $# -ge 2 ] || { _mail_err "usage" "--hostname requires a value"; exit 2; }
                hostname="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail ptr set <hostname> [--json]"
                exit 2 ;;
            *)
                if [ -z "$hostname" ]; then hostname="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one hostname only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$hostname" ] || { _mail_err "usage" "k2 hostmail ptr set <hostname>"; exit 2; }
    local M_VERB="ptr_set" M_JSON="$as_json" M_HOSTNAME="$hostname"
    _mail_py
}


cmd_hostmail_bans() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_bans; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        list) cmd_hostmail_bans_list "$@" ;;
        clear) cmd_hostmail_bans_clear "$@" ;;
        migrate) cmd_hostmail_bans_migrate "$@" ;;
        --help|-h) cmd_help_hostmail_bans; exit 0 ;;
        *)
            _mail_err "usage" "unknown bans command '$sub' — usage: k2 hostmail bans <list|clear|migrate>"
            exit 2 ;;
    esac
}


cmd_hostmail_bans_list() {
    if _wants_help "$@"; then cmd_help_hostmail_bans_list; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail bans list [--json]"
                exit 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail bans list [--json]"
                exit 2 ;;
        esac
    done
    local M_VERB="bans_list" M_JSON="$as_json"
    _mail_py
}


cmd_hostmail_bans_clear() {
    if _wants_help "$@"; then cmd_help_hostmail_bans_clear; exit 0; fi
    local ip="" as_json=false all=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --all) all=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail bans clear <ip> [--all] [--json]"
                exit 2 ;;
            *)
                if [ -z "$ip" ]; then ip="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one IP only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$ip" ] || { _mail_err "usage" "k2 hostmail bans clear <ip> [--all]"; exit 2; }
    local M_VERB="bans_clear" M_JSON="$as_json" M_IP="$ip" M_ALL="$all"
    _mail_py
}


cmd_hostmail_bans_migrate() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_bans_migrate; exit 0; fi
    local sub="$1"
    case "$sub" in
        restore)
            shift
            cmd_hostmail_bans_migrate_restore "$@"
            ;;
        status)
            shift
            cmd_hostmail_bans_migrate_status "$@"
            ;;
        --help|-h) cmd_help_hostmail_bans_migrate; exit 0 ;;
        *)
            # `migrate --hours N` or `migrate status`-style flags
            cmd_hostmail_bans_migrate_start "$@"
            ;;
    esac
}


cmd_hostmail_bans_migrate_start() {
    if _wants_help "$@"; then cmd_help_hostmail_bans_migrate; exit 0; fi
    local hours="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --hours)
                [ $# -ge 2 ] || { _mail_err "usage" "--hours requires N"; exit 2; }
                hours="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail bans migrate --hours N [--json]"
                exit 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail bans migrate --hours N"
                exit 2 ;;
        esac
    done
    [ -n "$hours" ] || { _mail_err "usage" "k2 hostmail bans migrate --hours N"; exit 2; }
    case "$hours" in
        ''|*[!0-9]*) _mail_err "usage" "--hours must be a positive integer"; exit 2 ;;
        0) _mail_err "usage" "--hours must be >= 1"; exit 2 ;;
    esac
    local M_VERB="bans_migrate" M_JSON="$as_json" M_HOURS="$hours"
    _mail_py
}


cmd_hostmail_bans_migrate_restore() {
    if _wants_help "$@"; then cmd_help_hostmail_bans_migrate_restore; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail bans migrate restore [--json]"
                exit 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1'"
                exit 2 ;;
        esac
    done
    local M_VERB="bans_migrate_restore" M_JSON="$as_json"
    _mail_py
}


cmd_hostmail_bans_migrate_status() {
    if _wants_help "$@"; then cmd_help_hostmail_bans_migrate_status; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail bans migrate status [--json]"
                exit 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1'"
                exit 2 ;;
        esac
    done
    local M_VERB="bans_migrate_status" M_JSON="$as_json"
    _mail_py
}


cmd_hostmail_allowlist() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_allowlist; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        list) cmd_hostmail_allowlist_list "$@" ;;
        add) cmd_hostmail_allowlist_add "$@" ;;
        remove) cmd_hostmail_allowlist_remove "$@" ;;
        --help|-h) cmd_help_hostmail_allowlist; exit 0 ;;
        *)
            _mail_err "usage" "unknown allowlist command '$sub' — usage: k2 hostmail allowlist <list|add|remove>"
            exit 2 ;;
    esac
}


cmd_hostmail_allowlist_list() {
    if _wants_help "$@"; then cmd_help_hostmail_allowlist_list; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail allowlist list [--json]"
                exit 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1'"
                exit 2 ;;
        esac
    done
    local M_VERB="allowlist_list" M_JSON="$as_json"
    _mail_py
}


cmd_hostmail_allowlist_add() {
    if _wants_help "$@"; then cmd_help_hostmail_allowlist_add; exit 0; fi
    local address="" reason="" expires="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --reason)
                [ $# -ge 2 ] || { _mail_err "usage" "--reason requires a value"; exit 2; }
                reason="$2"; shift 2 ;;
            --expires)
                [ $# -ge 2 ] || { _mail_err "usage" "--expires requires RFC3339"; exit 2; }
                expires="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail allowlist add <ip|cidr> [--reason migration] [--expires <rfc3339>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail allowlist add <ip|cidr>"; exit 2; }
    local M_VERB="allowlist_add" M_JSON="$as_json" M_ADDRESS="$address" M_REASON="$reason" M_EXPIRES="$expires"
    _mail_py
}


cmd_hostmail_allowlist_remove() {
    if _wants_help "$@"; then cmd_help_hostmail_allowlist_remove; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail allowlist remove <ip|cidr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail allowlist remove <ip|cidr>"; exit 2; }
    local M_VERB="allowlist_remove" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}


cmd_hostmail_dmarc_show() {
    if _wants_help "$@"; then cmd_help_hostmail_dmarc_show; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail dmarc show <domain> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one domain only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] || { _mail_err "usage" "k2 hostmail dmarc show <domain>"; exit 2; }
    local M_VERB="dmarc_show" M_JSON="$as_json" M_DOMAIN="$domain"
    _mail_py
}


cmd_hostmail_dmarc_report_to() {
    if _wants_help "$@"; then cmd_help_hostmail_dmarc_report_to; exit 0; fi
    local domain="" address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail dmarc report-to <domain> --address <minted> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] || { _mail_err "usage" "k2 hostmail dmarc report-to <domain> --address <minted>"; exit 2; }
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail dmarc report-to <domain> --address <minted>"; exit 2; }
    local M_VERB="dmarc_report_to" M_JSON="$as_json" M_DOMAIN="$domain" M_ADDRESS="$address"
    _mail_py
}

