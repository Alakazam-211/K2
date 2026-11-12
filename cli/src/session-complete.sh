# ── Host-session / sandbox completion — k2 done (API lifecycle) ───────
# Lifecycle-only complete: arms reaper Grace without appending a product
# message to the respond drain ring. Same transport as `k2 respond`
# (UDS then scoped Bearer). Discriminator is K2_API_CELL (D6) — NOT sock
# or scoped-token presence (workspace agents under COMPAT-58 also have
# those and must keep the plain `/cli/done` meaning).
_is_api_lifecycle_context() {
    case "${K2_API_CELL:-${K2SO_API_CELL:-}}" in
        1|true|yes|YES|True) return 0 ;;
    esac
    return 1
}

cmd_session_complete() {
    local reason=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --reason) reason="$2"; shift 2 ;;
            --help|-h)
                echo "Usage: k2 done [--reason <text>]"
                echo "  Mark this API-launched session complete (enter grace for reap)."
                echo "  Does NOT post a product message to the respond drain."
                echo "  Prefer this over empty 'k2 respond --final \"\"' when you have no answer text."
                echo "  Optional --reason is logged only (not shown to the API caller)."
                return 0 ;;
            --) shift; break ;;
            *) break ;;
        esac
    done
    local form=""
    if [ -n "$reason" ]; then
        form="reason=$(urlencode "$reason")"
    fi
    local sock; sock="$(_hook_sock)"
    if [ -n "$sock" ] && [ -S "$sock" ]; then
        if [ -n "$form" ]; then
            cli_post_form_uds "/cli/session/complete" "$form"
        else
            cli_post_form_uds "/cli/session/complete"
        fi
        return $?
    fi
    if [[ "$SCOPED_TOKEN" == *.* ]] && [ -n "$PORT" ]; then
        # Empty $form → empty body, exactly as before.
        local _K2_CFG=""
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        _k2_cfg_add data-raw "$form"
        _k2_curl "$_K2_CFG" -s -X POST "${BASE_URL}/cli/session/complete" \
            --connect-timeout 5 --max-time 30 \
            -H "Content-Type: application/x-www-form-urlencoded" 2>/dev/null
        return $?
    fi
    echo "Error: 'k2 done' (session complete) needs an API-launched session (per-cell socket or scoped session token)." >&2
    return 1
}

