# ── Phase 2.1: 3-tier help (verbatim mock text) ───────────────────────

cmd_help_v2_daily() {
    cat <<'EOF'
%include cli/text/help/_daily.txt
EOF
    # 0.46.17: not in an agent's help while agents may not report
    # (_sf_agents_off_here). Everyone else: today's text.
    if ! _sf_agents_off_here; then
        cat <<'EOF'
%include cli/text/help/_daily-feedback.txt
EOF
    fi
    # Permission cards (Beta): one line, only while both switches are on
    # for this agent (PRD rev 3.1 §11.4). Off = today's text.
    if _permission_cli_enabled; then
        echo '  permission request <action> --why "…"   Ask a person to allow what K2 refused you (k2 study permissions)'
    fi
    cat <<'EOF'
%include cli/text/help/_daily-end.txt
EOF
}

cmd_help_v2_advanced() {
    cat <<'EOF'
%include cli/text/help/_advanced.txt
EOF
}

