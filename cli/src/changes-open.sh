# ── k2 changes open | init (C5) ─────────────────────────────────────

# k2 changes open: ask a K2 window that shows this workspace to open (or
# focus) its Changes tab (daemon event open_changes). 200 = asked; no
# window may be listening.
_changes_open() {
    local ws=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            *) _changes_die usage "unknown argument for k2 changes open: $1" 2 ;;
        esac
    done
    _changes_target "$ws"
    _changes_call POST /cli/git/open-changes "$(_changes_body all)"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        return 0
    fi
    printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin)
print("Asked K2 to open the Changes tab for %s (any window showing that workspace opens it)." % r.get("workspacePath", ""))'
}

# k2 changes init: make the workspace root a git repository (git init; no
# commit). Refused inside an existing repo (already_a_repo) or below/above
# a workspace root (bad_path).
_changes_init() {
    local ws=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            *) _changes_die usage "unknown argument for k2 changes init: $1" 2 ;;
        esac
    done
    _changes_target "$ws"
    # Inside a checkout the target is that checkout (already a repo); init
    # is about the workspace root, so outside a cell send the workspace.
    if [ -z "$ws" ] && ! _changes_in_cell; then
        _CH_PATH="$PROJECT"
    elif [ -z "$ws" ]; then
        _CH_PATH=""
    fi
    _changes_call POST /cli/git/init "$(_changes_body all)"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        return 0
    fi
    printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin)
print("Initialized a git repository in %s (no commits yet)." % r.get("repoRoot", ""))'
}

