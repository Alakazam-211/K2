# ── 0.38.7: `k2 whats-new` ──────────────────────────────────────────
#
# Prints the user-facing changelog highlights for versions newer than
# the user's last-seen marker, with subcommands to inspect / reset /
# mark-seen. Mirrors what the Tauri popup shows on first launch after
# an update; mostly here for terminal-driven users who don't open the
# app every time.
cmd_whatsnew() {
    local action="show"  # show | all | reset | mark-seen
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --reset) action="reset"; shift ;;
            --all) action="all"; shift ;;
            --mark-seen) action="mark-seen"; shift ;;
            --help|-h)
                cat <<'EOF'
k2 whats-new [--reset] [--all] [--mark-seen]

Show the user-facing changelog for K2 versions you haven't seen yet
(the same content the Tauri app shows in its popup on first launch
after an update).

OPTIONS
  --reset      Clear the "last seen" marker so the popup shows again
               next K2 launch.
  --all        Print the full changelog regardless of last-seen state.
  --mark-seen  Manually mark the current version as seen (rarely needed;
               the popup does this on dismiss).
EOF
                return 0 ;;
            *) shift ;;
        esac
    done

    case "$action" in
        reset)
            local resp
            resp=$(cli_request_post "/cli/whats_new/reset" "")
            echo "$resp"
            echo "(K2 will show the popup again on next app launch.)" >&2
            ;;
        mark-seen)
            cli_request_post "/cli/whats_new/mark_seen" ""
            ;;
        show|all)
            local resp
            resp=$(cli_request "/cli/whats_new" "")
            # Parse + print content; --all bypasses the has_new gate.
            # Python embedded in single-quoted bash; uses only single
            # quotes internally so no `\"` escaping is needed.
            local script
            if [[ "$action" == "all" ]]; then
                script='import json, sys
try:
    d = json.load(sys.stdin)
    content = d.get("content", "").strip() or "(no changelog available)"
    cv = d.get("current_version", "?")
    ls = d.get("last_seen_version") or "(never)"
    print(content)
    sys.stderr.write("\n— current: " + cv + ", last seen: " + ls + "\n")
except Exception as e:
    sys.stderr.write("could not parse response: " + str(e) + "\n")
    sys.exit(1)'
            else
                script='import json, sys
try:
    d = json.load(sys.stdin)
    if d.get("has_new"):
        print(d.get("content", "").strip())
        cv = d.get("current_version", "?")
        sys.stderr.write("\n— up through K2 " + cv + "\n")
    else:
        cv = d.get("current_version", "?")
        ls = d.get("last_seen_version") or "(never)"
        sys.stderr.write("You are up to date on K2 " + cv + ". (Last seen: " + ls + ")\n")
except Exception as e:
    sys.stderr.write("could not parse response: " + str(e) + "\n")
    sys.exit(1)'
            fi
            echo "$resp" | python3 -c "$script"
            ;;
    esac
}

