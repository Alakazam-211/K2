# ── Phase 2.1: Inbox verbs (A22) ──────────────────────────────────────

# Pretty-print a list of inbox items received as JSON on stdin.
format_inbox() {
    python3 -c "
import json, sys
try:
    data = json.loads(sys.stdin.read() or '[]')
except Exception as e:
    sys.stderr.write('failed to parse inbox JSON: %s\n' % e)
    sys.exit(1)
if not data:
    print('No items.')
    sys.exit(0)
for item in data:
    folder = item.get('folder', '') or '(top)'
    priority = item.get('priority', 'normal')
    title = item.get('title') or item.get('filename', '')
    iid = item.get('id', '?')
    prio_mark = {'high': '!', 'critical': '!!', 'normal': ' ', 'low': '.'}
    mark = prio_mark.get(priority, ' ')
    print(f'  {iid:<24} [{folder:<10}] {mark} {title}')
    if item.get('summary'):
        print(f'  {\"\":<24}   summary: {item[\"summary\"]}')
" 2>/dev/null || cat
}

cmd_inbox_list() {
    local folder=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --folder) folder="$2"; shift 2 ;;
            -*) shift ;;
            *) [[ -z "$folder" ]] && folder="$1"; shift ;;
        esac
    done
    local params=""
    [[ -n "$folder" ]] && params="folder=$(urlencode "$folder")"
    if [[ -n "$params" ]]; then
        cli_send cli_request "/cli/inbox/list" "$params" | format_inbox
    else
        cli_send cli_request "/cli/inbox/list" | format_inbox
    fi
}

cmd_inbox_read() {
    local id="${1:-}"
    if [[ -z "$id" ]]; then
        echo "Usage: k2 inbox read <id>" >&2
        exit 2
    fi
    # 0.46.1: the error used to be hidden (2>/dev/null): a bad id printed
    # nothing, exit 1. Not found exits 4.
    cli_send cli_request "/cli/inbox/read" "id=$(urlencode "$id")" | python3 -c "
import json, sys
raw = sys.stdin.read()
if not raw.strip():
    sys.stderr.write('Error: no answer from the K2 daemon (neither this session\'s cell socket nor the loopback port answered). Is K2 running? Try again: k2 inbox read <id>\n')
    sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    sys.stderr.write('Error: the K2 daemon gave no usable answer: %r\n' % raw[:200])
    sys.exit(1)
err = d.get('error') if isinstance(d, dict) else 'unexpected answer'
if err is not None:
    hint = (err.get('hint') or err.get('code') or str(err)) if isinstance(err, dict) else str(err)
    sys.stderr.write('Error: %s\n' % hint)
    sys.exit(4 if 'not found' in hint.lower() or (isinstance(err, dict) and err.get('code') == 'not_found') else 1)
print(d.get('content', ''))
"
}

cmd_inbox_compose() {
    local title="" body="" priority="" source="" from="" summary=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --summary) summary="$2"; shift 2 ;;
            --title) title="$2"; shift 2 ;;
            --body) body="$2"; shift 2 ;;
            --priority) priority="$2"; shift 2 ;;
            --source) source="$2"; shift 2 ;;
            --from) from="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    if [[ -z "$title" ]]; then
        echo "Usage: k2 inbox compose --title \"...\" [--body \"...\"] [--priority H|N|L]" >&2
        exit 1
    fi
    local params="title=$(urlencode "$title")"
    [[ -n "$body" ]] && params="${params}&body=$(urlencode "$body")"
    [[ -n "$priority" ]] && params="${params}&priority=$(urlencode "$priority")"
    [[ -n "$source" ]] && params="${params}&source=$(urlencode "$source")"
    [[ -n "$from" ]] && params="${params}&from=$(urlencode "$from")"
    [[ -n "$summary" ]] && params="${params}&summary=$(urlencode "$summary")"
    # 0.39.45 (#35/#37): real POST body. The old `curl -sG -X POST` put
    # everything on the URL, where the daemon's request-head cap silently
    # clipped bodies at ~2.7KB — corrupting the durable inbox record.
    cli_send cli_post_form "/cli/inbox/compose" "$params"
    echo
}

cmd_inbox_respond() {
    local id="${1:-}" text="${2:-}"
    if [[ -z "$id" || -z "$text" ]]; then
        echo "Usage: k2 inbox respond <id> \"text\"" >&2
        exit 1
    fi
    # #58 Phase-1 close: scoped UDS arm (daemon stamps identity + own-
    # workspace path); transparent TCP+owner fallback. The mutating params
    # ride the form body, which the daemon's inbox POST arm merges with the
    # query — same contract as the prior raw `curl -sG -X POST`.
    cli_send cli_post_form "/cli/inbox/respond" \
        "id=$(urlencode "$id")" "text=$(urlencode "$text")"
    echo
}

cmd_inbox_move() {
    local id="${1:-}" folder="${2:-}"
    if [[ -z "$id" || -z "$folder" ]]; then
        echo "Usage: k2 inbox move <id> <folder>" >&2
        exit 1
    fi
    cli_send cli_post_form "/cli/inbox/move" \
        "id=$(urlencode "$id")" "folder=$(urlencode "$folder")"
    echo
}

cmd_inbox_archive() {
    local id="${1:-}"
    if [[ -z "$id" ]]; then
        echo "Usage: k2 inbox archive <id>" >&2
        exit 1
    fi
    cli_send cli_post_form "/cli/inbox/archive" "id=$(urlencode "$id")"
    echo
}

cmd_inbox_delete() {
    local id="${1:-}"
    if [[ -z "$id" ]]; then
        echo "Usage: k2 inbox delete <id>" >&2
        exit 1
    fi
    cli_send cli_post_form "/cli/inbox/delete" "id=$(urlencode "$id")"
    echo
}

cmd_inbox_search() {
    local q="${1:-}"
    if [[ -z "$q" ]]; then
        echo "Usage: k2 inbox search \"query\"" >&2
        exit 1
    fi
    cli_send cli_request "/cli/inbox/search" "q=$(urlencode "$q")" | format_inbox
}

cmd_inbox_folders() {
    cli_send cli_request "/cli/inbox/folders" | python3 -c "
import json, sys
try:
    folders = json.loads(sys.stdin.read() or '[]')
except Exception:
    folders = []
if not folders:
    print('No folders yet. Create one by moving an item:')
    print('  k2 inbox move <id> projects')
else:
    for f in folders:
        print('  ' + f)
" 2>/dev/null
}

cmd_inbox() {
    # `k2 inbox`            → list top-level (default subcommand)
    # `k2 inbox <sub>`      → dispatch to subcommand
    if [ $# -eq 0 ]; then
        cmd_inbox_list
        return
    fi
    local sub="$1"
    case "$sub" in
        list)     shift; cmd_inbox_list "$@" ;;
        read)     shift; cmd_inbox_read "$@" ;;
        compose)  shift; cmd_inbox_compose "$@" ;;
        respond)  shift; cmd_inbox_respond "$@" ;;
        move)     shift; cmd_inbox_move "$@" ;;
        archive)  shift; cmd_inbox_archive "$@" ;;
        delete)   shift; cmd_inbox_delete "$@" ;;
        search)   shift; cmd_inbox_search "$@" ;;
        folders)  cmd_inbox_folders ;;
        --help|-h) cmd_help_inbox ;;
        -*|*)
            # If it looks like a flag, treat as `inbox list <flags>`.
            cmd_inbox_list "$@"
            ;;
    esac
}

