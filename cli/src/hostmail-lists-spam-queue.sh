# ── k2 mail help text ─────────────────────────────────────────────────


cmd_hostmail_list() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_list; exit 2; fi
    local sub="$1"; shift
    case "$sub" in
        create) cmd_hostmail_list_create "$@" ;;
        members) cmd_hostmail_list_members "$@" ;;
        delete) cmd_hostmail_list_delete "$@" ;;
        show) cmd_hostmail_list_show "$@" ;;
        --help|-h) cmd_help_hostmail_list; exit 0 ;;
        *)
            _mail_err "usage" "unknown list command '$sub' — usage: k2 hostmail list <create|members|delete|show>"
            exit 2 ;;
    esac
}


cmd_hostmail_list_create() {
    if _wants_help "$@"; then cmd_help_hostmail_list_create; exit 0; fi
    local address="" members="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --member)
                [ $# -ge 2 ] || { _mail_err "usage" "--member requires an address"; exit 2; }
                if [ -n "$members" ]; then members="$members
$2"; else members="$2"; fi
                shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail list create <addr> --member <a> [--member <a>…] [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail list create <addr> --member <a>…"; exit 2; }
    local M_VERB="list_create" M_JSON="$as_json" M_ADDRESS="$address" M_MEMBERS="$members"
    _mail_py
}


cmd_hostmail_list_show() {
    if _wants_help "$@"; then cmd_help_hostmail_list_show; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail list show <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail list show <addr>"; exit 2; }
    local M_VERB="list_show" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}


cmd_hostmail_list_members() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_list_members; exit 0; fi
    local sub="$1"
    case "$sub" in
        add|remove)
            shift
            cmd_hostmail_list_members_mutate "$sub" "$@" ;;
        --help|-h) cmd_help_hostmail_list_members; exit 0 ;;
        *)
            cmd_hostmail_list_show "$@" ;;
    esac
}


cmd_hostmail_list_members_mutate() {
    local op="$1"; shift
    if _wants_help "$@"; then cmd_help_hostmail_list_members; exit 0; fi
    local address="" members="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --member)
                [ $# -ge 2 ] || { _mail_err "usage" "--member requires an address"; exit 2; }
                if [ -n "$members" ]; then members="$members
$2"; else members="$2"; fi
                shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail list members $op <addr> --member <a>…"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail list members $op <addr> --member <a>…"; exit 2; }
    [ -n "$members" ] || { _mail_err "usage" "give --member <addr>"; exit 2; }
    local M_VERB="list_members" M_JSON="$as_json" M_ADDRESS="$address" M_LIST_OP="$op" M_MEMBERS="$members"
    _mail_py
}


cmd_hostmail_list_delete() {
    if _wants_help "$@"; then cmd_help_hostmail_list_delete; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail list delete <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail list delete <addr>"; exit 2; }
    local M_VERB="list_delete" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}


cmd_hostmail_spam() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_spam; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        ham|spam) cmd_hostmail_spam_train "$sub" "$@" ;;
        allow|block) cmd_hostmail_spam_sender "$sub" "$@" ;;
        quarantine) cmd_hostmail_spam_quarantine "$@" ;;
        --help|-h) cmd_help_hostmail_spam; exit 0 ;;
        *)
            _mail_err "usage" "unknown spam command '$sub' — usage: k2 hostmail spam <ham|spam|allow|block|quarantine>"
            exit 2 ;;
    esac
}


cmd_hostmail_spam_train() {
    local class="$1"; shift
    if _wants_help "$@"; then cmd_help_hostmail_spam_train; exit 0; fi
    local address="" msgid="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail spam $class <message-id> --address <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$msgid" ]; then msgid="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$msgid" ] || { _mail_err "usage" "k2 hostmail spam $class <message-id> --address <addr>"; exit 2; }
    [ -n "$address" ] || { _mail_err "usage" "--address is required (ids are per-inbox)"; exit 2; }
    local M_VERB="spam_train" M_JSON="$as_json" M_ADDRESS="$address" M_ID="$msgid" M_CLASS="$class"
    _mail_py
}


cmd_hostmail_spam_sender() {
    local op="$1"; shift
    if _wants_help "$@"; then cmd_help_hostmail_spam_sender; exit 0; fi
    local sender="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail spam $op <sender> [--json]"
                exit 2 ;;
            *)
                if [ -z "$sender" ]; then sender="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$sender" ] || { _mail_err "usage" "k2 hostmail spam $op <sender>"; exit 2; }
    local M_VERB="spam_sender" M_JSON="$as_json" M_SENDER="$sender" M_LIST_OP="$op"
    _mail_py
}


cmd_hostmail_spam_quarantine() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_spam_quarantine; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        list) cmd_hostmail_spam_quarantine_list "$@" ;;
        release|discard) cmd_hostmail_spam_quarantine_id "$sub" "$@" ;;
        --help|-h) cmd_help_hostmail_spam_quarantine; exit 0 ;;
        *)
            _mail_err "usage" "unknown quarantine command '$sub' — usage: k2 hostmail spam quarantine <list|release|discard>"
            exit 2 ;;
    esac
}


cmd_hostmail_spam_quarantine_list() {
    if _wants_help "$@"; then cmd_help_hostmail_spam_quarantine; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail spam quarantine list --address <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$address" ] || { _mail_err "usage" "k2 hostmail spam quarantine list --address <addr>"; exit 2; }
    local M_VERB="spam_quarantine_list" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}


cmd_hostmail_spam_quarantine_id() {
    local op="$1"; shift
    if _wants_help "$@"; then cmd_help_hostmail_spam_quarantine; exit 0; fi
    local address="" msgid="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --address)
                [ $# -ge 2 ] || { _mail_err "usage" "--address requires a value"; exit 2; }
                address="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail spam quarantine $op <id> --address <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$msgid" ]; then msgid="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$msgid" ] || { _mail_err "usage" "k2 hostmail spam quarantine $op <id> --address <addr>"; exit 2; }
    [ -n "$address" ] || { _mail_err "usage" "--address is required"; exit 2; }
    local M_VERB="spam_quarantine_id" M_JSON="$as_json" M_ADDRESS="$address" M_ID="$msgid" M_LIST_OP="$op"
    _mail_py
}


cmd_hostmail_queue() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_queue; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        list) cmd_hostmail_queue_list "$@" ;;
        retry) cmd_hostmail_queue_retry "$@" ;;
        drop) cmd_hostmail_queue_drop "$@" ;;
        --help|-h) cmd_help_hostmail_queue; exit 0 ;;
        *)
            _mail_err "usage" "unknown queue command '$sub' — usage: k2 hostmail queue <list|retry|drop>"
            exit 2 ;;
    esac
}


cmd_hostmail_queue_list() {
    if _wants_help "$@"; then cmd_help_hostmail_queue; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *) _mail_err "usage" "unexpected argument '$1' — usage: k2 hostmail queue list [--json]"; exit 2 ;;
        esac
    done
    local M_VERB="queue_list" M_JSON="$as_json"
    _mail_py
}


cmd_hostmail_queue_retry() {
    if _wants_help "$@"; then cmd_help_hostmail_queue; exit 0; fi
    local id="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail queue retry <id> [--json]"
                exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$id" ] || { _mail_err "usage" "k2 hostmail queue retry <id>"; exit 2; }
    local M_VERB="queue_retry" M_JSON="$as_json" M_ID="$id"
    _mail_py
}


cmd_hostmail_queue_drop() {
    if _wants_help "$@"; then cmd_help_hostmail_queue; exit 0; fi
    local id="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail queue drop <id> [--json]"
                exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$id" ] || { _mail_err "usage" "k2 hostmail queue drop <id>"; exit 2; }
    local M_VERB="queue_drop" M_JSON="$as_json" M_ID="$id"
    _mail_py
}


cmd_hostmail_acl() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_acl; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        grant) cmd_hostmail_acl_grant "$@" ;;
        revoke) cmd_hostmail_acl_revoke "$@" ;;
        show) cmd_hostmail_acl_show "$@" ;;
        --help|-h) cmd_help_hostmail_acl; exit 0 ;;
        *)
            _mail_err "usage" "unknown acl command '$sub' — usage: k2 hostmail acl <grant|revoke|show>"
            exit 2 ;;
    esac
}


cmd_hostmail_acl_grant() {
    if _wants_help "$@"; then cmd_help_hostmail_acl; exit 0; fi
    local mailbox="" user="" rights="" readf=false writef=false postf=false adminf=false as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --user)
                [ $# -ge 2 ] || { _mail_err "usage" "--user requires a minted address"; exit 2; }
                user="$2"; shift 2 ;;
            --rights)
                [ $# -ge 2 ] || { _mail_err "usage" "--rights requires lrswipkxtena"; exit 2; }
                rights="$2"; shift 2 ;;
            --read) readf=true; shift ;;
            --write) writef=true; shift ;;
            --post) postf=true; shift ;;
            --admin) adminf=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail acl grant <mailbox> --user <addr> --rights lrswipkxten | --read|--write|--post|--admin"
                exit 2 ;;
            *)
                if [ -z "$mailbox" ]; then mailbox="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$mailbox" ] || { _mail_err "usage" "k2 hostmail acl grant <mailbox> --user <addr> …"; exit 2; }
    [ -n "$user" ] || { _mail_err "usage" "--user is required"; exit 2; }
    local M_VERB="acl_grant" M_JSON="$as_json" M_MAILBOX="$mailbox" M_USER="$user" \
          M_RIGHTS="$rights" M_ACL_READ="$readf" M_ACL_WRITE="$writef" M_ACL_POST="$postf" M_ACL_ADMIN="$adminf"
    _mail_py
}


cmd_hostmail_acl_revoke() {
    if _wants_help "$@"; then cmd_help_hostmail_acl; exit 0; fi
    local mailbox="" user="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --user)
                [ $# -ge 2 ] || { _mail_err "usage" "--user requires a minted address"; exit 2; }
                user="$2"; shift 2 ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail acl revoke <mailbox> --user <addr> [--json]"
                exit 2 ;;
            *)
                if [ -z "$mailbox" ]; then mailbox="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$mailbox" ] || { _mail_err "usage" "k2 hostmail acl revoke <mailbox> --user <addr>"; exit 2; }
    [ -n "$user" ] || { _mail_err "usage" "--user is required"; exit 2; }
    local M_VERB="acl_revoke" M_JSON="$as_json" M_MAILBOX="$mailbox" M_USER="$user"
    _mail_py
}


cmd_hostmail_acl_show() {
    if _wants_help "$@"; then cmd_help_hostmail_acl; exit 0; fi
    local mailbox="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail acl show <mailbox> [--json]"
                exit 2 ;;
            *)
                if [ -z "$mailbox" ]; then mailbox="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$mailbox" ] || { _mail_err "usage" "k2 hostmail acl show <mailbox>"; exit 2; }
    local M_VERB="acl_show" M_JSON="$as_json" M_MAILBOX="$mailbox"
    _mail_py
}


cmd_hostmail_autoconfig() {
    if [ $# -eq 0 ]; then cmd_help_hostmail_autoconfig; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        show) cmd_hostmail_autoconfig_show "$@" ;;
        apply) cmd_hostmail_autoconfig_apply "$@" ;;
        --help|-h) cmd_help_hostmail_autoconfig; exit 0 ;;
        *)
            _mail_err "usage" "unknown autoconfig command '$sub' — usage: k2 hostmail autoconfig <show|apply>"
            exit 2 ;;
    esac
}


cmd_hostmail_autoconfig_show() {
    if _wants_help "$@"; then cmd_help_hostmail_autoconfig; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail autoconfig show <domain> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] || { _mail_err "usage" "k2 hostmail autoconfig show <domain>"; exit 2; }
    local M_VERB="autoconfig_show" M_JSON="$as_json" M_DOMAIN="$domain"
    _mail_py
}


cmd_hostmail_autoconfig_apply() {
    if _wants_help "$@"; then cmd_help_hostmail_autoconfig; exit 0; fi
    local domain="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 hostmail autoconfig apply <domain> [--json]"
                exit 2 ;;
            *)
                if [ -z "$domain" ]; then domain="$1"; else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$domain" ] || { _mail_err "usage" "k2 hostmail autoconfig apply <domain>"; exit 2; }
    local M_VERB="autoconfig_apply" M_JSON="$as_json" M_DOMAIN="$domain"
    _mail_py
}

