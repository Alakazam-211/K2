# ── k2 mail help text ─────────────────────────────────────────────────


_hostmail_read_text() {
    local raw="$1"
    if [ "$raw" = "-" ]; then
        cat
    elif [ "${raw#@}" != "$raw" ]; then
        local path="${raw#@}"
        if [ -z "$path" ]; then
            _mail_err "usage" "--text @path needs a path"
            return 1
        fi
        if [ ! -f "$path" ]; then
            _mail_err "usage" "no such file: $path"
            return 1
        fi
        cat "$path"
    else
        printf '%s' "$raw"
    fi
}

cmd_hostmail_ooo() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_ooo; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        set) cmd_hostmail_ooo_set "$@" ;;
        unset) cmd_hostmail_ooo_unset "$@" ;;
        show) cmd_hostmail_ooo_show "$@" ;;
        --help|-h) cmd_help_hostmail_ooo; exit 0 ;;
        *)
            _mail_err "usage" "unknown ooo command '$sub' — usage: k2 hostmail ooo <set|unset|show>"
            exit 2 ;;
    esac
}


cmd_hostmail_ooo_set() {
    if _wants_help "$@"; then cmd_help_hostmail_ooo_set; exit 0; fi
    local address="" text="" days="" until="" as_json=false text_set=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --text)
                [ $# -ge 2 ] || { _mail_err "usage" "--text requires a value (literal, @path, or -)"; exit 2; }
                if ! text="$(_hostmail_read_text "$2")"; then exit 2; fi
                text_set=true; shift 2 ;;
            --days)
                [ $# -ge 2 ] || { _mail_err "usage" "--days requires 1-31"; exit 2; }
                days="$2"; shift 2 ;;
            --until)
                [ $# -ge 2 ] || { _mail_err "usage" "--until requires UTC YYYY-MM-DD"; exit 2; }
                until="$2"; shift 2 ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail ooo set <addr> --text FILE_OR_STRING [--days N] [--until DATE]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one address only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail ooo set <addr> --text FILE_OR_STRING [--days N] [--until DATE]"; exit 2; }
    $text_set || { _mail_err "usage" "ooo set needs --text (literal, @path, or -)"; exit 2; }
    if [ -n "$days" ]; then
        if ! _mail_is_uint "$days" || [ "$days" -lt 1 ] || [ "$days" -gt 31 ]; then
            _mail_err "usage" "--days must be 1-31"
            exit 2
        fi
    fi
    local M_VERB="ooo_set" M_JSON="$as_json" M_ADDRESS="$address" \
          M_TEXT="$text" M_DAYS="$days" M_UNTIL="$until"
    _mail_py
}


cmd_hostmail_ooo_unset() {
    if _wants_help "$@"; then cmd_help_hostmail_ooo_unset; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail ooo unset <addr>"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one address only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail ooo unset <addr>"; exit 2; }
    local M_VERB="ooo_unset" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}


cmd_hostmail_ooo_show() {
    if _wants_help "$@"; then cmd_help_hostmail_ooo_show; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail ooo show <addr>"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one address only"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail ooo show <addr>"; exit 2; }
    local M_VERB="ooo_get" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}


# 0.46.6: read-only audit of every mailbox's Sieve scripts (owner/admin).
cmd_hostmail_audit_sieve() {
    if _wants_help "$@"; then cmd_help_hostmail_audit_sieve; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail audit-sieve [--json]"; exit 2 ;;
        esac
    done
    local M_VERB="audit_sieve" M_JSON="$as_json"
    _mail_py
}


cmd_hostmail_footer() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_footer; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        set) cmd_hostmail_footer_set "$@" ;;
        unset) cmd_hostmail_footer_unset "$@" ;;
        show) cmd_hostmail_footer_show "$@" ;;
        --help|-h) cmd_help_hostmail_footer; exit 0 ;;
        *)
            _mail_err "usage" "unknown footer command '$sub' — usage: k2 hostmail footer <set|unset|show>"
            exit 2 ;;
    esac
}


cmd_hostmail_footer_set() {
    if _wants_help "$@"; then cmd_help_hostmail_footer_set; exit 0; fi
    local text="" domain="" as_json=false text_set=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --text)
                [ $# -ge 2 ] || { _mail_err "usage" "--text requires a value (literal, @path, or -)"; exit 2; }
                if ! text="$(_hostmail_read_text "$2")"; then exit 2; fi
                text_set=true; shift 2 ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain is reserved (footer is host-wide)"; exit 2; }
                domain="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail footer set --text FILE_OR_STRING"
                exit 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail footer set --text FILE_OR_STRING"
                exit 2 ;;
        esac
    done
    $text_set || { _mail_err "usage" "footer set needs --text (literal, @path, or -)"; exit 2; }
    local M_VERB="footer_set" M_JSON="$as_json" M_TEXT="$text" M_DOMAIN="$domain"
    _mail_py
}


cmd_hostmail_footer_unset() {
    if _wants_help "$@"; then cmd_help_hostmail_footer_unset; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain is reserved (footer is host-wide)"; exit 2; }
                domain="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail footer unset"
                exit 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail footer unset"
                exit 2 ;;
        esac
    done
    local M_VERB="footer_unset" M_JSON="$as_json" M_DOMAIN="$domain"
    _mail_py
}


cmd_hostmail_footer_show() {
    if _wants_help "$@"; then cmd_help_hostmail_footer_show; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --domain)
                _mail_err "usage" "--domain is reserved — footer is host-wide this cut"
                exit 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail footer show"
                exit 2 ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail footer show"
                exit 2 ;;
        esac
    done
    local M_VERB="footer_get" M_JSON="$as_json"
    _mail_py
}

