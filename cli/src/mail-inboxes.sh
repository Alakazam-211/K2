# ── S11 unified inbox surface: inboxes / link / access ──────────────────

cmd_mail_inboxes() {
    if _wants_help "$@"; then cmd_help_mail_inboxes; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — usage: k2 mail inboxes [--json]"
                exit 2 ;;
        esac
    done
    local M_VERB="inboxes" M_JSON="$as_json"
    _mail_py
}

# `k2 mail link <add|list|remove>` — connect the user's OWN external
# IMAP accounts as assistant inboxes (was `mail external add|…`).
cmd_mail_link() {
    if [ $# -eq 0 ] || _wants_help "$@"; then cmd_help_mail_link; exit 0; fi
    cmd_mail_external "$@"
}

# `k2 mail access <grant|revoke|primary|level>` — the PRIMARY/owner
# access surface over BOTH hosted + linked inboxes (S11).
cmd_mail_access() {
    if [ $# -eq 0 ] || _wants_help "$@"; then cmd_help_mail_access; exit 0; fi
    local sub="$1"; shift
    local verb=""
    case "$sub" in
        grant)   verb="access_grant" ;;
        revoke)  verb="access_revoke" ;;
        primary) verb="access_primary" ;;
        level)   verb="access_level" ;;
        manage)  verb="access_manage" ;;
        *)
            _mail_err "usage" "unknown access command '$sub' — usage: k2 mail access <grant|revoke|primary|level|manage>"
            exit 2 ;;
    esac
    local address="" workspace="" level="" as_json=false manage_v="" delete_v=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --level)
                [ $# -ge 2 ] || { _mail_err "usage" "--level requires read, draft, or send"; exit 2; }
                level="$2"; shift 2 ;;
            --on)         manage_v="true"; shift ;;
            --off)        manage_v="false"; shift ;;
            --delete)     delete_v="true"; shift ;;
            --no-delete)  delete_v="false"; shift ;;
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — see k2 mail access --help"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"
                elif [ -z "$workspace" ]; then workspace="$1"
                else _mail_err "usage" "unexpected extra argument '$1'"; exit 2; fi
                shift ;;
        esac
    done
    if [ -z "$address" ] || [ -z "$workspace" ]; then
        _mail_err "usage" "access $sub requires <address> <workspace>"
        exit 2
    fi
    # The on/off/delete flags belong to `manage` only.
    if [ "$verb" != "access_manage" ] && { [ -n "$manage_v" ] || [ -n "$delete_v" ]; }; then
        _mail_err "usage" "--on/--off/--delete/--no-delete only apply to 'k2 mail access manage'"
        exit 2
    fi
    if [ "$verb" = "access_grant" ] || [ "$verb" = "access_level" ]; then
        if [ -z "$level" ] && [ "$verb" = "access_grant" ]; then level="read"; fi
        if [ -z "$level" ]; then
            _mail_err "usage" "access $sub requires --level read|draft|send"
            exit 2
        fi
        case "$level" in
            read|draft|send) ;;
            *) _mail_err "usage" "--level must be read, draft, or send (send is hosted-only)"; exit 2 ;;
        esac
    fi
    if [ "$verb" = "access_manage" ]; then
        # Default: enable manage; delete stays off unless asked. Clearing
        # manage (--off) also clears delete (the daemon enforces the same).
        [ -n "$manage_v" ] || manage_v="true"
        [ -n "$delete_v" ] || delete_v="false"
        if [ "$manage_v" = "false" ]; then delete_v="false"; fi
        if [ "$delete_v" = "true" ] && [ "$manage_v" = "false" ]; then
            _mail_err "usage" "delete requires manage — use --on --delete (a workspace can't delete what it can't manage)"
            exit 2
        fi
        local M_VERB="$verb" M_JSON="$as_json" M_ADDRESS="$address" \
              M_WORKSPACE="$workspace" M_MANAGE="$manage_v" M_DELETE="$delete_v"
        _mail_py
        return
    fi
    local M_VERB="$verb" M_JSON="$as_json" M_ADDRESS="$address" \
          M_WORKSPACE="$workspace" M_LEVEL="$level"
    _mail_py
}

