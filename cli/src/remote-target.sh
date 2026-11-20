# ── Remote target addressing (K2-as-a-server, 0.40.x) ────────────────
# `k2 talk <ws>@<subdomain>` / `k2 read <ws>@<host>` address a workspace
# on a CONNECTED remote daemon over K2 Connect — the SAME model as local
# workspace<->workspace agent talk, just pointed at a remote base URL.
# We split off the @host, repoint BASE_URL at https://<host>, and swap
# TOKEN for that host's owner token. Every existing cli_request /
# cli_post_* call then routes to the remote daemon with NO call-site
# change: the remote daemon resolves the workspace name against ITS own
# projects (canonical session by default; --session <id> targets a
# specific one). No daemon change — all routes used already exist and are
# reachable over the tunnel with the owner token.
_REMOTE_HOST=""
_BARE_TARGET=""
# PR2 addressing flags (set by _split_remote_target):
#   _IS_WIRE_FORM=1   → 3-part fp::ws::agent (fed send only)
#   _USED_LEGACY_AT=1 → peeled via legacy agent@host
_IS_WIRE_FORM=0
_USED_LEGACY_AT=0
# One-shot stderr hint when agents use @ federated form (prefer ::).
_FED_AT_HINT_SHOWN=0
# Split a remote user address into globals:
#   _BARE_TARGET  — agent/workspace token (host stripped)
#   _REMOTE_HOST  — host, or "" when local
#   _IS_WIRE_FORM / _USED_LEGACY_AT — see above
# Callers MUST invoke it directly (NOT in a "$(...)" subshell).
#
# Grammar (PR2 / GH #47):
#   agent::host.k2.dev  → user remote (2-part ::, host looks like host)
#   agent@host          → legacy user remote
#   fp::ws::agent       → wire form (3-part; not peeled as user remote)
#   bare name           → local
_split_remote_target() {
    local raw="$1"
    _REMOTE_HOST=""
    _BARE_TARGET="$raw"
    _IS_WIRE_FORM=0
    _USED_LEGACY_AT=0

    # 3+ part wire form: at least two '::' separators.
    if [[ "$raw" == *::*::* ]]; then
        _IS_WIRE_FORM=1
        return
    fi

    # 2-part user form agent::host (RHS must look like a host).
    if [[ "$raw" == *::* ]]; then
        local left="${raw%%::*}"
        local right="${raw#*::}"
        if [ -n "$left" ] && [ -n "$right" ] && { [[ "$right" == *.* ]] || [[ "$right" == *.k2.dev ]] || [[ "$right" == *.K2.DEV ]]; }; then
            _BARE_TARGET="$left"
            _REMOTE_HOST="$right"
            return
        fi
        # Not host-like → leave as bare local token.
        return
    fi

    # Legacy agent@host (split on LAST '@').
    case "$raw" in
        *@*)
            _REMOTE_HOST="${raw##*@}"
            _BARE_TARGET="${raw%@*}"
            _USED_LEGACY_AT=1
            ;;
    esac
}

# Postal addresses (prd-postal-in-k2-v1 S5): `handle::label.postal.bot` is
# an agent on the Postal network, not a K2 server. `k2 msg <address> "text"`
# sends Postal mail from this workspace's own address (the daemon takes the
# sender from the credential), to a friend only.
_is_postal_address() {
    local a
    a="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
    [[ "$a" =~ ^[a-z0-9][a-z0-9_-]*::[a-z0-9][a-z0-9-]{1,61}[a-z0-9]\.postal\.bot$ ]]
}

# _postal_send_teach <address> [args…]: when the address is a Postal one,
# send the message (`k2 msg <address> "text" [--no-wake] [--json]`) and
# exit with its code; return 0 otherwise. K2-only flags are refused.
_postal_send_teach() {
    _is_postal_address "$1" || return 0
    local to="$1" text="" wake="__true" as_json=false
    shift
    while [ $# -gt 0 ]; do
        case "$1" in
            --no-wake) wake="__false"; shift ;;
            --wake) shift ;;
            --json) as_json=true; shift ;;
            --summary|--wake-timeout|--session|--inbox|--inbox-wake|--inbox-silent|--signal|--command|--from|--via)
                echo "k2 msg: $1 is for messages to agents on this server; Postal mail to $to takes only the text, --no-wake and --json." >&2
                exit 2 ;;
            -*) echo "k2 msg: unknown flag '$1' for Postal mail (k2 msg --help)" >&2; exit 2 ;;
            *) if [ -z "$text" ]; then text="$1"; else text="$text $1"; fi; shift ;;
        esac
    done
    if [ -z "$text" ]; then
        echo "Usage: k2 msg $to \"text\" [--no-wake] [--json]" >&2
        exit 2
    fi
    # In a session: the passport, never the owner token (k2 postal send).
    _postal_session_token
    local body
    body="$(_postal_body "workspace=${PROJECT:-$PWD}" "to=$to" "text=$text" "wake=$wake")"
    _K2_PIPE_MARK=1 cli_send cli_post_json "/cli/postal/send" "$body" | _postal_print send "$as_json"
    exit "${PIPESTATUS[1]}"
}

# `_k2_addr_note <old> <new> [renamed]` (defined once near the top of this
# file) prints the shared `note:` line (TR17): "is" for a second name (A7),
# "is now" for a rename.

