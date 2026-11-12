# ── cmd_mail_* wrappers (arg parsing + validation; PRD §11) ──────────

# Positive-integer check shared by --timeout/--limit/--get/... (bash
# 3.2-safe; no [[ =~ ]]).
_mail_is_uint() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

# Validate a wait/--wait timeout: integer, 1..900 (§8.2 bounds — the
# daemon enforces the same; this just gives a friendlier local error).
_mail_check_timeout() {
    if ! _mail_is_uint "$1" || [ "$1" -lt 1 ] || [ "$1" -gt 900 ]; then
        _mail_err "usage" "--timeout must be 1-900 seconds (default 300); loop the call to wait longer"
        exit 2
    fi
}

cmd_mail_create() {
    if _wants_help "$@"; then cmd_help_mail_create; exit 0; fi
    local local_part="" domain="" client_id="" as_json=false person=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --domain)
                [ $# -ge 2 ] || { _mail_err "usage" "--domain requires a value"; exit 2; }
                domain="$2"; shift 2 ;;
            --id)
                [ $# -ge 2 ] || { _mail_err "usage" "--id requires a value (the idempotency key)"; exit 2; }
                client_id="$2"; shift 2 ;;
            --person) person=true; shift ;;
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail create <localpart>[@<domain>] [--domain <d>] [--id <client-id>] [--person] [--json]"
                exit 2 ;;
            *)
                if [ -z "$local_part" ]; then local_part="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one <localpart>[@<domain>] only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$local_part" ]; then
        _mail_err "usage" "create requires a <localpart> (e.g. research-bot or research-bot@acme.dev)"
        exit 2
    fi
    local M_VERB="create" M_JSON="$as_json" M_LOCAL="$local_part" \
          M_DOMAIN="$domain" M_CLIENT_ID="$client_id" M_PERSON="$person"
    _mail_py
}

cmd_mail_addresses() {
    if _wants_help "$@"; then cmd_help_mail_addresses; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 mail addresses [--json]"
                exit 2 ;;
        esac
    done
    local M_VERB="addresses" M_JSON="$as_json"
    _mail_py
}

cmd_mail_domains() {
    if _wants_help "$@"; then cmd_help_mail_domains; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 mail domains [--json]"
                exit 2 ;;
        esac
    done
    local M_VERB="domains" M_JSON="$as_json"
    _mail_py
}

cmd_mail_messages() {
    if _wants_help "$@"; then cmd_help_mail_messages; exit 0; fi
    local address="" unread=false limit="" query="" from="" as_json=false
    local since="" before="" folder="" junk=false offset=""
    local usage="k2 mail messages [<address>] [--unread] [--limit N] [--offset N] [--since <date>] [--before <date>] [--folder <name>] [--junk] [--query <text>] [--from <substr>] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --unread) unread=true; shift ;;
            --limit)
                [ $# -ge 2 ] || { _mail_err "usage" "--limit requires a number (1-200)"; exit 2; }
                limit="$2"; shift 2 ;;
            --offset)
                [ $# -ge 2 ] || { _mail_err "usage" "--offset requires a whole number (0-based page cursor)"; exit 2; }
                offset="$2"; shift 2 ;;
            --since)
                [ $# -ge 2 ] || { _mail_err "usage" "--since requires a date (YYYY-MM-DD or a relative age like 7d/24h/30m)"; exit 2; }
                since="$2"; shift 2 ;;
            --before)
                [ $# -ge 2 ] || { _mail_err "usage" "--before requires a date (YYYY-MM-DD or a relative age like 7d/24h/30m)"; exit 2; }
                before="$2"; shift 2 ;;
            --folder)
                [ $# -ge 2 ] || { _mail_err "usage" "--folder requires a mailbox name"; exit 2; }
                folder="$2"; shift 2 ;;
            --junk) junk=true; shift ;;
            --query)
                [ $# -ge 2 ] || { _mail_err "usage" "--query requires a value (matches subject AND body)"; exit 2; }
                query="$2"; shift 2 ;;
            --from)
                [ $# -ge 2 ] || { _mail_err "usage" "--from requires a value (matches the From header)"; exit 2; }
                from="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: $usage"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one optional <address> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -n "$limit" ] && { ! _mail_is_uint "$limit" || [ "$limit" -lt 1 ] || [ "$limit" -gt 200 ]; }; then
        _mail_err "usage" "--limit must be a number from 1 to 200 (default 25)"
        exit 2
    fi
    if [ -n "$offset" ] && ! _mail_is_uint "$offset"; then
        _mail_err "usage" "--offset must be a whole number (0 = first page)"
        exit 2
    fi
    if $junk && [ -n "$folder" ]; then
        _mail_err "usage" "use either --junk or --folder <name>, not both"
        exit 2
    fi
    local M_VERB="messages" M_JSON="$as_json" M_ADDRESS="$address" \
          M_UNREAD="$unread" M_LIMIT="$limit" M_QUERY="$query" M_FROM="$from" \
          M_OFFSET="$offset" M_SINCE="$since" M_BEFORE="$before" \
          M_FOLDER="$folder" M_JUNK="$junk"
    _mail_py
}

cmd_mail_read() {
    if _wants_help "$@"; then cmd_help_mail_read; exit 0; fi
    local id="" html=false raw=false as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --html) html=true; shift ;;
            --raw) raw=true; shift ;;
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 mail read <message-id> [--html] [--raw] [--json]"
                exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one <message-id> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$id" ]; then
        _mail_err "usage" "read requires a <message-id> (from 'k2 mail messages')"
        exit 2
    fi
    if $html && $raw; then
        _mail_err "usage" "--html and --raw are mutually exclusive (--raw is the full RFC 822 blob)"
        exit 2
    fi
    local M_VERB="read" M_JSON="$as_json" M_ID="$id" M_HTML="$html" M_RAW="$raw"
    _mail_py
}

cmd_mail_attachments() {
    if _wants_help "$@"; then cmd_help_mail_attachments; exit 0; fi
    local id="" get="" out="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --get)
                [ $# -ge 2 ] || { _mail_err "usage" "--get requires the 1-based attachment number"; exit 2; }
                get="$2"; shift 2 ;;
            --out)
                [ $# -ge 2 ] || { _mail_err "usage" "--out requires a workspace-relative path"; exit 2; }
                out="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 mail attachments <message-id> [--get <n> --out <path>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1' — one <message-id> only"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$id" ]; then
        _mail_err "usage" "attachments requires a <message-id> (from 'k2 mail messages')"
        exit 2
    fi
    if [ -n "$get" ]; then
        if ! _mail_is_uint "$get" || [ "$get" -lt 1 ]; then
            _mail_err "usage" "--get takes the 1-based attachment number from the list"
            exit 2
        fi
        if [ -z "$out" ]; then
            _mail_err "usage" "--get needs --out <path> (workspace-relative; the daemon writes the bytes there, nothing more)"
            exit 2
        fi
    elif [ -n "$out" ]; then
        _mail_err "usage" "--out only makes sense with --get <n>"
        exit 2
    fi
    local M_VERB="attachments" M_JSON="$as_json" M_ID="$id" M_GET="$get" M_OUT="$out"
    _mail_py
}

