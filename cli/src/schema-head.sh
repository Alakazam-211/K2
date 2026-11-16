# ── GH#8 #4: `k2 --schema [json]` — machine-readable command surface ──
#
# Dumps every agent-facing verb (flags/args + a one-line description) as
# JSON so a calling agent loads the whole CLI surface in ONE call rather
# than burning a round-trip per `--help`. The wire format is a STATIC
# manifest hand-maintained below.
#
#   !!! KEEP IN SYNC WITH THE VERBS !!!
#   This block is hand-written and is NOT auto-derived from the router
#   (the `case` at the bottom) or the cmd_help_* text. When you ADD,
#   RENAME, REMOVE, or change the flags of a verb, update this manifest
#   in the same commit. It intentionally covers the agent-facing surface
#   (msg / talk / read / terminal / inbox / review / sessions /
#   connections / workspace / activity / --schema / ...), not
#   every internal/deprecated arm. See the report note re: a generated
#   approach (feasible later via a router+help introspection pass).
#
# Every top-level word the router takes needs an entry here, or a line in
# the list below saying why it has none (one of: retired, stub, internal,
# person-only, beta). tests/cli/schema_drift.sh reads both and fails on a
# word with neither. An alias (feedback|tickets, k2 reminders) is covered
# by its command's entry.
#
# BEGIN_SCHEMA_LEFT_OUT
#   state           retired
#   status          retired
#   checkin         retired
#   app-update      retired
#   context         stub
#   commit          internal
#   reviews         internal
#   hooks           internal
#   reserve         internal
#   release         internal
#   trust-folders   internal
#   onboarding      person-only
#   api             person-only
#   permission      beta
# END_SCHEMA_LEFT_OUT
cmd_schema() {
    # The manifest body is a QUOTED heredoc (<<'EOF'), so nothing in it is
    # interpolated: backticks, `$`, and `\\` are literal bytes and the body
    # is exactly the JSON written here. Write `\\` for one JSON backslash;
    # never write `\$`. The only dynamic value (the version) is printed by
    # printf, JSON-escaped, before the body. This was once an unquoted
    # heredoc: backtick spans ran as commands and `\\` collapsed into
    # invalid JSON escapes.
    # Test: tests/cli/schema_json.sh (parses the full output; stderr empty).
    local _v="$K2_CLI_VERSION"
    _v="${_v//\\/\\\\}"
    _v="${_v//\"/\\\"}"
    printf '{\n  "tool": "k2",\n  "version": "%s",\n' "$_v"
    cat <<'EOF'
  "description": "Agent interface to K2 workspace orchestration (talks to the local K2 daemon over HTTP).",
  "schema_note": "Static hand-maintained manifest of the agent-facing command surface. Run 'k2 help <verb>' for full prose.",
  "addressing": "Most verbs take a <workspace> as a NAME (preferred; see 'k2 connections list'), an absolute path, or a project UUID.",
  "commands": [
