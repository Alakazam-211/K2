# ── k2 checkin: retired 0.46.11 ──────────────────────────────────────

# `k2 checkin [--status "msg"] [--done]` no longer runs: it points at the
# verbs that replace it, on stderr, and exits 2.
_checkin_retired() {
    local form="checkin"
    case " $* " in
        *" --done "*) form="checkin --done" ;;
        *" --status "*) form="checkin --status" ;;
    esac
    {
        echo "Error: 'k2 $form' is retired. Use instead:"
        case "$form" in
            "checkin --done") echo "  finish your task: k2 done [--blocked \"reason\"]" ;;
            "checkin --status") echo "  send status to your head: k2 organization update \"<news>\"" ;;
            *)
                echo "  bulletin and status: k2 organization bulletin / k2 organization update \"<news>\""
                echo "  people: k2 thread    work: k2 tickets, k2 inbox    finishing: k2 done"
                ;;
        esac
    } >&2
    exit 2
}

