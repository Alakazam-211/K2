# ── Phase 2.1: update --cli / --app flag handler ─────────────────────

cmd_update_v2() {
    local target="app"  # default per mock: `update` = `update --app`
    local cli_args=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --app)  target="app";  shift ;;
            --cli)  target="cli";  shift ;;
            --list) cmd_app_update --list; return ;;
            --check|-c) cli_args+=("$1"); shift ;;
            *) shift ;;
        esac
    done
    if [[ "$target" == "cli" ]]; then
        cmd_update ${cli_args[@]+"${cli_args[@]}"}
    else
        cmd_app_update
    fi
}
