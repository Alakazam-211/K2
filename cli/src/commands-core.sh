# ── Commands ─────────────────────────────────────────────────────────

cmd_agent_update() {
    local name="" field="" value=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --name) name="$2"; shift 2 ;;
            --field) field="$2"; shift 2 ;;
            --value) value="$2"; shift 2 ;;
            *) [ -z "$name" ] && name="$1"; shift ;;
        esac
    done
    if [ -z "$name" ] || [ -z "$field" ] || [ -z "$value" ]; then
        echo "Usage: k2 agent update --name <agent> --field <field> --value \"...\"" >&2
        echo "  field: frontmatter key (role, type) or section name (Work Sources, Capabilities)" >&2
        exit 1
    fi
    cli_request_post "/cli/agent/update" \
        "agent=$(urlencode "$name")" \
        "field=$(urlencode "$field")" \
        "value=$(urlencode "$value")" \
        | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    if d.get('success'):
        print(f'Updated {\"$field\"} for agent {\"$name\"}')
    else:
        print(f'Error: {d}', file=sys.stderr)
except:
    print('Error: failed to parse response', file=sys.stderr)
" 2>/dev/null
}

# 0.40.31: cmd_heartbeat_noop / cmd_heartbeat_action removed with the
# legacy per-agent adaptive-backoff heartbeat API. The dispatch arms in
# cmd_heartbeat_toggle route noop/action/force/set/get to
# fail_deprecated; the daemon routes return HTTP 410.

cmd_terminal_spawn() {
    local agent="" command="" title="" wait="" cwd="" sandbox=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --title) title="$2"; shift 2 ;;
            --command) command="$2"; shift 2 ;;
            --agent) agent="$2"; shift 2 ;;
            --cwd) cwd="$2"; shift 2 ;;
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
        echo "Usage: k2 terminal spawn --command \"...\" [--title \"...\"] [--agent <name>] [--wait] [--sandbox|--no-sandbox]" >&2
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
            -m|--message) message="$2"; shift 2 ;;
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
            -m|--message) message="$2"; shift 2 ;;
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

