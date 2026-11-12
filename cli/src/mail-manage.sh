# ── 0081 inbox MANAGEMENT + DELETE (agent verbs; gated by the
#    per-workspace canManage/canDelete caps, orthogonal to level). DELETE
#    is ALWAYS a move to Trash — never a permanent erase. ────────────────

# `k2 mail move <message-id> <folder>` — move a message to a folder.
cmd_mail_move() {
    if _wants_help "$@"; then cmd_help_mail_manage; exit 0; fi
    local id="" folder="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*) _mail_err "usage" "unknown flag '$1' — usage: k2 mail move <message-id> <folder> [--json]"; exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"
                elif [ -z "$folder" ]; then folder="$1"
                else _mail_err "usage" "unexpected extra argument '$1'"; exit 2; fi
                shift ;;
        esac
    done
    [ -n "$id" ] || { _mail_err "usage" "move requires <message-id> <folder>"; exit 2; }
    [ -n "$folder" ] || { _mail_err "usage" "move requires a destination <folder> (see 'k2 mail folder list <address>')"; exit 2; }
    local M_VERB="move" M_JSON="$as_json" M_ID="$id" M_FOLDER="$folder"
    _mail_py
}

# `k2 mail flag <message-id> [--read|--unread] [--star|--unstar]`.
cmd_mail_flag() {
    if _wants_help "$@"; then cmd_help_mail_manage; exit 0; fi
    local id="" read_v="" flagged_v="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --read) read_v="true"; shift ;;
            --unread) read_v="false"; shift ;;
            --star|--flag) flagged_v="true"; shift ;;
            --unstar|--unflag) flagged_v="false"; shift ;;
            --json) as_json=true; shift ;;
            -*) _mail_err "usage" "unknown flag '$1' — usage: k2 mail flag <message-id> [--read|--unread] [--star|--unstar] [--json]"; exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"
                else _mail_err "usage" "unexpected extra argument '$1' — one <message-id> only"; exit 2; fi
                shift ;;
        esac
    done
    [ -n "$id" ] || { _mail_err "usage" "flag requires a <message-id>"; exit 2; }
    if [ -z "$read_v" ] && [ -z "$flagged_v" ]; then
        _mail_err "usage" "flag needs at least one of --read/--unread or --star/--unstar"
        exit 2
    fi
    local M_VERB="flag" M_JSON="$as_json" M_ID="$id" M_READ="$read_v" M_FLAGGED="$flagged_v"
    _mail_py
}

# `k2 mail archive <message-id>` — move to the Archive folder.
cmd_mail_archive() {
    if _wants_help "$@"; then cmd_help_mail_manage; exit 0; fi
    local id="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*) _mail_err "usage" "unknown flag '$1' — usage: k2 mail archive <message-id> [--json]"; exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"
                else _mail_err "usage" "unexpected extra argument '$1' — one <message-id> only"; exit 2; fi
                shift ;;
        esac
    done
    [ -n "$id" ] || { _mail_err "usage" "archive requires a <message-id>"; exit 2; }
    local M_VERB="archive" M_JSON="$as_json" M_ID="$id"
    _mail_py
}

# `k2 mail delete <message-id|address>` — one verb, two forms, chosen by
# the ARGUMENT SHAPE (detection, not a breaking rename):
#   • a message id (opaque `m_…` token, base64url — never contains '@')
#     → MOVE the message to Trash (never erased; needs the delete cap).
#   • a hosted address (always contains '@') → RETIRE the address, exactly
#     as `k2 hostmail delete` does (back-compat; frees the cap slot).
#   • anything else → a usage error naming BOTH forms.
cmd_mail_delete() {
    if _wants_help "$@"; then cmd_help_mail_manage; exit 0; fi
    local arg="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*) _mail_err "usage" "unknown flag '$1' — usage: k2 mail delete <message-id|address> [--json]"; exit 2 ;;
            *)
                if [ -z "$arg" ]; then arg="$1"
                else _mail_err "usage" "unexpected extra argument '$1' — one <message-id|address> only"; exit 2; fi
                shift ;;
        esac
    done
    [ -n "$arg" ] || { _mail_err "usage" "delete requires a <message-id> (moves the message to Trash, never erases it) or an <address> (retires the hosted address)"; exit 2; }
    case "$arg" in
        *@*)
            # Address form → retire the hosted address (same as hostmail).
            local M_VERB="address_delete" M_JSON="$as_json" M_ADDRESS="$arg"
            _mail_py ;;
        m_*)
            # Message-id form → move that message to Trash (never erased).
            local M_VERB="delete" M_JSON="$as_json" M_ID="$arg"
            _mail_py ;;
        *)
            _mail_err "usage" "'$arg' is neither a message id ('m_…', from 'k2 mail messages') nor an address ('name@domain') — nothing deleted"
            exit 2 ;;
    esac
}

# `k2 mail folder <list|create|rename> <address> …` — folder management.
cmd_mail_folder() {
    if [ $# -eq 0 ] || _wants_help "$@"; then cmd_help_mail_manage; exit 0; fi
    local sub="$1"; shift
    case "$sub" in
        list|create|rename) ;;
        *) _mail_err "usage" "unknown folder command '$sub' — usage: k2 mail folder <list|create|rename> <address> …"; exit 2 ;;
    esac
    local p1="" p2="" p3="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*) _mail_err "usage" "unknown flag '$1' — see k2 mail folder --help"; exit 2 ;;
            *)
                if [ -z "$p1" ]; then p1="$1"
                elif [ -z "$p2" ]; then p2="$1"
                elif [ -z "$p3" ]; then p3="$1"
                else _mail_err "usage" "unexpected extra argument '$1'"; exit 2; fi
                shift ;;
        esac
    done
    case "$sub" in
        list)
            [ -n "$p1" ] || { _mail_err "usage" "folder list requires <address> ('k2 mail inboxes')"; exit 2; }
            local M_VERB="folder_list" M_JSON="$as_json" M_ADDRESS="$p1"
            _mail_py ;;
        create)
            { [ -n "$p1" ] && [ -n "$p2" ]; } || { _mail_err "usage" "folder create requires <address> <name>"; exit 2; }
            local M_VERB="folder_create" M_JSON="$as_json" M_ADDRESS="$p1" M_NAME="$p2"
            _mail_py ;;
        rename)
            { [ -n "$p1" ] && [ -n "$p2" ] && [ -n "$p3" ]; } || { _mail_err "usage" "folder rename requires <address> <from> <to>"; exit 2; }
            local M_VERB="folder_rename" M_JSON="$as_json" M_ADDRESS="$p1" M_FOLDER_FROM="$p2" M_FOLDER_TO="$p3"
            _mail_py ;;
    esac
}

# S9 (PRD §17.5): the user's OWN external accounts (Gmail, Fastmail,
# company IMAP), each bound to exactly ONE workspace. OWNER family —
# server-enforced (agent tokens exit 3). The password never rides argv:
# --pass-stdin pipes it, --pass-ref points at env:<VAR> / an absolute
# file path (resolved once daemon-side, then vaulted).
# Linked external IMAP accounts (S11 `k2 mail link`; internal name kept
# for the add/list/remove provisioning). grant/revoke moved to
# `k2 mail access`.
cmd_mail_external() {
    if _wants_help "$@"; then cmd_help_mail_link; exit 0; fi
    local sub=""
    case "${1:-}" in
        add|list|remove|upgrade|calendars) sub="$1"; shift ;;
        "") cmd_help_mail_link; exit 0 ;;
        -*) sub="list" ;;  # bare `k2 mail link --json` = list
        grant|revoke)
            _mail_err "usage" "per-workspace access moved — use 'k2 mail access ${1} <address> <workspace>'"
            exit 2 ;;
        *)
            _mail_err "usage" "unknown link command '${1:-}' — usage: k2 mail link <add|upgrade|calendars|list|remove>"
            exit 2 ;;
    esac
    local address="" workspace="" host="" port="" tls="" user="" name="" \
          drafts_folder="" pass_stdin=false pass_ref="" as_json=false \
          ws_arg="" level="" smtp_host="" smtp_port="" smtp_tls="" provider="" features=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --provider)
                [ $# -ge 2 ] || { _mail_err "usage" "--provider requires gmail or microsoft"; exit 2; }
                provider="$2"; shift 2 ;;
            --features)
                [ $# -ge 2 ] || { _mail_err "usage" "--features requires mail, calendar or mail,calendar"; exit 2; }
                features="$2"; shift 2 ;;
            --level)
                [ $# -ge 2 ] || { _mail_err "usage" "--level requires read, draft, or send"; exit 2; }
                level="$2"; shift 2 ;;
            --workspace)
                [ $# -ge 2 ] || { _mail_err "usage" "--workspace requires a value (the ONE workspace to bind)"; exit 2; }
                workspace="$2"; shift 2 ;;
            --host)
                [ $# -ge 2 ] || { _mail_err "usage" "--host requires a value (e.g. imap.gmail.com)"; exit 2; }
                host="$2"; shift 2 ;;
            --port)
                [ $# -ge 2 ] || { _mail_err "usage" "--port requires a number"; exit 2; }
                if ! _mail_is_uint "$2"; then
                    _mail_err "usage" "--port must be a number (default: 993 implicit-tls / 143 starttls)"
                    exit 2
                fi
                port="$2"; shift 2 ;;
            --tls)
                [ $# -ge 2 ] || { _mail_err "usage" "--tls requires implicit-tls or starttls"; exit 2; }
                tls="$2"; shift 2 ;;
            --user)
                [ $# -ge 2 ] || { _mail_err "usage" "--user requires the IMAP login (default: the address)"; exit 2; }
                user="$2"; shift 2 ;;
            --name)
                [ $# -ge 2 ] || { _mail_err "usage" "--name requires a display name"; exit 2; }
                name="$2"; shift 2 ;;
            --drafts-folder)
                [ $# -ge 2 ] || { _mail_err "usage" "--drafts-folder requires a folder name"; exit 2; }
                drafts_folder="$2"; shift 2 ;;
            --smtp-host)
                [ $# -ge 2 ] || { _mail_err "usage" "--smtp-host requires a value (e.g. smtp.gmail.com)"; exit 2; }
                smtp_host="$2"; shift 2 ;;
            --smtp-port)
                [ $# -ge 2 ] || { _mail_err "usage" "--smtp-port requires a number"; exit 2; }
                if ! _mail_is_uint "$2"; then
                    _mail_err "usage" "--smtp-port must be a number (default: 465 implicit-tls / 587 starttls)"
                    exit 2
                fi
                smtp_port="$2"; shift 2 ;;
            --smtp-tls)
                [ $# -ge 2 ] || { _mail_err "usage" "--smtp-tls requires implicit-tls or starttls"; exit 2; }
                smtp_tls="$2"; shift 2 ;;
            --pass-stdin) pass_stdin=true; shift ;;
            --pass-ref)
                [ $# -ge 2 ] || { _mail_err "usage" "--pass-ref requires env:<VAR> or an absolute file path"; exit 2; }
                pass_ref="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — see k2 mail link --help"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"
                else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    : "$ws_arg" "$level"  # (retained for arg-parser symmetry)
    case "$sub" in
        add)
            # O4: --provider gmail|microsoft = OAuth link (no host/password).
            # The daemon runs the flow server-side; we long-poll to done.
            if [ -n "$provider" ]; then
                case "$provider" in
                    gmail|microsoft) : ;;
                    *) _mail_err "usage" "--provider must be gmail or microsoft"; exit 2 ;;
                esac
                if [ -z "$address" ] || [ -z "$workspace" ]; then
                    _mail_err "usage" "link add --provider $provider requires <address> --workspace <ws>"
                    exit 2
                fi
                if [ -n "$host" ] || $pass_stdin || [ -n "$pass_ref" ]; then
                    _mail_err "usage" "--provider is OAuth — drop --host/--pass-stdin/--pass-ref (those are the generic-IMAP path)"
                    exit 2
                fi
                local M_VERB="link_oauth_add" M_JSON="$as_json" M_ADDRESS="$address" \
                      M_WORKSPACE="$workspace" M_PROVIDER="$provider" M_FEATURES="$features"
                _mail_py
                return
            fi
            if [ -n "$features" ]; then
                _mail_err "usage" "--features is for OAuth links (--provider gmail|microsoft)"
                exit 2
            fi
            if [ -z "$address" ] || [ -z "$host" ] || [ -z "$workspace" ]; then
                _mail_err "usage" "link add requires <address> --host <imap-host> --workspace <ws> (+ --pass-stdin | --pass-ref <ref>)  |  OAuth: --provider gmail|microsoft --workspace <ws>"
                exit 2
            fi
            local pass=""
            if $pass_stdin; then
                if [ -n "$pass_ref" ]; then
                    _mail_err "usage" "--pass-stdin and --pass-ref are mutually exclusive"
                    exit 2
                fi
                # The app-password arrives on stdin — NEVER argv (ps
                # would show it) and never a shell history line.
                IFS= read -r pass || true
                if [ -z "$pass" ]; then
                    _mail_err "usage" "--pass-stdin: no password on stdin (pipe it: printf %s \"\$PW\" | k2 mail link add …)"
                    exit 2
                fi
            elif [ -z "$pass_ref" ]; then
                _mail_err "usage" "link add needs the app-password: --pass-stdin (piped) or --pass-ref env:<VAR> / /abs/path"
                exit 2
            fi
            local M_VERB="external_add" M_JSON="$as_json" M_ADDRESS="$address" \
                  M_WORKSPACE="$workspace" M_HOST="$host" M_PORT="$port" M_TLS="$tls" \
                  M_USER="$user" M_NAME="$name" M_DRAFTS_FOLDER="$drafts_folder" \
                  M_SMTP_HOST="$smtp_host" M_SMTP_PORT="$smtp_port" M_SMTP_TLS="$smtp_tls" \
                  M_PASS="$pass" M_PASS_REF="$pass_ref"
            _mail_py ;;
        upgrade)
            # MCS1: incremental re-consent IN PLACE (same row, grants, ids).
            if [ -z "$address" ] || [ -z "$features" ]; then
                _mail_err "usage" "link upgrade requires <address> --features mail|calendar|mail,calendar"
                exit 2
            fi
            if [ -n "$host" ] || $pass_stdin || [ -n "$pass_ref" ] || [ -n "$workspace" ]; then
                _mail_err "usage" "link upgrade takes only <address> --features (the row keeps its workspace)"
                exit 2
            fi
            local M_VERB="link_oauth_upgrade" M_JSON="$as_json" M_ADDRESS="$address" \
                  M_PROVIDER="$provider" M_FEATURES="$features"
            _mail_py ;;
        calendars)
            # MCS3: show or set the per-server switch for calendars on
            # LINKED inboxes (owner verb; agents exit 3).
            case "$address" in
                ""|on|off) : ;;
                *) _mail_err "usage" "link calendars takes on or off (or nothing to show the switch)"; exit 2 ;;
            esac
            if [ -n "$provider" ] || [ -n "$features" ] || [ -n "$host" ] || [ -n "$workspace" ] \
                || $pass_stdin || [ -n "$pass_ref" ]; then
                _mail_err "usage" "link calendars takes only [on|off] [--json]"
                exit 2
            fi
            local M_VERB="link_calendars" M_JSON="$as_json" M_LINK_CAL="$address"
            _mail_py ;;
        list)
            local M_VERB="link_list" M_JSON="$as_json"
            _mail_py ;;
        remove)
            if [ -z "$address" ]; then
                _mail_err "usage" "link remove requires the <address> ('k2 mail link list')"
                exit 2
            fi
            local M_VERB="external_remove" M_JSON="$as_json" M_ADDRESS="$address"
            _mail_py ;;
    esac
}

# MCS1: `k2 mail capabilities [<address>]` — what each inbox you can see
# can do (mail read/draft/compose/send, calendar read/write/invite/rsvp,
# free/busy reach) and, for anything off, why + the fix. Agents see their
# own grants; the owner sees every inbox.
cmd_mail_capabilities() {
    if _wants_help "$@"; then cmd_help_mail_capabilities; exit 0; fi
    local address="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _mail_err "usage" "unknown flag '$1' — usage: k2 mail capabilities [<address>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$address" ]; then address="$1"; shift
                else
                    _mail_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi ;;
        esac
    done
    local M_VERB="capabilities" M_JSON="$as_json" M_ADDRESS="$address"
    _mail_py
}

# MCS1: `k2 mail oauth-config show|set|clear` — the owner's own OAuth app
# per provider (BYO) + the Microsoft tenant. OWNER verb (agents exit 3).
# The Gmail client secret rides stdin only (--secret-stdin), never argv.
cmd_mail_oauth_config() {
    if _wants_help "$@"; then cmd_help_mail_oauth_config; exit 0; fi
    local sub=""
    case "${1:-}" in
        show|set|clear) sub="$1"; shift ;;
        ""|-*) sub="show" ;;
        *)
            _mail_err "usage" "unknown oauth-config command '${1:-}' — usage: k2 mail oauth-config <show|set|clear>"
            exit 2 ;;
    esac
    local provider="" client_id="" secret_stdin=false tenant="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --provider)
                [ $# -ge 2 ] || { _mail_err "usage" "--provider requires gmail or microsoft"; exit 2; }
                provider="$2"; shift 2 ;;
            --client-id)
                [ $# -ge 2 ] || { _mail_err "usage" "--client-id requires the app's client (application) id"; exit 2; }
                client_id="$2"; shift 2 ;;
            --tenant)
                [ $# -ge 2 ] || { _mail_err "usage" "--tenant requires common|organizations|consumers|<tenant id or domain>"; exit 2; }
                tenant="$2"; shift 2 ;;
            --secret-stdin) secret_stdin=true; shift ;;
            --json) as_json=true; shift ;;
            *)
                _mail_err "usage" "unexpected argument '$1' — see k2 mail oauth-config --help"
                exit 2 ;;
        esac
    done
    case "$provider" in
        ""|gmail|microsoft) : ;;
        *) _mail_err "usage" "--provider must be gmail or microsoft"; exit 2 ;;
    esac
    local secret=""
    case "$sub" in
        set)
            [ -n "$provider" ] || { _mail_err "usage" "oauth-config set requires --provider gmail|microsoft"; exit 2; }
            if [ -z "$client_id" ] && [ -z "$tenant" ]; then
                _mail_err "usage" "oauth-config set requires --client-id (Microsoft may pass just --tenant)"
                exit 2
            fi
            if $secret_stdin; then
                [ "$provider" = "gmail" ] || { _mail_err "usage" "--secret-stdin is for Gmail only (Microsoft is a public client)"; exit 2; }
                IFS= read -r secret || true
                [ -n "$secret" ] || { _mail_err "usage" "--secret-stdin: no secret on stdin (pipe it in)"; exit 2; }
            fi
            ;;
        clear)
            [ -n "$provider" ] || { _mail_err "usage" "oauth-config clear requires --provider gmail|microsoft"; exit 2; }
            ;;
    esac
    local M_VERB="oauth_config_$sub" M_JSON="$as_json" M_PROVIDER="$provider" \
          M_CLIENT_ID="$client_id" M_TENANT="$tenant" M_OAUTH_SECRET="$secret"
    _mail_py
}

