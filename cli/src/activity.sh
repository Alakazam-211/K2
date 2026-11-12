# ── Phase 2.1: Renames (feed→activity, roster→who) ────────────────────

cmd_activity() {
    # 0.39.0f Phase 2.1: feed → activity (PRD A5).
    # Same endpoint; renamed at the CLI layer.
    cmd_feed "$@"
}

cmd_help_activity() {
    cat <<'EOF'
k2 activity [<limit>] [--limit N] [--agent <name>] [--json]

The cross-workspace activity feed, newest last: date and time, agent,
event type, route and summary.

  <limit> / --limit N   how many events (default 20)
  --agent <name>        only this agent's events
  --json                the daemon's JSON as is

Exit: 0 ok · 1 the daemon answered with an error or nothing usable · 2 bad usage
EOF
}

