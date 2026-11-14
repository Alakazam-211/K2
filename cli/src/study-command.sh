cmd_study() {
    local as_json=0 topic=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --help|-h)
                _study_usage_all
                exit 0
                ;;
            --json)
                as_json=1
                shift
                ;;
            --*)
                echo "Error: unknown flag '$1'" >&2
                _study_usage_all >&2
                exit 2
                ;;
            *)
                if [ -n "$topic" ]; then
                    echo "Error: unexpected extra argument '$1'" >&2
                    _study_usage_all >&2
                    exit 2
                fi
                topic="$1"
                shift
                ;;
        esac
    done

    if [ -z "$topic" ] || [ "$topic" = "list" ]; then
        if [ "$as_json" = 1 ]; then
            for id in $(_study_topic_ids_all); do
                printf '%s\t%s\n' "$id" "$(_study_title "$id")"
            done | python3 -c '
import json, sys
topics = []
for line in sys.stdin:
    line = line.rstrip("\n")
    tid, title = line.split("\t", 1)
    topics.append({"id": tid, "title": title})
print(json.dumps({"topics": topics}, separators=(",", ":")))
'
        else
            _study_catalog_all
        fi
        exit 0
    fi

    # What people call heartbeats, so a search lands on the right page.
    case "$topic" in routines|routine|reminders|reminder|cron) topic="heartbeats" ;; esac
    if ! _study_is_topic "$topic" && ! { [ "$topic" = permissions ] && _permission_cli_enabled; }; then
        echo "Error: unknown study topic '$topic'" >&2
        _study_usage_all >&2
        exit 2
    fi

    if [ "$as_json" = 1 ]; then
        _study_page "$topic" | python3 -c '
import json, sys
body = sys.stdin.read()
if body.endswith("\n"):
    body = body[:-1]
print(json.dumps({"id": sys.argv[1], "body": body}, separators=(",", ":")))
' "$topic"
    else
        _study_page "$topic"
    fi
}

# 0.39.0f Phase 2.1b: cmd_help_general / cmd_help_advanced (the old
# pre-2.1a help screens) removed. The 3-tier help system
# (cmd_help_v2_daily / cmd_help_v2_advanced / cmd_help_internal +
# cmd_help_msg / cmd_help_inbox) owns all help text now.

