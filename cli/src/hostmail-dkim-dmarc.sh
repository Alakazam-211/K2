# ── k2 mail help text ─────────────────────────────────────────────────


cmd_hostmail_dkim() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_dkim; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        show) cmd_hostmail_dkim_show "$@" ;;
        rotate) cmd_hostmail_dkim_rotate "$@" ;;
        retire) cmd_hostmail_dkim_retire "$@" ;;
        --help|-h) cmd_help_hostmail_dkim; exit 0 ;;
        *)
            _mail_err "usage" "unknown dkim command '$sub' — usage: k2 hostmail dkim <show|rotate|retire>"
            exit 2 ;;
    esac
}


cmd_hostmail_dkim_show() {
    if _wants_help "$@"; then cmd_help_hostmail_dkim_show; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail dkim show <domain> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one domain only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] || { _mail_err "usage" "k2 hostmail dkim show <domain>"; exit 2; }
    local M_VERB="dkim_show" M_JSON="$as_json" M_DOMAIN="$domain"
    _mail_py
}


cmd_hostmail_dkim_rotate() {
    if _wants_help "$@"; then cmd_help_hostmail_dkim_rotate; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail dkim rotate <domain> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one domain only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] || { _mail_err "usage" "k2 hostmail dkim rotate <domain>"; exit 2; }
    local M_VERB="dkim_rotate" M_JSON="$as_json" M_DOMAIN="$domain"
    _mail_py
}


cmd_hostmail_dkim_retire() {
    if _wants_help "$@"; then cmd_help_hostmail_dkim_retire; exit 0; fi
    local domain="" selector="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            --selector)
                [ $# -ge 2 ] || { _mail_err "usage" "--selector requires a value"; exit 2; }
                selector="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail dkim retire <domain> <selector> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"
                elif [ -z "$selector" ]; then selector="$1"
                else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] && [ -n "$selector" ] || { _mail_err "usage" "k2 hostmail dkim retire <domain> <selector>"; exit 2; }
    local M_VERB="dkim_retire" M_JSON="$as_json" M_DOMAIN="$domain" M_SELECTOR="$selector"
    _mail_py
}


cmd_hostmail_dmarc() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_dmarc; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        show) cmd_hostmail_dmarc_show "$@" ;;
        report-to) cmd_hostmail_dmarc_report_to "$@" ;;
        --help|-h) cmd_help_hostmail_dmarc; exit 0 ;;
        *)
            _mail_err "usage" "unknown dmarc command '$sub' — usage: k2 hostmail dmarc <show|report-to>"
            exit 2 ;;
    esac
}


cmd_hostmail_profile() {
    if _wants_help "$@"; then cmd_help_hostmail_profile; exit 0; fi
    local address="" apple=false out="" force=false as_json=false
    local usage="k2 hostmail profile <addr> --apple [--out <file> [--force]] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --apple) apple=true; shift ;;
            --json) as_json=true; shift ;;
            --force) force=true; shift ;;
            --out)
                [ $# -ge 2 ] && [ -n "$2" ] || { _mail_err "usage" "--out requires a file path — usage: $usage"; exit 2; }
                out="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: $usage"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one <addr> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "$usage"; exit 2; }
    if [ "$apple" != true ]; then
        _mail_err "usage" "pass --apple (the Apple .mobileconfig is the only profile format) — usage: $usage"
        exit 2
    fi
    if [ "$force" = true ] && [ -z "$out" ]; then
        _mail_err "usage" "--force only goes with --out <file>"
        exit 2
    fi
    if [ -n "$out" ]; then
        # Refuse BEFORE the daemon mints an app password.
        if [ -d "$out" ]; then
            _mail_err "usage" "--out $out is a directory — pass a file path (e.g. $out/$address.mobileconfig)"
            exit 2
        fi
        if { [ -e "$out" ] || [ -L "$out" ]; } && [ "$force" != true ]; then
            _mail_err "usage" "$out already exists — pass --force to overwrite (nothing was minted)"
            exit 2
        fi
        local out_dir
        out_dir="$(dirname -- "$out")"
        if [ ! -d "$out_dir" ]; then
            _mail_err "usage" "--out directory $out_dir does not exist"
            exit 2
        fi
    fi
    local M_VERB="profile" M_JSON="$as_json" M_ADDRESS="$address" M_OUT="$out" M_FORCE="$force"
    _mail_py
}


cmd_hostmail_calendar() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_calendar; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        status|enable|disable) cmd_hostmail_calendar_action "$sub" "$@" ;;
        create|rename|share|unshare|shares|sharing) cmd_hostmail_calendar_share "$sub" "$@" ;;
        --help|-h) cmd_help_hostmail_calendar; exit 0 ;;
        *)
            _mail_err "usage" "unknown calendar command '$sub' — usage: k2 hostmail calendar <status|enable|disable|create|rename|share|unshare|shares|sharing>"
            exit 2 ;;
    esac
}


cmd_hostmail_calendar_action() {
    local action="$1"; shift
    if _wants_help "$@"; then
        case "$action" in
            status) cmd_help_hostmail_calendar_status ;;
            enable) cmd_help_hostmail_calendar_enable ;;
            disable) cmd_help_hostmail_calendar_disable ;;
        esac
        exit 0
    fi
    local as_json=false files=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --files)
                [ $# -ge 2 ] || { _mail_err "usage" "--files requires on|off — usage: k2 hostmail calendar $action [--files on|off] [--json]"; exit 2; }
                case "$2" in
                    on|off) files="$2" ;;
                    *) _mail_err "usage" "--files must be on or off (got '$2')"; exit 2 ;;
                esac
                shift 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail calendar $action [--files on|off] [--json]"
                exit 2 ;;
        esac
    done
    if [ "$action" = "status" ] && [ -n "$files" ]; then
        _mail_err "usage" "--files goes with enable|disable, not status"
        exit 2
    fi
    local M_VERB="dav" M_JSON="$as_json" M_DAV_ACTION="$action" M_FILES="$files"
    _mail_py
}



