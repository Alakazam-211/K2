# ── Phase 2.1: Deprecation helpers ────────────────────────────────────

# Print a one-line deprecation warning. Use for soft-deprecation
# (old verb still works; forward to the new form).
warn_deprecated() {
    local old_verb="$1"
    local new_verb="$2"
    echo "warning: '$old_verb' is deprecated; use '$new_verb' instead." >&2
}

# BEGIN_RETIRED_VERBS
# Retired verbs the CLI no longer runs. Each row is "old verb|pointer".
# The top-level and `agent` dispatchers look a verb up here before they
# say "Unknown command", so old AGENT.md / SKILL.md text still gets a
# pointer, and `k2 help-deprecated` prints its map from this table.
# tests/cli/retired_verbs_table.sh runs every row.
_K2_RETIRED_VERBS=(
    "agents create|k2 skills create <name> --role \"...\""
    "agents running|k2 workspace list --running"
    "agents list|k2 workspace list  (or k2 skills list)"
    "agents work|k2 inbox"
    "agents status|k2 workspace list"
    "agents delete|k2 skills remove <name>"
    "agents remove|k2 skills remove <name>"
    "agents launch|your harness's own session start (K2 no longer launches agents)"
    "agents generate-md|k2 skills regenerate [<name>]"
    "agents profile|k2 skills profile <name>"
    "agents lock|(nothing replaces it)"
    "agents unlock|(nothing replaces it)"
    "agents triage|k2 workspace triage"
    "agents reap|(nothing replaces it)"
    "agent create|k2 skills create <name> --role \"...\""
    "agent delete|k2 agent retire <name>"
    "agent remove|k2 agent retire <name>"
    "work create|k2 inbox compose --title \"...\" --body \"...\""
    "work send|k2 msg <workspace> --inbox-silent <file>  (or --inbox-wake)"
    "work move|k2 agent complete --agent <name> --file <file>"
    "work inbox|k2 inbox"
    "work done|k2 inbox archive <id>"
    "delegate|your harness's sub-agent feature, with k2 skills profile <agent>"
    "mode|k2 settings --mode custom  (or off|manager|k2)"
    "who|k2 connections list"
    "roster|k2 connections list"
    "whatsnew|k2 whats-new"
    "commit-merge|k2 commit --merge"
    "feed|k2 activity"
    "signal|k2 msg <workspace> --signal <kind> [--payload <json>]"
    "companion|k2 users pair-phone"
    "agentic|(agentic mode is always on)"
)

# `_retired_verb_exit "<verb> [<sub>]"`: when the words name a retired
# verb, print its pointer on one line and exit 1 (the old stubs' code).
# Otherwise return, and the caller says "Unknown ...".
_retired_verb_exit() {
    local key="$1" row new
    for row in "${_K2_RETIRED_VERBS[@]}"; do
        [ "${row%%|*}" = "$key" ] || continue
        new="${row#*|}"
        case "$new" in
            "("*) echo "Error: '$key' is retired $new. Run 'k2 help-deprecated' for the full map." >&2 ;;
            *)    echo "Error: '$key' is retired. Use instead: $new" >&2 ;;
        esac
        exit 1
    done
}

# The help-deprecated map rows for the table above.
_retired_verbs_map() {
    local row
    for row in "${_K2_RETIRED_VERBS[@]}"; do
        printf '  %-31s → %s\n' "${row%%|*}" "${row#*|}"
    done
}
# END_RETIRED_VERBS

# Hard-deprecation: print pointer to new equivalent + help-deprecated.
# Exits non-zero so scripts notice.
fail_deprecated() {
    local old_verb="$1"
    local new_form="$2"
    cat >&2 <<EOF
Error: '$old_verb' is no longer supported.

Use instead:
  $new_form

Run 'k2 help-deprecated' for the full map of retired verbs.
EOF
    exit 1
}

