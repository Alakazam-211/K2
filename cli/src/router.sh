# ── Router ───────────────────────────────────────────────────────────
# Select TCP credential by catalog mode BEFORE any verb handler runs.
# ID/locked tools in a scoped cell keep SCOPED_TOKEN; Open tools may
# invert to DISK_OWNER_TOKEN (COMPAT-58). See _cli_apply_auth_for_verb.
# A passport from a shared Codex app-server or a dead session is refused
# with a clear error first (never an owner fallback).
_cli_check_session_passport "${1:-help}"
_cli_apply_auth_for_verb "${1:-help}" "${2:-}"

# Permission cards (Beta): print the refusal's closing line, if a transport
# noted one, once the command's own output is done. Prints nothing and
# keeps the exit code otherwise (see BEGIN_CLI_PERMISSION_NOTE).
if [ -e "$_K2_PERMISSION_NOTE" ]; then rm -f "$_K2_PERMISSION_NOTE" 2>/dev/null || true; fi
trap _permission_note_flush EXIT

# TC-19: a refused App command that asked for --json gets the JSON error
# object only (with next and explain), no plain "fix:" line.
_K2_ARGS_JSON=0
for _k2_arg in "$@"; do [ "$_k2_arg" = "--json" ] && _K2_ARGS_JSON=1; done
unset _k2_arg

case "${1:-help}" in
    fed)
        # Federation V1 (cross-server agent comms). Dark by default.
        shift; cmd_fed "$@"
        ;;
    agent)
        # 0.40.24: `k2 agent` is the agent MANAGEMENT plane (PRD
        # prd-agent-cli-0.40.24.md) — conf/get/set (S2), hire/list
        # (S3), retire (S4). The 0.39.0f internal-tier subverbs
        # (`update`, `complete`, `profile`) and the hard-deprecated
        # arms are preserved below the new verbs.
        case "${2:-}" in
            conf)   shift 2; cmd_agent_conf "$@" ;;
            get)    shift 2; cmd_agent_get "$@" ;;
            set)    shift 2; cmd_agent_set "$@" ;;
            hire)   shift 2; cmd_agent_hire "$@" ;;
            # 0.46.1: picture, color and name (agents: their own color and picture).
            identity) shift 2; cmd_agent_identity "$@" ;;
            # Sew for Apps (0.46.6): the switch the Sew hints name
            # (`k2 agent access set <ws> chat_manage on`), owner/admin only.
            access) shift 2; cmd_agent_access "$@" ;;
            # 0.45.6 agent hire: team links, hire instances, the hire root.
            team|teams) shift 2; cmd_agent_team "$@" ;;
            instances|instance) shift 2; cmd_agent_instances "$@" ;;
            hire-root) shift 2; cmd_agent_hire_root "$@" ;;
            # 0.46.12: hireable agent types (prd-agent-templates-v1).
            template|templates) shift 2; cmd_agent_template "$@" ;;
            # S3 deliberately RECLAIMS the previously-deprecated `list`
            # arm with the real fleet-view semantics (PRD §3).
            list)   shift 2; cmd_agent_list "$@" ;;
            retire) shift 2; cmd_agent_retire "$@" ;;
            restart) shift 2; cmd_agent_restart "$@" ;;
            # 0.46.6: move the pinned chat to another harness.
            harness) shift 2; cmd_agent_harness "$@" ;;
            refresh) shift 2; cmd_agent_refresh "$@" ;;
            # Context management stack — day-2 AGENTS.md layer stack
            # (prd-context-hamburger-v1).
            context) shift 2; cmd_agent_context "$@" ;;
            --help|-h|'') cmd_help_agent; exit 0 ;;
            update) shift 2; cmd_agent_update "$@" ;;
            complete) shift 2; cmd_agent_complete "$@" ;;
            profile) shift 2; cmd_workspace_profile "$@" ;;
            *)
                _retired_verb_exit "agent ${2:-}"
                echo "Unknown agent subcommand: ${2:-}" >&2
                cmd_help_agent >&2
                exit 2 ;;
        esac
        ;;
    context)
        # Top-level teach string — stack lives under `k2 agent context`.
        echo "Use: k2 agent context <list|add|remove|on|off|move|show|regen|catalog>" >&2
        echo "Day-2 management of the always-on AGENTS.md context management stack." >&2
        echo "See: k2 agent context --help" >&2
        exit 2
        ;;
    preset)
        # W6 (0.40.30): `k2 preset` — agent-preset management
        # (list/show/add/set/remove) over /cli/presets/*. See
        # `k2 preset --help`; docs/agent-contract.md for the custom-
        # agent contract these presets describe.
        case "${2:-}" in
            list)   shift 2; cmd_preset_list "$@" ;;
            show)   shift 2; cmd_preset_show "$@" ;;
            add)    shift 2; cmd_preset_add "$@" ;;
            set)    shift 2; cmd_preset_set "$@" ;;
            remove) shift 2; cmd_preset_remove "$@" ;;
            --help|-h|'') cmd_help_preset; exit 0 ;;
            *)
                _preset_err "usage" "unknown command '${2:-}' — see k2 preset --help"
                exit 2 ;;
        esac
        ;;
    feedback|tickets)
        # Feedback F1 — durable agent→human asks (ask/list/show/
        # comment/resolve). See `k2 feedback --help`.
        case "${2:-}" in
            ask)     shift 2; cmd_feedback_ask "$@" ;;
            list)    shift 2; cmd_feedback_list "$@" ;;
            show)    shift 2; cmd_feedback_show "$@" ;;
            template) shift 2; cmd_feedback_template "$@" ;;
            comment) shift 2; cmd_feedback_comment "$@" ;;
            assign)  shift 2; cmd_feedback_assign "$@" ;;
            resolve) shift 2; cmd_feedback_resolve "$@" ;;
            settings) shift 2; cmd_feedback_settings "$@" ;;
            --help|-h|'') cmd_help_feedback; exit 0 ;;
            *)
                echo "Unknown feedback subcommand: ${2:-}" >&2
                cmd_help_feedback >&2
                exit 2 ;;
        esac
        ;;
    organization)
        # The Org Chart (prd-org-chart-v1): orgs, heads, lines and the daily
        # bulletin. One tool, no `k2 org` short form. See `k2 organization --help`.
        shift; cmd_organization "$@"
        ;;
    project)
        # Projects V1 P3 — named groups of agent workspaces with ONE
        # shared chat + one PoC (create/list/show/add/remove/poc/msg/
        # read). See `k2 project --help`. NOT the legacy workspace
        # registry (`/cli/projects/*`).
        case "${2:-}" in
            create) shift 2; cmd_project_create "$@" ;;
            rename) shift 2; cmd_project_rename "$@" ;;
            delete) shift 2; cmd_project_delete "$@" ;;
            pin)    shift 2; cmd_project_pin "$@" ;;
            view)   shift 2; cmd_project_view "$@" ;;
            list)   shift 2; cmd_project_list "$@" ;;
            show)   shift 2; cmd_project_show "$@" ;;
            add)    shift 2; cmd_project_add "$@" ;;
            remove) shift 2; cmd_project_remove "$@" ;;
            poc)    shift 2; cmd_project_poc "$@" ;;
            msg)    shift 2; cmd_project_msg "$@" ;;
            read)   shift 2; cmd_project_read "$@" ;;
            access) shift 2; cmd_project_access "$@" ;;
            --help|-h|'') cmd_help_project; exit 0 ;;
            *)
                _project_err "usage" "unknown command '${2:-}' — see k2 project --help"
                exit 2 ;;
        esac
        ;;
    mail)
        # K2 Mail (prd-email-server-v1 §11) — agents mint addresses on
        # verified domains, read/wait on incoming mail, and send under
        # human governance; owners run the server/domains/approvals.
        # Family is `mail`, deliberately NOT `inbox` (that's K2's
        # internal work queue). See `k2 mail --help`.
        case "${2:-}" in
            # S11: the unified discovery verb (hosted + linked, with your
            # access level). `addresses`/`list` are aliases.
            inboxes)     shift 2; cmd_mail_inboxes "$@" ;;
            addresses)   shift 2; cmd_mail_inboxes "$@" ;;
            list)        shift 2; cmd_mail_inboxes "$@" ;;
            domains)     shift 2; cmd_mail_domains "$@" ;;
            messages)    shift 2; cmd_mail_messages "$@" ;;
            read)        shift 2; cmd_mail_read "$@" ;;
            attachments) shift 2; cmd_mail_attachments "$@" ;;
            wait)        shift 2; cmd_mail_wait "$@" ;;
            watch)       shift 2; cmd_mail_watch "$@" ;;
            send)        shift 2; cmd_mail_send "$@" ;;
            reply)       shift 2; cmd_mail_reply "$@" ;;
            forward-message) shift 2; cmd_mail_forward_message "$@" ;;
            # 0081 inbox management + delete (agent verbs; canManage /
            # canDelete gated). delete = move to Trash, never an erase.
            move)        shift 2; cmd_mail_move "$@" ;;
            flag)        shift 2; cmd_mail_flag "$@" ;;
            archive)     shift 2; cmd_mail_archive "$@" ;;
            delete)      shift 2; cmd_mail_delete "$@" ;;
            folder)      shift 2; cmd_mail_folder "$@" ;;
            # Save a DRAFT into a LINKED assistant inbox's real Drafts
            # folder (reply onto <id>, or compose --to/--subject). Agent
            # verb; sending is a separate 'send'-level verb.
            draft)       shift 2; cmd_mail_draft "$@" ;;
            drafts)      shift 2; cmd_mail_drafts "$@" ;;
            outbox)      shift 2; cmd_mail_outbox "$@" ;;
            # S11: the user's OWN external accounts (link add|list|remove)
            # and the unified per-workspace access surface.
            link)        shift 2; cmd_mail_link "$@" ;;
            access)      shift 2; cmd_mail_access "$@" ;;
            # MCS1: per-inbox capabilities (agent verb) and the owner's
            # OAuth app config (owner verb; agents exit 3 server-side).
            capabilities) shift 2; cmd_mail_capabilities "$@" ;;
            oauth-config) shift 2; cmd_mail_oauth_config "$@" ;;
            # Owner/admin: per-workspace agent-send mode (every OS).
            sending)     shift 2; cmd_mail_sending "$@" ;;
            # Back-compat: `mail external` → `mail link`.
            external)    shift 2; cmd_mail_external "$@" ;;
            # The mail reader (prd-ocmail-admin-in-k2-v1 OA7): PEOPLE open
            # this server's inboxes (Owner/Admin: all; else their own).
            # Agents exit 3 server-side.
            reader)      shift 2; cmd_mail_reader "$@" ;;
            # Owner/hosting verbs moved to `k2 hostmail` — redirect.
            # (`delete` is now the message move-to-Trash verb above; the
            # address-retire verb is `k2 hostmail delete`.)
            # 0.46.7: a person's OWN mailbox forward (person routes). The
            # admin verb stays `k2 hostmail forward`.
            forward)     shift 2; cmd_mail_forward "$@" ;;
            create|status|domain|doctor|config|approvals|catchall|alias|ooo|footer|spam|queue|acl|autoconfig|app-password|person|dkim|dmarc|ptr|bans|allowlist|audit-sieve)
                _mail_err "usage" "'k2 mail ${2}' moved to 'k2 hostmail ${2}' (server admin)"
                exit 2 ;;
            --help|-h|'') cmd_help_mail; exit 0 ;;
            *)
                _mail_err "usage" "unknown command '${2:-}' — see k2 mail --help"
                exit 2 ;;
        esac
        ;;
    calendar|cal)
        # Calendars S3 — the agent calendar CLI over a hosted inbox
        # (read/draft = your k2 mail access grant). See k2 calendar --help.
        shift
        cmd_calendar "$@"
        ;;
    hostmail)
        # K2 Mail server administration (owner surface) — status, enable/
        # disable, domains, mint addresses, doctor, config, approvals.
        # Agents: the per-workspace hosted-mail toggle opens the M5 verbs;
        # uninstall/upgrade/domain remove stay owner-only (server-side, exit 3).
        shift
        cmd_hostmail "$@"
        ;;
    db)
        shift
        cmd_db "$@"
        ;;
    store)
        shift
        cmd_store "$@"
        ;;
    board)
        shift
        cmd_board "$@"
        ;;
    dns)
        # DNS K1 — toggle-gated zone/record management via /cli/dns/*.
        # Prefer UDS; POST JSON for mutations. See `k2 dns --help`.
        case "${2:-}" in
            access)  shift 2; cmd_dns_access "$@" ;;
            list)    shift 2; cmd_dns_list "$@" ;;
            records) shift 2; cmd_dns_records "$@" ;;
            record)  shift 2; cmd_dns_record "$@" ;;
            verify)  shift 2; cmd_dns_verify "$@" ;;
            --help|-h|'') cmd_help_dns; exit 0 ;;
            *)
                _dns_err "usage" "unknown command '${2:-}' — see k2 dns --help"
                exit 2 ;;
        esac
        ;;
    domain)
        # Box inventory (prd-custom-domains). Not `k2 hostmail domain`.
        shift
        cmd_domain "$@"
        ;;
    cert)
        # Box certs for attached hostnames. Not `k2 hostmail cert`.
        shift
        cmd_cert "$@"
        ;;
    terminal)
        case "${2:-}" in
            spawn) shift 2; cmd_terminal_spawn "$@" ;;
            write) shift 2; cmd_terminal_write "$@" ;;
            read)  shift 2; cmd_terminal_read "$@" ;;
            *)     echo "Usage: k2 terminal <spawn|write|read> ..." >&2
                   echo "  write <id|workspace> [--agent <name>] \"message\"" >&2
                   exit 1 ;;
        esac
        ;;
    state)
        # Workspace States product feature retired entirely.
        fail_deprecated "state" "(removed — Workspace States no longer exist)"
        ;;
    commit)
        # Phase 2.1: commit --merge subsumes commit-merge per A2.3.
        shift; cmd_commit_v2 "$@"
        ;;
    reviews)
        shift; cmd_reviews "$@"
        ;;
    review)
        case "${2:-}" in
            approve)  shift 2; cmd_review_approve "$@" ;;
            reject)   shift 2; cmd_review_reject "$@" ;;
            feedback) shift 2; cmd_review_feedback "$@" ;;
            *)        echo "Unknown review subcommand: ${2:-}" >&2; cmd_help; exit 1 ;;
        esac
        ;;
    trust-folders)
        shift; cmd_trust_folders "$@"
        ;;
    workspace)
        # 0.40.24 (B4): --help/-h anywhere in a workspace subcommand prints
        # help and exits 0 — it must never reach a handler as an argument
        # (`k2 workspace create --help` used to create a workspace named
        # `--help`).
        if _wants_help "${@:2}"; then
            case "${2:-}" in
                design-tab|design-mode) _workspace_pinned_tab_help design ;;
                mobile-tab)             _workspace_pinned_tab_help mobile ;;
                *)                      cmd_help_workspace ;;
            esac
            exit 0
        fi
        case "${2:-}" in
            # Phase 2.1 A6.1 daily subverbs.
            list)              shift 2; cmd_workspace_list "$@" ;;
            launch)            shift 2; cmd_workspace_launch "$@" ;;
            profile)           shift 2; cmd_workspace_profile "$@" ;;
            update)            shift 2; cmd_workspace_update "$@" ;;
            triage)            shift 2; cmd_workspace_triage "$@" ;;
            # Existing lifecycle subverbs.
            create)            shift 2; cmd_workspace_create "$@" ;;
            open)              shift 2; cmd_workspace_open "$@" ;;
            remove)            shift 2; cmd_workspace_remove "$@" ;;
            resources)         shift 2; cmd_workspace_resources "$@" ;;
            # Folder trust (security sweep SS10): status / request / decide.
            trust)             shift 2; cmd_workspace_trust "$@" ;;
            preview)           shift 2; cmd_workspace_preview "$@" ;;
            cleanup)           cmd_workspace_cleanup ;;
            agent-name)        shift 2; cmd_workspace_get_agent_display_name "$@" ;;
            set-agent-name)    shift 2; cmd_workspace_set_agent_display_name "$@" ;;
            set-agent-identity) shift 2; cmd_workspace_set_agent_identity "$@" ;;
            set-handle)        shift 2; cmd_workspace_set_handle "$@" ;;
            resume-chat-args)  shift 2; cmd_workspace_resume_chat_args "$@" ;;
            swap-canonical)    shift 2; cmd_workspace_swap_canonical "$@" ;;
            # B3a (sandbox) — set/clear the per-workspace Anthropic API key.
            api-key)           shift 2; cmd_workspace_set_api_key "$@" ;;
            # Phase 0b — owner API guest policy (host-session inject every turn).
            api-guest-policy)  shift 2; cmd_workspace_api_guest_policy "$@" ;;
            # Host-sessions: keep/strip auto-approve flags on /v1 spawns (default ON).
            api-skip-permissions) shift 2; cmd_workspace_api_skip_permissions "$@" ;;
            # Per-workspace concurrent host-session live cell cap (default 15, max 512).
            host-session-cell-cap|api-host-session-cap) shift 2; cmd_workspace_host_session_cell_cap "$@" ;;
            default-model)     shift 2; cmd_workspace_default_model "$@" ;;
            force-model-on-resume) shift 2; cmd_workspace_force_model_on_resume "$@" ;;
            design-tab|design-mode) shift; cmd_workspace_pinned_tab design "$@" ;;
            mobile-tab)        shift; cmd_workspace_pinned_tab mobile "$@" ;;
            *)                 echo "Usage: k2 workspace <list|launch|profile|update|create|open|remove|resources|trust|triage|set-agent-name|set-agent-identity|set-handle|resume-chat-args|swap-canonical|api-key|api-guest-policy|api-skip-permissions|api-host-session-cap|host-session-cell-cap|default-model|force-model-on-resume|design-tab|mobile-tab|...> [args]" >&2; exit 1 ;;
        esac
        ;;
    onboarding)
        # Phase 2.1 A5: later → defer, fresh → start-fresh.
        # Old names still work but warn.
        case "${2:-}" in
            scan)         shift 2; cmd_onboarding_scan "$@" ;;
            adopt)        shift 2; cmd_onboarding_adopt "$@" ;;
            defer)        shift 2; cmd_onboarding_later "$@" ;;
            start-fresh)  shift 2; cmd_onboarding_fresh "$@" ;;
            later)
                warn_deprecated "onboarding later" "k2 onboarding defer"
                shift 2; cmd_onboarding_later "$@" ;;
            fresh)
                warn_deprecated "onboarding fresh" "k2 onboarding start-fresh"
                shift 2; cmd_onboarding_fresh "$@" ;;
            *)            echo "Usage: k2 onboarding <scan|adopt|defer|start-fresh> [args]" >&2; exit 1 ;;
        esac
        ;;
    remind)
        shift
        cmd_heartbeat_remind "$@"
        ;;
    reminders)
        shift
        cmd_heartbeat_remind list "$@"
        ;;
    heartbeat)
        # GH#22/#23/#24: introspection must NEVER mutate state. --help/-h
        # anywhere in the arg vector prints usage and exits 0 — before any
        # arm below can consume it as a name or schedule spec.
        # (`remind` prints its own help: its text may be any words.)
        for hb_arg in "${@:2}"; do
            case "$hb_arg" in
                --help|-h)
                    case "${2:-}" in
                        remind|reminders) ;;
                        *) cmd_help_heartbeat; exit 0 ;;
                    esac ;;
            esac
        done
        case "${2:-}" in
            # 0.46.11: a one-time heartbeat (k2 remind is the short form).
            remind)         shift 2; cmd_heartbeat_remind "$@" ;;
            reminders)      shift 2; cmd_heartbeat_remind list "$@" ;;
            # GH#10/#24: documented default — bare `k2 heartbeat` lists
            # active schedules (the old bare-verb triage POST hit a route
            # removed in 0.40.31 and exited 0 on the error).
            "")             cmd_heartbeat_list ;;
            help)           cmd_help_heartbeat ;;
            schedule)
                # GH#22/#24: the documented `schedule` family, routed to the
                # named-heartbeat CRUD. Sub-verbs are validated — an
                # unrecognized token is a usage error, never a write.
                shift 2
                case "${1:-}" in
                    ""|list)
                        # GH#27: pass flags through — cmd_heartbeat_list
                        # handles --json and --archived (in either order).
                        [ "${1:-}" = "list" ] && shift
                        cmd_heartbeat_list "$@"
                        ;;
                    add)        shift; cmd_heartbeat_add "$@" ;;
                    remove)     shift; cmd_heartbeat_remove "$@" ;;
                    archive)    shift; cmd_heartbeat_remove "$@" ;;   # alias — soft-archive default
                    unarchive)  shift; cmd_heartbeat_unarchive "$@" ;;
                    edit)       shift; cmd_heartbeat_edit "$@" ;;
                    rename)     shift; cmd_heartbeat_rename "$@" ;;
                    enable)     shift; cmd_heartbeat_enable_disable true "$@" ;;
                    disable)    shift; cmd_heartbeat_enable_disable false "$@" ;;
                    off)
                        # Pre-0.40.31 single unnamed workspace schedule.
                        fail_deprecated "heartbeat schedule off" "k2 heartbeat schedule disable <name>   (per-heartbeat pause)"
                        ;;
                    hourly|daily|weekly|monthly|yearly)
                        # Pre-0.40.31 single unnamed workspace schedule.
                        # Heartbeats are named rows now; the positional
                        # frequency syntax wrote unvalidated junk (GH#22).
                        fail_deprecated "heartbeat schedule ${1}" "k2 heartbeat schedule add --name <n> --${1} ...   (see 'k2 help heartbeat')"
                        ;;
                    *)
                        echo "Unknown 'heartbeat schedule' subcommand: ${1}" >&2
                        echo "Verbs: add, list [--archived], remove <n> [--purge], unarchive, edit, rename, enable, disable. See 'k2 help heartbeat'." >&2
                        exit 2
                        ;;
                esac
                ;;
            signal)
                shift 2
                case "${1:-}" in
                    fire)   shift; cmd_heartbeat_fire "$@" ;;
                    wakeup) shift; cmd_heartbeat_wakeup "$@" ;;
                    wake)   shift; cmd_heartbeat_wake "$@" ;;
                    *)
                        echo "Unknown 'heartbeat signal' subcommand: ${1:-}" >&2
                        echo "Verbs: fire <n>, wakeup <n> [--path-only|--edit], wake. See 'k2 help heartbeat'." >&2
                        exit 2
                        ;;
                esac
                ;;
            wake)           shift 2; cmd_heartbeat_wake "$@" ;;
            log)            shift 2; cmd_heartbeat_log "$@" ;;
            add)            shift 2; cmd_heartbeat_add "$@" ;;
            list)           shift 2; cmd_heartbeat_list "$@" ;;
            list-archived)  shift 2; cmd_heartbeat_list_archived "$@" ;;
            archive)        shift 2; cmd_heartbeat_remove "$@" ;;     # alias — soft-archive default
            unarchive)      shift 2; cmd_heartbeat_unarchive "$@" ;;
            remove)         shift 2; cmd_heartbeat_remove "$@" ;;
            fire)           shift 2; cmd_heartbeat_fire "$@" ;;
            enable)         shift 2; cmd_heartbeat_enable_disable true "$@" ;;
            disable)        shift 2; cmd_heartbeat_enable_disable false "$@" ;;
            use-pinned-session)
                shift 2
                case "${1:-}" in
                    on|true|1)   shift; cmd_heartbeat_use_pinned_session true "$@" ;;
                    off|false|0) shift; cmd_heartbeat_use_pinned_session false "$@" ;;
                    *) echo "Usage: k2 heartbeat use-pinned-session [on|off] <name>" >&2; exit 1 ;;
                esac
                ;;
            session)        shift 2; cmd_heartbeat_session "$@" ;;
            restart)        shift 2; cmd_heartbeat_restart "$@" ;;
            rename)         shift 2; cmd_heartbeat_rename "$@" ;;
            edit)           shift 2; cmd_heartbeat_edit "$@" ;;
            show)           shift 2; cmd_heartbeat_show "$@" ;;
            wakeup)         shift 2; cmd_heartbeat_wakeup "$@" ;;
            status)         shift 2; cmd_heartbeat_status "$@" ;;
            *)              shift; cmd_heartbeat_toggle "$@" ;;
        esac
        ;;
    hooks)
        case "${2:-}" in
            status)    shift 2; cmd_hooks_status "$@" ;;
            install)   shift 2; cmd_hooks_install 0 "$@" ;;
            uninstall) shift 2; cmd_hooks_install 1 "$@" ;;
            emit)      shift 2; cmd_hooks_emit "$@" ;;
            *)
                echo "Usage: k2 hooks status [--limit N] [--json]" >&2
                echo "       k2 hooks install [--json]     re-run the daemon's hook installer now" >&2
                echo "       k2 hooks uninstall [--json]   remove K2's entries from the agent CLI configs" >&2
                echo "       k2 hooks emit <event> [--json '<obj>']  report a custom program's lifecycle (k2 study hooks)" >&2
                exit 1
                ;;
        esac
        ;;
    daemon)
        case "${2:-status}" in
            status)    shift 2; cmd_daemon_status "$@" ;;
            start)     shift 2; cmd_daemon_start "$@" ;;
            stop)      shift 2; cmd_daemon_stop "$@" ;;
            restart)   shift 2; cmd_daemon_restart "$@" ;;
            survive)   shift 2; cmd_daemon_survive "$@" ;;
            log|logs)  shift 2; cmd_daemon_log "$@" ;;
            install)   shift 2; cmd_daemon_install "$@" ;;
            uninstall) shift 2; cmd_daemon_uninstall "$@" ;;
            *)
                echo "Usage: k2 daemon <status|start|stop|restart|survive|log|install|uninstall>" >&2
                echo "" >&2
                echo "  survive [on|off|status]" >&2
                echo "    On by default: agents keep running through K2 updates, restarts and crashes" >&2
                echo "    (the daemon's terminal.ptyd setting; same as Settings → General → Server)." >&2
                echo "" >&2
                echo "  install [--version <x.y.z>] [--bin-dir <dir>] [--manifest-url <url>]" >&2
                echo "          [--no-service] [--dry-run]" >&2
                echo "    Install the STANDALONE (headless, no-GUI) daemon binary from a" >&2
                echo "    signed GitHub release. Minisign-verifies the binary against the" >&2
                echo "    embedded updater pubkey, installs to ~/.local/bin by default, and" >&2
                echo "    writes a systemd (Linux) / launchd (macOS) supervisor unit so a" >&2
                echo "    crash respawns it. --dry-run prints the plan without downloading." >&2
                echo "" >&2
                echo "  stop" >&2
                echo "    Quit the daemon. This ENDS its agent sessions. macOS: launchctl unload;" >&2
                echo "    Linux: systemctl [--user] stop <unit>; Windows: POST /cli/daemon/stop." >&2
                echo "" >&2
                echo "  restart [--sessions] [--host <base-url>] [--token <token>] [--wait]" >&2
                echo "    Default: restart the LOCAL daemon (macOS launchctl kickstart, Linux" >&2
                echo "    systemctl restart). Agent sessions hosted by k2-ptyd keep running." >&2
                echo "    --sessions: end the agent sessions too (they start fresh)." >&2
                echo "    --host:  restart a REMOTE daemon with no GUI by POSTing" >&2
                echo "             <base-url>/cli/daemon/restart (e.g. --host https://<sub>.k2.dev)." >&2
                echo "             Authorizes with the owner token OR an Owner/Admin connect-user" >&2
                echo "             session token; --token defaults to ~/.k2/daemon.token." >&2
                echo "    --wait:  after the restart is accepted, poll <base>/boot-status every" >&2
                echo "             ~2s up to ~60s until phase=ready, then print the version it" >&2
                echo "             came back on. Timeout → message + non-zero exit." >&2
                exit 1
                ;;
        esac
        ;;
    tunnel)
        # K2 Connect tunnel — expose this daemon at https://<user>.k2.dev.
        shift; cmd_tunnel "$@"
        ;;
    settings)
        shift; cmd_settings "$@"
        ;;
    checkin)
        # Retired 0.46.11: it needed K2SO_AGENT_NAME, so it failed for
        # normal agents. The arm stays so old AGENT.md / SKILL.md text
        # gets a pointer. `/cli/checkin` stays on the daemon (public API).
        shift; _checkin_retired "$@"
        ;;
    status)
        # Hard-deprecated (top-level only).
        fail_deprecated "status" "k2 organization update \"<news>\""
        ;;
    done)
        # API host-session / sandbox (K2_API_CELL=1): lifecycle complete
        # (mark Grace, no drain message). Outside that context: mark your
        # task done (or --blocked) via /cli/done. D6: never use
        # sock/token presence as the discriminator.
        if _is_api_lifecycle_context; then
            shift; cmd_session_complete "$@"
        else
            shift; cmd_done "$@"
        fi
        ;;
    whoami)
        shift; cmd_whoami "$@"
        ;;
    setup)
        # prd-setup-checklist-v1 C2: the setup checklist (k2 setup --help).
        shift; cmd_setup "$@"
        ;;
    msg)
        # 0.45.6 sender check: `k2 msg who m_…`. 0.46.1: `who` is ALWAYS
        # the lookup — any other id used to fall through to a live send to
        # a workspace named "who". A bad id is the daemon's answer, a
        # missing one is exit 2.
        if [ "${2:-}" = "who" ]; then
            shift 2; cmd_msg_who "$@"
        else
            # Phase 2.1: msg now has --inbox and --signal flag forms.
            shift; cmd_msg_v2 "$@"
        fi
        ;;
    thread)
        shift; cmd_thread "$@"
        ;;
    browser)
        shift; cmd_browser "$@"
        ;;
    read)
        # 0.39.x: read a workspace's live terminal by NAME — the read
        # complement to `msg` (talk live) and `inbox` (mail). Addresses
        # by workspace name like `msg`, resolving to that workspace's
        # canonical session (or `--agent <name>` for a specific agent).
        shift; cmd_read "$@"
        ;;
    talk)
        # BETA: fused read+send. Reads the peer's live screen (k2 read)
        # FIRST so the agent can't forget to look, surfaces the
        # drive-or-back-off HITL guidance, then delivers via the
        # existing `k2 msg` path. No new daemon route; `msg` untouched.
        shift; cmd_talk "$@"
        ;;
    respond)
        # F2 (sandbox cell): the in-cell agent's response egress. Posts a
        # curated line to THIS session's response log over the per-cell UDS;
        # the external API caller drains it via GET /v1/sandboxes/<id>/messages.
        # Cell-only — errors outside a sandbox (no per-cell socket).
        shift; cmd_respond "$@"
        ;;
    whats-new)
        shift; cmd_whatsnew "$@"
        ;;
    activity)
        # Phase 2.1 A5: feed → activity.
        shift; cmd_activity "$@"
        ;;
    inbox)
        # Phase 2.1 A22: inbox-as-email primitive.
        shift; cmd_inbox "$@"
        ;;
    glossary)
        # Phase 2.1 A10: K2 glossary lookup.
        shift; cmd_glossary "$@"
        ;;
    help-deprecated)
        cmd_help_deprecated
        ;;
    help-internal)
        cmd_help_internal
        ;;
    sessions)
        # `k2 sessions spawn` is retired (moved to `k2 sidecar new`).
        shift; cmd_sessions "$@"
        ;;
    sidecar)
        # k2 sidecar v1 (prd-k2-sidecar-cli-v1).
        shift; cmd_sidecar "$@"
        ;;
    root-helper)
        # K2's one root door on Linux (prd-k2-root-helper-v1 §8.3).
        shift; cmd_root_helper "$@"
        ;;
    front)
        # front443: the box's :443 as an SNI router (0.46.1).
        shift; cmd_front "$@"
        ;;
    firewall)
        # The public-ports view and the owner's firewall options (0.46.6).
        shift; cmd_firewall "$@"
        ;;
    desktop)
        # This computer's screen in a Desktop tab, for people (0.46.11).
        shift; cmd_desktop "$@"
        ;;
    git-tools)
        # First run without git (0.46.4): Apple's command line tools / Linux git.
        shift; cmd_git_tools "$@"
        ;;
    push)
        # Phones and end-to-end encrypted pushes (prd-e2e-push-v1 §4a).
        shift; cmd_push "$@"
        ;;
    notifications)
        # Email Notifications (prd-email-notifications-core-v1, 0.46.5).
        shift; cmd_notifications "$@"
        ;;
    notify)
        # Agent email notices to people here (email notifications part 2, N10a).
        shift; cmd_notify "$@"
        ;;
    compute)
        # K2 compute nodes (prd-k2-compute-nodes-v1).
        shift; cmd_compute "$@"
        ;;
    changes)
        # The Changes tab from the CLI (prd-changes-tab-v1, C4).
        shift; cmd_changes "$@"
        ;;
    sim)
        # The workspace's mobile device (prd-mobile-dev-v1 §4).
        shift; cmd_sim "$@"
        ;;
    doc|docs)
        # Live Markdown docs (Markdown multiplayer v1, K3).
        shift; cmd_doc "$@"
        ;;
    hostchat)
        # The Sew chat + voice server on this box (Sew S29 K14).
        shift; cmd_hostchat "$@"
        ;;
    voice)
        # Voice rooms on this server (prd-k2-voice-rooms-v1 §9.3, preview).
        shift; cmd_voice "$@"
        ;;
    chat)
        # Sew chat inside Spaces, for agents (Sew S30 K21).
        shift; cmd_chat "$@"
        ;;
    sources|source)
        # Garden v3 data sources (contract-sources-v1 §7).
        shift; cmd_sources "$@"
        ;;
    llm)
        # LLM tokens (`k2 llm tokens`; `k2 llm accounts` is an alias).
        shift; cmd_llm "$@"
        ;;
    usage)
        # Token usage + subscription limits (prd-usage-v2 §5).
        shift; cmd_usage "$@"
        ;;
    vault)
        # K2 vault (prd-k2-vault-v1 §14).
        shift; cmd_vault "$@"
        ;;
    migrate)
        # Box moves carry the vault (prd-box-move-carry-v1).
        shift; cmd_migrate "$@"
        ;;
    job|jobs)
        # Daemon-run scheduled jobs (prd-vault-gaps-v1 item 5).
        shift; cmd_job "$@"
        ;;
    ci)
        # `k2 ci` (0.46.10, C6a): CI on K2 Compute, daemon-owned.
        shift; cmd_ci "$@"
        ;;
    reserve)
        shift; cmd_reserve "$@"
        ;;
    release)
        shift; cmd_release "$@"
        ;;
    connections)
        # 0.40.24 (B4): --help/-h on any connections subcommand prints help
        # and exits 0 instead of being consumed as a workspace name.
        if _wants_help "${@:2}"; then
            cmd_help_connections
            exit 0
        fi
        shift; cmd_connections "$@"
        ;;
    api)
        # 0.40.43 (1c) — public /v1 API surface switch (owner/admin). The
        # daemon gate is per-request, so on/off are live with no restart.
        shift; cmd_api "$@"
        ;;
    api-key)
        # P3a (sandbox) — owner-only API-key auth-tier management for /v1/*.
        shift; cmd_api_key "$@"
        ;;
    app)
        # Exact match only. Does not steal app-update or update --app.
        case "${2:-}" in
            # App runtime verbs (prd-k2-app-runtime-v1 stage 1) own their --help.
            # 0.46.4: `k2 app data` too (put|ls|get|rm|mv). 0.46.6: `k2 app
            # domain` (its custom-domain help, not the App family's) and
            # `k2 app sdk` (add|status|kit). 0.46.7: `k2 app updates`, `k2 app status`,
            # `k2 app chat` (App-governed Spaces; it printed the whole family before).
            validate|build|install|deploy|rollback|stop|start|restart|deployment|config|list|inspect|history|logs|errors|uninstall|expose|egress|approvals|approve|deny|guest|data|domain|sdk|updates|status|chat)
                shift; cmd_app "$@"
                ;;
            # 0.46.7: `k2 app init` and `k2 app links` own their --help.
            # 0.46.8: `k2 app push`, `k2 app pairing` and `k2 app login` too
            # (their pages say --help; it printed the whole family before).
            # 0.46.9: `k2 app doctor` (--store has its own page), `k2 app set`
            # and `k2 app account-deletion`; `k2 app auth` (the sign-in door).
            # 0.46.11: `k2 app recording` (Sew 0.9.0 recording in Apps).
            init|links|push|pairing|login|doctor|set|account-deletion|account-deletions|auth|recording|recordings)
                shift; cmd_app "$@"
                ;;
            # 0.46.11: `k2 app db` (connect an App to a database) owns its --help.
            db|database)
                shift; cmd_app "$@"
                ;;
            # RS-A15: `k2 app grant <app> --kind …` owns its --help too.
            grant)
                case "${3:-}" in
                    ""|list|add|create|rm|remove|delete|revoke|copy-host|copyhost|-h|--help)
                        if _wants_help "${@:2}"; then _skin_usage; _app_rt_usage >&2; exit 0; fi
                        ;;
                esac
                shift; cmd_app "$@"
                ;;
            *)
                if _wants_help "${@:2}"; then
                    _skin_usage
                    _app_rt_usage >&2
                    exit 0
                fi
                shift; cmd_app "$@"
                ;;
        esac
        ;;
    skin)
        _teach_skin_now_app
        if _wants_help "${@:2}"; then
            _skin_usage
            exit 0
        fi
        shift; cmd_skin "$@"
        ;;
    skin-token)
        _teach_skin_token_now_app_token
        if _wants_help "${@:2}"; then
            cmd_skin_token --help
            exit 0
        fi
        shift; cmd_skin_token "$@"
        ;;
    publish)
        # Hosted services (run/ps/start/stop/rm/logs → /cli/publish/*) plus
        # nested-subdomain CP (status + subdomain *). P6: status stays plan.
        shift; cmd_publish "$@"
        ;;
    clone)
        # k2 clone (prd-clone-carries-threads-and-vault-v1 §4.3/§4.4).
        shift; cmd_clone "$@"
        ;;
    users)
        # Local daemon connect-user accounts (list / add). Owner-token
        # bootstrap for the first owner; gated thereafter server-side.
        if _wants_help "${@:2}"; then
            _users_usage
            exit 0
        fi
        shift; cmd_users "$@"
        ;;
    remote-session)
        # Remote Session Layer 0 + grants + shell PTY (owner/admin CLI).
        # Locked id tool; connection gate required (daemon). Grant-driven
        # shell/write/read use --token k2rs_…; admin ops use owner token.
        if _wants_help "${@:2}"; then
            cmd_help_remote_session
            exit 0
        fi
        shift; cmd_remote_session "$@"
        ;;
    zen)
        # Zen Mode (prd-zen-mode-v1 Z17): this computer's ~/.k2/zen via the
        # local daemon's owner-only /cli/zen/* routes. No grant verb: widgets
        # need no permissions (2026-10-08).
        _zen_garden_changes_hint "${@:2}"
        # `k2 zen guide` is static and handles its own --help; so does
        # `k2 zen data` (Garden data, contract-garden-data-v1 §8).
        if [ "${2:-}" = "data" ] && _wants_help "${@:2}"; then
            cmd_help_zen_data
            exit 0
        fi
        # The widget dev loop's verbs have their own pages (0.46.1 S3).
        if [ "${2:-}" = "widget" ] && _wants_help "${@:3}"; then
            case "${3:-}" in
                put) _zen_help_widget_put; exit 0 ;;
                apply) _zen_help_widget_put; exit 0 ;;
                delete) _zen_help_widget_delete; exit 0 ;;
                preview) _zen_help_widget_preview; exit 0 ;;
                errors|show) shift; cmd_zen widget "${2}" --help ;;
            esac
        fi
        if [ "${2:-}" = "validate" ] && _wants_help "${@:2}"; then
            _zen_help_validate
            exit 0
        fi
        # 0.46.4: agents-order has its own page.
        if [ "${2:-}" = "agents-order" ] && _wants_help "${@:3}"; then
            cmd_zen_agents_order --help
            exit 0
        fi
        # 0.46.4: links (k2.open's kept sites) has its own page too.
        if [ "${2:-}" = "links" ] && _wants_help "${@:3}"; then
            cmd_zen_links --help
            exit 0
        fi
        if [ "${2:-}" != "guide" ] && _wants_help "${@:2}"; then
            cmd_help_zen
            exit 0
        fi
        shift; cmd_zen "$@"
        ;;
    wiki)
        # Workspace knowledge base (brain map): index / note / seed /
        # localhost serve / publish via nested subdomain. Daemon
        # /cli/wiki/*; publish reuses k2 publish subdomain.
        if _wants_help "${@:2}"; then
            cmd_help_wiki
            exit 0
        fi
        shift; cmd_wiki "$@"
        ;;
    connect)
        # Headless Connect account pairing (login → pick subdomain →
        # tunnel bind/start). Nested Pro verbs live under `k2 publish`.
        if _wants_help "${@:2}"; then
            _connect_usage
            exit 0
        fi
        shift; cmd_connect "$@"
        ;;
    skills)
        shift; cmd_skills "$@"
        ;;
    app-update)
        # Phase 2.1 A1: hard-deprecated. Use update --app.
        fail_deprecated "app-update" "k2 update --app"
        ;;
    update)
        # Phase 2.1: update --app / --cli flag handler.
        shift; cmd_update_v2 "$@"
        ;;
    version|--version|-v)
        echo "k2 $K2_CLI_VERSION"
        ;;
    --schema|schema)
        # GH#8 #4: dump the full command surface as JSON so an agent
        # loads every verb + flags + a one-line description in ONE call,
        # instead of probing `--help` per verb. Optional trailing `json`
        # is accepted (JSON is the only/default format).
        cmd_schema
        ;;
    study)
        shift; cmd_study "$@"
        ;;
    explain)
        # 0.46.7 (prd-tauri-core-0467-v1 TC-18): an App refusal id → what
        # happened, the fix, who fixes it. No daemon.
        shift; cmd_explain "$@"
        ;;
    capabilities)
        # 0.46.7 (TC-17): what this server can do for Apps, before trying.
        shift; cmd_capabilities "$@"
        ;;
    send-feedback)
        # 0.46.11 (prd-k2-product-feedback-v1): feedback about K2 itself to
        # Alakazam Labs. Not `k2 feedback` (asking your own human).
        shift; cmd_send_feedback "$@"
        ;;
    permission)
        # Permission cards (Beta): unknown, exactly as today, unless the
        # switches are on (cmd_permission decides; BEGIN_CLI_PERMISSION).
        shift; cmd_permission "$@"
        ;;
    help|--help|-h)
        # Guard against `set -u` when no trailing args.
        if [ $# -gt 0 ]; then shift; fi
        cmd_help "$@"
        ;;
    help-advanced)
        # Legacy alias for the Phase 2.1 `help --advanced` form.
        cmd_help_v2_advanced
        ;;
    *)
        _retired_verb_exit "$1 ${2:-}"
        _retired_verb_exit "$1"
        echo "Unknown command: $1" >&2
        cmd_help
        exit 1
        ;;
esac
