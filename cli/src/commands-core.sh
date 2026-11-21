# ── Commands ─────────────────────────────────────────────────────────

cmd_agent_update() {
    # Help wins: -h or --help anywhere prints the agent help and updates
    # nothing (the loop skipped it, and a flag took it as its value).
    if _wants_help "$@"; then cmd_help_agent; return 0; fi
    local name="" field="" value=""
    local usage='Usage: k2 agent update --name <agent> --field <field> --value "..."'
    while [ $# -gt 0 ]; do
        case "$1" in
            --name|--field|--value)
                if [ $# -lt 2 ]; then
                    echo "Error: $1 needs a value" >&2
                    echo "$usage" >&2
                    exit 2
                fi
                case "$1" in
                    --name) name="$2" ;;
                    --field) field="$2" ;;
                    *) value="$2" ;;
                esac
                shift 2 ;;
            *) [ -z "$name" ] && name="$1"; shift ;;
        esac
    done
    if [ -z "$name" ] || [ -z "$field" ] || [ -z "$value" ]; then
        echo "$usage" >&2
        echo "  field: frontmatter key (role, type) or section name (Work Sources, Capabilities)" >&2
        exit 2
    fi
    # Through cli_send, as `k2 workspace update` (_ws_request): an error is
    # an error, on stderr with its exit and closing line.
    _ws_request cli_request_post "/cli/agent/update" \
        "agent=$(urlencode "$name")" \
        "field=$(urlencode "$field")" \
        "value=$(urlencode "$value")"
    cli_print_or_fail "$_WS_ANSWER" /cli/agent/update >/dev/null
    if ! printf '%s' "$_WS_ANSWER" | python3 -c 'import json, sys; sys.exit(0 if json.load(sys.stdin).get("success") is True else 1)' 2>/dev/null; then
        echo "Error: K2 did not say it updated $field for agent $name: ${_WS_ANSWER:0:200}" >&2
        exit 1
    fi
    echo "Updated $field for agent $name"
}

# 0.40.31: cmd_heartbeat_noop / cmd_heartbeat_action removed with the
# legacy per-agent adaptive-backoff heartbeat API. The dispatch arms in
# cmd_heartbeat_toggle route noop/action/force/set/get to
# fail_deprecated; the daemon routes return HTTP 410.

cmd_terminal_spawn() {
    local usage="Usage: k2 terminal spawn --command \"...\" [--title \"...\"] [--agent <name>] [--wait] [--sandbox|--no-sandbox]"
    # -h/--help anywhere prints the usage and spawns nothing (0.46.17).
    if _wants_help "$@"; then echo "$usage"; return 0; fi
    local agent="" command="" title="" wait="" cwd="" sandbox=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --title) _need_value "$@"; title="$2"; shift 2 ;;
            --command) _need_value "$@"; command="$2"; shift 2 ;;
            --agent) _need_value "$@"; agent="$2"; shift 2 ;;
            --cwd) _need_value "$@"; cwd="$2"; shift 2 ;;
            --wait) wait="1"; shift ;;
            # Sandbox P1 (accept-and-mark): request/decline a sandbox. In P1
            # this resolves to the host-direct "passthrough" backend (no real
            # isolation yet) and is never rejected. Default: omit the param
            # entirely so the spawn body is byte-identical to pre-seam.
            --sandbox) sandbox="true"; shift ;;
            --no-sandbox) sandbox="false"; shift ;;
            *) shift ;;
        esac
    done
    if [ -z "$command" ]; then
        echo "$usage" >&2
        exit 1
    fi
    local params="command=$(urlencode "$command")"
    [ -n "$title" ] && params="${params}&title=$(urlencode "$title")"
    [ -n "$agent" ] && params="${params}&agent=$(urlencode "$agent")"
    [ -n "$cwd" ] && params="${params}&cwd=$(urlencode "$cwd")"
    [ -n "$wait" ] && params="${params}&wait=1"
    [ -n "$sandbox" ] && params="${params}&sandbox=${sandbox}"
    local resp
    resp=$(cli_request_post "/cli/terminal/spawn" "$params") || true
    if [ -z "$resp" ]; then
        echo "Error: terminal spawn returned no body" >&2
        exit 1
    fi
    cli_print_or_fail "$resp"
    echo "Terminal spawned: ${title:-$command}"
}

cmd_commit() {
    local message=""
    while [ $# -gt 0 ]; do
        case "$1" in
            -m|--message) _need_value "$@"; message="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    local params=""
    [ -n "$message" ] && params="message=$(urlencode "$message")"
    echo "Requesting AI Commit..."
    cli_request_post "/cli/commit" "$params"
}

cmd_commit_merge() {
    local message=""
    while [ $# -gt 0 ]; do
        case "$1" in
            -m|--message) _need_value "$@"; message="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    local params=""
    [ -n "$message" ] && params="message=$(urlencode "$message")"
    echo "Requesting AI Commit & Merge..."
    cli_request_post "/cli/commit-merge" "$params"
}

# 0.39.0f Phase 2.1b: cmd_heartbeat orphan removed. 0.40.41: bare
# `k2 heartbeat` now lists active schedules at dispatch; the `heartbeat`
# fallback (cmd_heartbeat_toggle) only maps retired verbs to their
# replacements and rejects unknowns with exit 2.

