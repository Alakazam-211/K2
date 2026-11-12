# ── `k2 thread` — user-agent overlay (not PTY; that's k2 msg) ───────
_thread_sender_from() {
    local explicit_from="$1"
    if [[ -n "$explicit_from" ]]; then
        printf '%s' "$explicit_from"
        return 0
    fi
    if [[ "${K2_CELL:-}" == "sidecar" && -n "${K2_PRIMARY:-}" && -n "${K2_SIDECAR_NAME:-}" ]]; then
        printf '%s' "${K2_PRIMARY}/${K2_SIDECAR_NAME}"
        return 0
    fi
    local sender_path="${K2_PROJECT_PATH:-${K2SO_PROJECT_PATH:-$PWD}}"
    if [[ -n "$sender_path" ]]; then
        local sender_name
        sender_name=$(cli_request "/cli/workspace/handle" \
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

