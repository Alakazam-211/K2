# ── `k2 thread` — user-agent overlay (not PTY; that's k2 msg) ───────
_thread_sender_from() {
    local explicit_from="$1"
    if [[ -n "$explicit_from" ]]; then
        printf '%s' "$explicit_from"
        return 0
    fi
    local s_cell s_prim s_side
    _envn_into s_cell CELL
    _envn_into s_prim PRIMARY
    _envn_into s_side SIDECAR_NAME
    if [[ "$s_cell" == "sidecar" && -n "$s_prim" && -n "$s_side" ]]; then
        printf '%s' "${s_prim}/${s_side}"
        return 0
    fi
    local sender_path
    _envn_into sender_path PROJECT_PATH "$PWD"
    if [[ -n "$sender_path" ]]; then
        local sender_name
        # A lookup, not the command's request: never its closing line.
        sender_name=$(_K2_PERMISSION_QUIET=1 cli_request "/cli/workspace/handle" \
            "project=$(urlencode "$sender_path")" 2>/dev/null \
            | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    name = (d.get("handle") or "").strip()
    if name:
        print(name)
except Exception:
    pass' 2>/dev/null)
        printf '%s' "${sender_name:-k2}"
        return 0
    fi
    printf '%s' "k2"
}

