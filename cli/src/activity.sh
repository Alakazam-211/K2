# ── Phase 2.1: Renames (feed→activity, roster→who) ────────────────────

cmd_activity() {
    # 0.39.0f Phase 2.1: feed → activity (PRD A5).
    # Same endpoint; renamed at the CLI layer.
    cmd_feed "$@"
}

cmd_help_activity() {
    cat <<'EOF'
%include cli/text/help/activity.txt
EOF
}

