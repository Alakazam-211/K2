# ── Phase 2.1: update --cli / --app flag handler ─────────────────────

# `k2 update` = `k2 update --app`. _k2_update_form (cli-tools-policy.sh)
# reads the words; --check and -c go to the --cli form.
cmd_update_v2() {
    local a cli_args=()
    for a in "$@"; do
        case "$a" in --check|-c) cli_args+=("$a") ;; esac
    done
    case "$(_k2_update_form "$@")" in
        help) _update_usage ;;
        list) cmd_app_update --list ;;
        cli)  cmd_update ${cli_args[@]+"${cli_args[@]}"} ;;
        *)    cmd_app_update ;;
    esac
}
