# ── Phase 2.1: help text helpers ──────────────────────────────────────

cmd_help_inbox() {
    cat <<'EOF'
%include cli/text/help/inbox.txt
EOF
}

cmd_help_workspace() {
    cat <<'EOF'
%include cli/text/help/workspace.txt
EOF
}

cmd_help_connections() {
    cat <<'EOF'
%include cli/text/help/connections.txt
EOF
}

