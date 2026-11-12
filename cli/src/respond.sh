# ── F2 (sandbox cell + HOST session) — k2 respond ────────────────────
# `k2 respond [--final] <message>` — the in-session agent's RESPONSE
# egress. Posts a curated line to THIS session's response log; the
# external API caller drains it via
# `GET /v1/sandboxes/<id>/messages` (cells) or
# `GET /v1/w/<ws>/host-sessions/<id>/messages` (host sessions). The
# agent itself decides how many times to respond and when (`--final`
# marks the turn's last line) — multi-emit, like a person narrating.
#
# Transport (in preference order):
#   1. Per-cell UDS ($K2_HOOK_SOCK) + scoped Bearer — sandbox cells and
#      scoped host sessions where the socket bound.
#   2. Loopback TCP `/cli/respond` + scoped Bearer — HOST sessions
#      (prd-v1-api-completion §4). The daemon accepts ONLY a scoped
#      per-session token here (session identity comes from the token),
#      so this needs K2_HOOK_SCOPED on the daemon; the owner token is
#      REFUSED by design and we never fall back to it (it names no
#      session — a scoped token is structurally `<sid>.<secret>`).
# Outside any scoped session this errors loudly.
cmd_respond() {
    local final="0"
    while [ $# -gt 0 ]; do
        case "$1" in
            --final) final="1"; shift ;;
            --help|-h)
                echo "Usage: k2 respond [--final] <message>"
                echo "  Send a response line from inside an API-launched session to the API caller."
                echo "  Call it as many times as you like as you work; --final marks the last."
                echo "  Lifecycle-only (no product message): use  k2 done  instead of empty --final."
                return 0 ;;
            --) shift; break ;;
            *) break ;;
        esac
    done
    local message="$*"
    if [ -z "$message" ]; then
        echo "Usage: k2 respond [--final] <message>" >&2
        return 1
    fi
    local sock; sock="$(_hook_sock)"
    if [ -n "$sock" ] && [ -S "$sock" ]; then
        cli_post_form_uds "/cli/respond" "message=$(urlencode "$message")" "final=${final}"
        return $?
    fi
    # HOST session (no per-cell socket): loopback TCP with the SCOPED
    # token as Bearer. Only a scoped token (shape `<sid>.<secret>`)
    # carries session identity — with none present, fail loudly.
    if [[ "$SCOPED_TOKEN" == *.* ]] && [ -n "$PORT" ]; then
        local _K2_CFG=""
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        _k2_cfg_add data-raw "message=$(urlencode "$message")&final=${final}"
        _k2_curl "$_K2_CFG" -s -X POST "${BASE_URL}/cli/respond" \
            --connect-timeout 5 --max-time 30 \
            -H "Content-Type: application/x-www-form-urlencoded" 2>/dev/null
        return $?
    fi
    echo "Error: 'k2 respond' needs an API-launched session (per-cell socket or scoped session token)." >&2
    return 1
}

