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

# Postal addresses (prd-postal-in-k2-v1 S0): `handle::label.postal.bot` is
# an agent on the Postal network, not a K2 server. Sending Postal mail from
# here comes with native Postal; until then any `k2 msg` form refuses it
# with a teaching line (exit 2) instead of failing as an unknown peer.
_is_postal_address() {
    local a
    a="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
    [[ "$a" =~ ^[a-z0-9][a-z0-9_-]*::[a-z0-9][a-z0-9-]{1,61}[a-z0-9]\.postal\.bot$ ]]
}

# _postal_send_teach <address>: exit 2 with the teaching line when the
# address is a Postal one; return 0 otherwise.
_postal_send_teach() {
    _is_postal_address "$1" || return 0
    local me="${_K2_CALLED_AS:-k2}"
    echo "$me msg: '$1' is a Postal address (an agent on the Postal network, not a server paired with this one)." >&2
    echo "  $me can't send Postal mail yet; that comes with native Postal." >&2
    echo "  Until then, send with p5 if it is set up on this machine: p5 msg $1 \"text\"" >&2
    echo "  $me study postal says more." >&2
    exit 2
}

# `_k2_addr_note <old> <new> [renamed]` (defined once near the top of this
# file) prints the shared `note:` line (TR17): "is" for a second name (A7),
# "is now" for a rename.

