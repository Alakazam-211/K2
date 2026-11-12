# ── CLI Tools identity policy (mirrors crates/k2-core cli_tool_policy) ──
# Static catalog defaults for the bash CLI (daemon overrides live in
# Settings → CLI Tools; locked tools cannot be Open). Pure helpers below
# are unit-testable via the BEGIN/END markers (see tests/cli/).
# BEGIN_CLI_TOOL_POLICY

# Map a top-level `k2 <verb>` (and, for `agent`, its sub-verb) to a catalog
# tool id (empty = ungoverned).
_cli_tool_id_for_verb() {
    case "${1:-}" in
        # Agent hire (0.45.6, HT14): `k2 agent team` / `k2 agent instances`
        # are an `id` tool, so inside a K2 terminal the daemon sees the
        # session's passport (an agent reads its own links; it never links,
        # hires teams or retires). `k2 agent hire` stays open until noun
        # tiers S6, but sends its passport for the hire core itself.
        agent)
            case "${2:-}" in
                team|teams|instances|instance) echo "agent-teams" ;;
                # 0.46.1: `k2 agent identity` sends the passport in a K2
                # terminal, so an agent sets only its own color and picture.
                identity) echo "agent-identity" ;;
                # 0.46.6: `k2 agent harness` sends the passport in a K2
                # terminal, so an agent is refused (it asks with a card)
                # instead of moving its own pinned chat as the owner.
                harness) echo "agent-harness" ;;
                # 0.46.8: `k2 agent refresh` too: an agent's own chat, by its
                # own passport and its workspace's switch.
                refresh) echo "agent-harness" ;;
                *) echo "" ;;
            esac ;;
        # Kessel at first run: `k2 setup chief-of-staff` rides the locked
        # agent-harness tool id, so inside a K2 terminal it sends Kessel's
        # session passport (the daemon admits only Kessel's own pinned chat),
        # never the owner token an Open verb would re-source.
        setup)
            case "${2:-}" in
                chief-of-staff) echo "agent-harness" ;;
                *) echo "" ;;
            esac ;;
        # The leftover spelling of `k2 agent identity` rides the same tool.
        workspace)
            case "${2:-}" in
                set-agent-identity) echo "agent-identity" ;;
                *) echo "" ;;
            esac ;;
        help|--help|-h|glossary|help-deprecated|help-internal|help-advanced) echo "help" ;;
        --schema|schema) echo "schema" ;;
        study) echo "study" ;;
        whats-new) echo "whats-new" ;;
        # `checkin` is retired (a stub) but the tool id stays: `done` and
        # the `status` pointer still map to it (open by default).
        checkin|done|status) echo "checkin" ;;
        activity) echo "activity" ;;
        read) echo "read" ;;
        skills) echo "skills" ;;
        msg|talk|thread) echo "msg" ;;
        inbox) echo "inbox" ;;
        wiki) echo "wiki" ;;
        respond) echo "respond" ;;
        connections|reserve|release|roster|who) echo "connections" ;;
        # k2 sidecar v1 (SC35): an `id` tool, so inside a K2 terminal the
        # daemon sees the session's passport, never the owner token.
        sidecar) echo "sidecar" ;;
        # k2 compute (prd-k2-compute-nodes-v1): id + locked, so inside a K2
        # terminal it never re-sources the owner token (agents would
        # bypass the workspace switch and grants).
        compute) echo "compute" ;;
        # k2 ci (0.46.10, C6a): rides the locked compute tool id, so inside
        # a K2 terminal the daemon sees the session passport (its own
        # workspace, its "Allow compute nodes" switch), never the owner token.
        ci) echo "compute" ;;
        # k2 changes (prd-changes-tab-v1 C4): id + locked, so inside a K2
        # terminal it sends the session passport (the daemon scopes it to
        # the agent's own workspace), never the owner token.
        changes) echo "changes" ;;
        # k2 sim (prd-mobile-dev-v1 §4): id + locked, so inside a K2
        # terminal it sends the session passport (the daemon keeps the agent
        # to its own workspace's device and checks its "Allow mobile
        # devices" switch), never the owner token.
        sim) echo "sim" ;;
        # k2 desktop env (prd-linux-virtual-desktop-v1 L8): its own id +
        # locked tool, so inside a K2 terminal it sends the session passport
        # (the daemon gives the agent its own workspace's desktop and checks
        # its "Allow a virtual desktop" switch), never the owner token. The
        # other desktop verbs are people's and stay as they were.
        desktop)
            case "${2:-}" in
                env) echo "desktop" ;;
                *) echo "" ;;
            esac ;;
        # k2 doc (Markdown multiplayer v1, vs-live MI4): id + locked, so
        # inside a K2 terminal the daemon sees the session passport (its
        # own workspace; another only when connected and shared), never
        # the owner token, and edits are attributed to the agent.
        doc|docs) echo "docs" ;;
        # k2 hostchat (Sew S29 K14): id + locked, so inside a K2 terminal the
        # daemon sees the session passport and the "Manage Sew" switch
        # applies; person-only steps refuse it.
        hostchat) echo "hostchat" ;;
        # k2 voice (voice rooms, preview): rides the locked hostchat tool id,
        # so inside a K2 terminal the daemon sees the session passport (agents
        # read rooms; joins and changes refuse it), never the owner token.
        voice) echo "hostchat" ;;
        # k2 chat (Sew S30 K21): id + locked, so inside a K2 terminal the
        # daemon sees the session passport (the agent's own Sew member,
        # levels and Manage Sew), never the owner token.
        chat) echo "chat" ;;
        # k2 sources (Garden v3 data sources): id + locked, so inside a K2
        # terminal the daemon sees the session passport (agents can't act as
        # the owner on approvals, secrets or pins).
        sources|source) echo "sources" ;;
        # LLM tokens (`k2 llm tokens`, alias `accounts`): an `id` tool, so inside a K2
        # terminal the daemon sees the session passport (read-only there).
        llm) echo "llm-accounts" ;;
        # k2 usage (Usage v2): an `id` tool, so inside a K2 terminal the
        # daemon sees the session passport (its own workspace, read-only
        # limits), never the owner token.
        usage) echo "usage" ;;
        # K2 vault: an `id` tool, so inside a K2 terminal the daemon sees the
        # session passport (agent-level), never the owner token.
        vault) echo "vault" ;;
        # Box moves (prd-box-move-carry-v1): id + locked, so inside a K2
        # terminal the daemon sees the session passport (its steps are held
        # for the owner, with the permission card), never the owner token.
        migrate) echo "migrate" ;;
        # k2 job (prd-vault-gaps-v1 item 5): id + locked, so inside a K2
        # terminal the daemon sees the session passport (its own workspace's
        # jobs only), never the owner token.
        job|jobs) echo "jobs" ;;
        # K2 projects (0.45.6): an `id` tool, so inside a K2 terminal the
        # daemon sees the session passport, never the owner token, and the
        # agent's "Manage K2 projects" switch applies to its mutations.
        project) echo "project" ;;
        # The Org Chart: an `id` tool, so inside a K2 terminal the daemon
        # sees the session passport (the agent's own org, head and read
        # marker), never the owner token.
        # Its bare help needs no passport.
        organization)
            case "${2:-}" in ""|help|--help|-h) echo "" ;; *) echo "organization" ;; esac ;;
        heartbeat|remind|reminders) echo "heartbeat" ;;
        mail|hostmail) echo "mail" ;;
        # Calendars S3 (CAL23.4, security): `k2 calendar` rides the mail
        # tool id. Unmapped verbs are "open" and re-source the OWNER token
        # in a scoped cell, which would bypass every k2 mail access grant.
        calendar|cal) echo "mail" ;;
        db) echo "db" ;;
        store) echo "store" ;;
        # k2 board (Rosson 2026-11-01) is Garden data too: the same locked
        # id tool, so a K2 terminal sends its passport, never the owner token.
        board) echo "store" ;;
        dns) echo "dns" ;;
        # Custom-domain inventory + certs share the locked dns tool id
        # (prd-custom-domains A10). Unknown verbs elevate as Open.
        domain|cert) echo "dns" ;;
        # 0.46.6: the box's firewall and the :443 front ride the same locked
        # id tool, so inside a K2 terminal an agent sends its passport (it
        # reads the view; every change is owner_only), never the owner
        # token an Open verb would re-source.
        firewall|front) echo "dns" ;;
        publish) echo "publish" ;;
        # Headless Connect account pairing (login/status/logout). Distinct
        # from publish (nested-subdomain control plane) so login is not
        # always attributed as the publish tool.
        connect) echo "connect-account" ;;
        sessions|terminal) echo "sessions-spawn" ;;
        fs) echo "fs" ;;
        daemon) echo "daemon-admin" ;;
        users) echo "users-admin" ;;
        api-key) echo "api-key" ;;
        # 0.46.7: `k2 capabilities` reads App and compute facts as the
        # caller (a passport: its own workspace's nodes and Apps).
        app|skin|skin-token|capabilities) echo "skin" ;;
        # Remote Session owner/admin surface (Layer 0 + grants + shell PTY).
        # Locked id: cannot Open; needs owner/admin or grant token server-side.
        remote-session) echo "remote-session" ;;
        settings) echo "settings" ;;
        browser) echo "browser-open" ;;
        *) echo "" ;;
    esac
}

# Default identity mode for a tool id: open | id.
# Locked production tools are always id (cannot be Open in the catalog).
_cli_tool_default_mode() {
    case "${1:-}" in
        help|schema|whats-new|checkin|activity|read|skills|wiki|tools-policy|study|"") echo "open" ;;
        msg|inbox|respond|awareness|connections|heartbeat|sidecar|project|organization) echo "id" ;;
        mail|db|store|dns|publish|connect-account|sessions-spawn|fs|daemon-admin|users-admin|api-key|skin|remote-session) echo "id" ;;
        browser-open|settings|llm-accounts|compute|vault|sources|agent-teams|agent-identity|agent-harness|changes|usage|hostchat|jobs|sim|desktop|chat|migrate|docs) echo "id" ;;
        *) echo "open" ;;
    esac
}

# True when the tool is a locked production floor (cannot be set Open).
_cli_tool_is_locked() {
    case "${1:-}" in
        mail|db|store|dns|publish|connect-account|sessions-spawn|fs|daemon-admin|users-admin|api-key|skin|remote-session|llm-accounts|compute|vault|sources|agent-teams|agent-identity|agent-harness|changes|usage|hostchat|jobs|sim|desktop|chat|migrate|docs) return 0 ;;
        *) return 1 ;;
    esac
}

# Teaching error when an ID/locked verb has no session passport in-cell.
_cli_require_session_passport() {
    local tool_id="${1:-unknown}"
    echo "Error: tool '${tool_id}' requires session ID (passport)." >&2
    echo "Run from a K2 agent session that carries session identity, or ask the owner" >&2
    echo "to set this tool to Open in Settings → CLI Tools (if the platform allows)." >&2
    echo "Locked production tools cannot be set Open." >&2
    exit 3
}

# COMPAT-58 disk-token inversion — CONDITIONAL on tool mode.
# When K2_HOOK_SCOPED is on, the daemon injects a per-cell SCOPED token behind
# $K2_HOOK_TOKEN and the per-cell socket behind $K2_HOOK_SOCK.
#
#   Open tools  — re-source DISK OWNER for the TCP arm (migration ergonomics;
#                 scoped credentials still 403 on many owner-only TCP routes).
#   ID / locked — MUST NOT re-source ~/.k2/heartbeat.token. Keep SCOPED_TOKEN
#                 only; prefer per-cell UDS. No passport → exit 3.
#
# Outside a scoped cell (no $K2_HOOK_SOCK), disk owner discovery above is the
# human/external-terminal path and remains valid Owner proof for ID tools.
_cli_apply_auth_for_verb() {
    local verb="${1:-}" sub="${2:-}"
    local tool_id mode
    tool_id="$(_cli_tool_id_for_verb "$verb" "$sub")"
    mode="$(_cli_tool_default_mode "$tool_id")"
    local hook_sock="${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}"

    if [ "$mode" = "id" ]; then
        if [ -n "$hook_sock" ]; then
            # In-cell / scoped-session marker: never elevate to disk owner.
            if [ -z "$SCOPED_TOKEN" ]; then
                _cli_require_session_passport "${tool_id:-$verb}"
            fi
            TOKEN="$SCOPED_TOKEN"
        fi
        # No hook sock → TOKEN already owner (external) or env; Owner OK for ID.
    else
        # Open / ungoverned: keep COMPAT-58 inversion for TCP ergonomics.
        if [ -n "$hook_sock" ] && [ -n "${DISK_OWNER_TOKEN:-}" ]; then
            TOKEN="$DISK_OWNER_TOKEN"
        fi
    fi
}

# The credential project membership changes ride from verbs that are not
# themselves `id` tools (`k2 agent hire|set --project`): the session passport
# inside a K2 terminal (so the agent's "Manage K2 projects" switch applies),
# else $TOKEN (owner / external terminal).
_k2_project_token() {
    if [ -n "${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}" ] && [ -n "${SCOPED_TOKEN:-}" ]; then
        printf '%s' "$SCOPED_TOKEN"
    else
        printf '%s' "$TOKEN"
    fi
}

# END_CLI_TOOL_POLICY

# BEGIN_CLI_CURL_CFG
# Credentials never ride curl's argv (0.45.1). Every process's argv is
# readable by every other OS user (`ps`), so the owner token, a session
# passport, a Connect bearer and secret bodies (passwords, API keys) go
# to curl as a config read from stdin (`curl -K -`). The `printf` builtin
# writes it: no exec, no argv, no temp file, no /dev/fd. Works with the
# macOS system curl and Linux curl 7.68+ (Ubuntu 20.04), bash 3.2+.
# Wire bytes are unchanged: `?token=` stays in the query, only the way
# curl learns the URL / header / body moved.

# _k2_cq VALUE — set _K2_CQ to VALUE as a double-quoted curl config string
# (\ and " escaped, newline → \n; curl's config parser undoes exactly
# that). Plain values (tokens, urlencoded params) never fork. Values that
# need escaping go through sed: bash 3.2's ${s//x/y} is quadratic.
_k2_cq() {
    local s="$1"
    if [[ "$s" == *\\* || "$s" == *\"* || "$s" == *$'\n'* ]]; then
        # Sentinel `x` keeps a trailing newline (sed/$(…) would drop it).
        s="$(printf '%sx' "$s" | LC_ALL=C sed -e 's/[\\"]/\\&/g' -e '$!s/$/\\n/' | LC_ALL=C tr -d '\n')"
        s="${s%x}"
    fi
    _K2_CQ="\"${s}\""
}

# _k2_cfg_add NAME VALUE — append `NAME = "VALUE"` to the caller's
# `local _K2_CFG` (bash dynamic scope; no subshell).
_k2_cfg_add() {
    _k2_cq "$2"
    _K2_CFG="${_K2_CFG}${1} = ${_K2_CQ}"$'\n'
}

# _k2_curl CFG curl-args… — run curl with CFG (config lines) on stdin.
# CFG empty → plain curl. The caller's stdin is NOT available to curl.
#
# k2-ptyd S5 (PRD P47, P49): a daemon restart must not fail an agent's
# `k2` call. curl exit 7 (nothing accepted the connection: the daemon is
# between processes and nothing held its socket) is retried, at most
# $_K2_CURL_TRIES tries, 1 s apart; such a request never reached a
# handler, so a retry can't run it twice. Never `curl --retry` (it also
# retries after a response). A request whose body is read from stdin
# (`@-`, `-T -`) is not retried: the first try consumed it. A refused
# loopback request on $PORT also tries the port the daemon published in
# ~/.k2/daemon.port, when that one differs (the daemon came back on
# another port).
_K2_CURL_TRIES=4
_k2_curl() {
    local cfg="$1"; shift
    local try=1 rc once=0 a
    for a in "$@"; do
        case "$a" in *@-*|-T) once=1 ;; esac
    done
    while :; do
        rc=0
        if [ -z "$cfg" ]; then
            curl "$@" || rc=$?
        else
            printf '%s' "$cfg" | curl -K - "$@" || rc=$?
        fi
        if [ "$rc" -ne 7 ] || [ "$once" -eq 1 ] || [ "$try" -ge "$_K2_CURL_TRIES" ]; then
            return "$rc"
        fi
        try=$((try + 1))
        sleep 1
        _k2_follow_port_move
        if [ -n "$_K2_PORT_FROM" ]; then
            # Only the request URL moves: the config's `url = "…"` line or a
            # URL argument, by exact prefix. Bodies, data and headers that
            # happen to contain `:<port>/` are left alone.
            local from_url="http://${HOST:-127.0.0.1}:${_K2_PORT_FROM}/" to_url="http://${HOST:-127.0.0.1}:${PORT}/" l new_cfg=""
            if [ -n "$cfg" ]; then
                while IFS= read -r l; do
                    case "$l" in
                        "url = \"${from_url}"*) l="url = \"${to_url}${l#"url = \"${from_url}"}" ;;
                    esac
                    new_cfg="${new_cfg}${l}"$'\n'
                done <<< "${cfg%$'\n'}"
                cfg="$new_cfg"
            fi
            local -a _k2_args=()
            for a in "$@"; do
                case "$a" in
                    "${from_url}"*) a="${to_url}${a#"$from_url"}" ;;
                esac
                _k2_args+=("$a")
            done
            set -- "${_k2_args[@]}"
        fi
    done
}

# P49: when $PORT is a loopback port the daemon no longer publishes, move
# PORT/BASE_URL to the one in ~/.k2/daemon.port. Sets _K2_PORT_FROM to the
# old port (empty when nothing moved).
_k2_follow_port_move() {
    _K2_PORT_FROM=""
    case "${HOST:-127.0.0.1}" in 127.0.0.1|localhost|::1) ;; *) return 0 ;; esac
    local f="${K2_HOME:-$HOME/.k2}/daemon.port" p
    [ -f "$f" ] || return 0
    p="$(tr -d '[:space:]' < "$f" 2>/dev/null)" || return 0
    case "$p" in ''|*[!0-9]*) return 0 ;; esac
    [ -n "${PORT:-}" ] && [ "$p" != "$PORT" ] || return 0
    _K2_PORT_FROM="$PORT"
    PORT="$p"
    BASE_URL="http://${HOST:-127.0.0.1}:${PORT}"
}
# END_CLI_CURL_CFG

# BEGIN_CLI_STALE_PASSPORT
# Codex 0.155+ runs every interactive session's tool commands under ONE
# shared, detached app-server per CODEX_HOME (`codex app-server --listen
# unix://`, parent pid 1). That server keeps the env of whichever K2 session
# started it, so the passport a Codex tool sees ($K2_HOOK_SOCK +
# $K2_HOOK_TOKEN) can be another tab's or a dead one. K2 now launches Codex
# with --no-daemon; these checks catch tabs started before that, and say so
# instead of a bare 401. They never fall back to the owner token.

# stdin: `ps -axo pid=,ppid=,command=` rows. Succeeds when the ancestry of
# pid $1 includes a shared Codex app-server (unix listener, managed daemon,
# or detached to pid 1). A private in-process / stdio server never matches.
_cli_ancestry_has_shared_codex_server() {
    awk -v start="${1:-}" '
        {
            pid = $1; ppid = $2
            $1 = ""; $2 = ""
            sub(/^ +/, "")
            parent[pid] = ppid
            cmd[pid] = $0
        }
        END {
            p = start; n = 0; found = 0
            while (p != "" && (p + 0) > 1 && n < 64) {
                c = cmd[p]
                if (c ~ /codex/ && c ~ /app-server/ &&
                    (c ~ /--listen[ =]unix/ || c ~ /--managed-daemon/ ||
                     c ~ /app-server daemon/ || parent[p] == 1)) {
                    found = 1
                    break
                }
                p = parent[p]; n++
            }
            exit(found ? 0 : 1)
        }'
}

# True when this process runs under a shared Codex app-server. Only looks
# when a CODEX_* var is exported (Codex sets CODEX_THREAD_ID on tool runs).
_cli_under_shared_codex_server() {
    [ -n "$(compgen -e CODEX_ 2>/dev/null)" ] || return 1
    command -v ps >/dev/null 2>&1 || return 1
    ps -axo pid=,ppid=,command= 2>/dev/null | _cli_ancestry_has_shared_codex_server "$$"
}

# One loopback probe with the SCOPED token: true only when the daemon
# answers with its auth refusal (the token names no live session).
_cli_scoped_token_rejected() {
    [ -n "${SCOPED_TOKEN:-}" ] && [ -n "${BASE_URL:-}" ] || return 1
    local out _K2_CFG=""
    _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    out="$(_k2_curl "$_K2_CFG" -s --connect-timeout 3 --max-time 5 \
        "${BASE_URL}/cli/whoami" 2>/dev/null)" || return 1
    [[ "$out" == *"Invalid or missing auth token"* ]]
}

_cli_passport_stale_die() {
    local why="$1" sock="$2"
    case "$why" in
        codex_shared)
            echo "Error: this tool runs under a shared Codex app-server started by another session," >&2
            echo "so the K2 session credentials it sees are not this tab's (or no longer exist)." >&2
            echo "Restart Codex in this tab; K2 now starts Codex without the shared server (--no-daemon)." >&2
            echo "If it keeps happening, stop the shared server once: codex app-server daemon stop" >&2
            ;;
        *)
            echo "Error: this shell's K2 session credentials are stale: the session socket is gone" >&2
            echo "(${sock}) and the daemon rejects the session token." >&2
            echo "If this is a Codex tab, the tool runs under a shared Codex app-server started by" >&2
            echo "another session; restart Codex in this tab. Otherwise restart this tab." >&2
            ;;
    esac
    exit 3
}

# Before an ID/locked verb in a scoped cell: refuse a passport that is not
# this session's (shared Codex app-server) or that is dead (socket gone AND
# the daemon rejects the token). A missing socket alone is not enough — a
# failed socket bind still serves the scoped token over TCP.
_cli_check_session_passport() {
    local verb="${1:-}"
    local hook_sock="${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}"
    [ -n "$hook_sock" ] || return 0
    local tool_id mode
    tool_id="$(_cli_tool_id_for_verb "$verb")"
    mode="$(_cli_tool_default_mode "$tool_id")"
    [ "$mode" = "id" ] || return 0
    if _cli_under_shared_codex_server; then
        _cli_passport_stale_die codex_shared "$hook_sock"
    fi
    if [ ! -S "$hook_sock" ] && _cli_scoped_token_rejected; then
        _cli_passport_stale_die stale "$hook_sock"
    fi
    return 0
}
# END_CLI_STALE_PASSPORT

# DAEMON_INSTALL_EARLY_BYPASS — `k2 daemon install` provisions the
# standalone daemon on a FRESH headless box where no daemon is running
# yet, so it cannot satisfy the PORT/TOKEN connection gate below (and
# doesn't need to: it talks only to GitHub + the filesystem). Skip the
# connection gate for it; the normal router dispatches it to
# cmd_daemon_install (which is defined before the router runs).
_skip_conn_gate=0
[ "${1:-}" = "daemon" ] && [ "${2:-}" = "install" ] && _skip_conn_gate=1
# `k2 publish` P15 — split the connection gate.
# Skip only plan/CP verbs that talk to connect.k2.dev + tunnel.json
# (or print help) and do not need the workspace daemon:
#   publish (no sub / help), publish status, publish subdomain list
#   (empty action = list).
# Never skip hosting verbs: run | ps | start | stop | rm | logs.
# create/point/rm/claim/unclaim POST the local daemon (stamp) — leave
# the gate on so they fail loud without PORT/TOKEN.
if [ "${1:-}" = "publish" ]; then
    case "${2:-}" in
        ""|help|--help|-h|status)
            _skip_conn_gate=1
            ;;
        subdomain)
            case "${3:-}" in
                # create/point/rm stamp 0074 on the daemon — need PORT/TOKEN.
                # list (empty action) and help still skip.
                claim|unclaim|create|point|target|rm|remove|delete) ;;
                *) _skip_conn_gate=1 ;;
            esac
            ;;
    esac
fi
# Headless Connect account commands: login talks to Supabase first
# (no daemon needed for the account step); status/logout can run from
# disk alone. Daemon-touching steps (tunnel/config + start) check
# PORT/TOKEN themselves and fail loud with a start-daemon hint.
[ "${1:-}" = "connect" ] && _skip_conn_gate=1
# Air-gap: refuse hosted-network verbs before any curl/GitHub, even
# without a running daemon (fresh image). --help still prints.
_k2_cli_wants_help=0
for _a in "$@"; do
    case "$_a" in -h|--help) _k2_cli_wants_help=1 ;; esac
done
if [ "$_k2_cli_wants_help" != "1" ]; then
    case "${1:-}" in
        connect)
            case "${2:-}" in login) _k2_refuse_if_airgap ;; esac
            ;;
        publish)
            if [ "${2:-}" = "subdomain" ]; then
                case "${3:-}" in
                    claim|unclaim) ;;
                    *) _k2_refuse_if_airgap ;;
                esac
            fi
            ;;
        daemon)
            [ "${2:-}" = "install" ] && _k2_refuse_if_airgap
            ;;
        tunnel)
            [ "${2:-}" = "enable" ] && _k2_refuse_if_airgap
            ;;
    esac
fi
unset _k2_cli_wants_help
# `k2 --schema` (GH#8 #4) prints a STATIC command-surface manifest — it
# talks to nothing, so it must work even with no daemon running (its whole
# point is letting an agent load the CLI surface in one call).
[ "${1:-}" = "--schema" ] && _skip_conn_gate=1
[ "${1:-}" = "schema" ] && _skip_conn_gate=1
# `k2 study` prints static in-CLI pages (same skip as --schema). Live
# unknown verbs never skip PORT/TOKEN. `k2 help` is not daemon-optional.
[ "${1:-}" = "study" ] && _skip_conn_gate=1
# 0.46.7 (TC-18): `k2 explain` reads the refusal registry built into this
# file; `k2 capabilities --help` only prints help. No daemon for either.
[ "${1:-}" = "explain" ] && _skip_conn_gate=1
if [ "${1:-}" = "capabilities" ]; then
    for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 send-feedback` with no args, help or --help only prints help.
if [ "${1:-}" = "send-feedback" ]; then
    case "${2:-}" in ""|help|-h|--help) _skip_conn_gate=1 ;; esac
fi
# `k2 organization`, `k2 organization --help` and any verb's --help only
# print help (the Org Chart): no daemon.
if [ "${1:-}" = "organization" ]; then
    case "${2:-}" in ""|help|--help|-h) _skip_conn_gate=1 ;; esac
    for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 zen guide` is static pages, and `k2 zen` / `k2 zen help` / any
# `k2 zen ... --help` only print help (prd-zen-freeform-chrome FC55):
# none of them talks to the daemon. Neither do `k2 zen widget grant` and
# `revoke`, which only say there are no widget permissions (Rosson
# 2026-10-08).
if [ "${1:-}" = "zen" ]; then
    case "${2:-}" in
        ""|guide|help|--help|-h) _skip_conn_gate=1 ;;
        widget|widgets)
            case "${3:-}" in grant|allow|revoke|""|help) _skip_conn_gate=1 ;; esac
            for _a in "${@:2}"; do
                case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac
            done
            ;;
        *)
            for _a in "${@:2}"; do
                case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac
            done
            ;;
    esac
fi
# `k2 ci` / `k2 ci --help` (0.46.10, C6a) only print help: no daemon.
if [ "${1:-}" = "ci" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *)
            for _a in "${@:2}"; do
                [ "$_a" = "--" ] && break
                case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac
            done
            ;;
    esac
fi
# `k2 setup --help` (and `k2 setup help`) only print help.
if [ "${1:-}" = "setup" ]; then
    for _a in "${@:2}"; do
        case "$_a" in -h|--help|help) _skip_conn_gate=1 ;; esac
    done
fi
# `k2 sidecar` / `k2 sidecar --help` print help; `k2 sessions spawn` is
# retired and only prints where it moved. Neither talks to the daemon.
if [ "${1:-}" = "sidecar" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *)
            for _a in "${@:2}"; do
                case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac
            done
            ;;
    esac
fi
[ "${1:-}" = "sessions" ] && [ "${2:-}" = "spawn" ] && _skip_conn_gate=1
# `k2 compute` / `k2 compute --help` only print help.
if [ "${1:-}" = "compute" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 sim`, `k2 sim help` and any `k2 sim … --help` only print help.
if [ "${1:-}" = "sim" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 doc` / `k2 doc --help` / `k2 doc help` only print help.
if [ "${1:-}" = "doc" ] || [ "${1:-}" = "docs" ]; then
    case "${2:-}" in ""|help) _skip_conn_gate=1 ;; esac
    for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 changes --help` (any sub) and `k2 changes help` only print help.
if [ "${1:-}" = "changes" ]; then
    [ "${2:-}" = "help" ] && _skip_conn_gate=1
    for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 chat` / `k2 chat --help` only print help.
if [ "${1:-}" = "chat" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 voice` / `k2 voice --help` only print help.
if [ "${1:-}" = "voice" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 hostchat` / `k2 hostchat --help` only print help.
if [ "${1:-}" = "hostchat" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 app recording … --help` (0.46.11) only prints help.
if [ "${1:-}" = "app" ] && { [ "${2:-}" = "recording" ] || [ "${2:-}" = "recordings" ]; }; then
    for _a in "${@:3}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 sources` / `k2 sources --help` only print help.
if [ "${1:-}" = "sources" ] || [ "${1:-}" = "source" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 llm` / `k2 llm tokens` (alias `accounts`) / any `--help` only print help.
if [ "${1:-}" = "llm" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        tokens|token|accounts|account)
            case "${3:-}" in ""|help|--help|-h) _skip_conn_gate=1 ;; esac
            for _a in "${@:3}"; do
                case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac
            done
            ;;
    esac
fi
# `k2 vault` / `k2 vault <verb> --help` only print help (no daemon needed).
if [ "${1:-}" = "vault" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 publish secret [<verb>] --help` (and bare `k2 publish secret`) only
# print help (no daemon needed).
if [ "${1:-}" = "publish" ] && { [ "${2:-}" = "secret" ] || [ "${2:-}" = "secrets" ]; }; then
    case "${3:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:3}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 migrate` / `k2 migrate <verb> --help` only print help (no daemon needed).
if [ "${1:-}" = "migrate" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 job` / `k2 job <verb> --help` only print help (no daemon needed).
if [ "${1:-}" = "job" ] || [ "${1:-}" = "jobs" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 usage` / `k2 usage --help` only print help.
if [ "${1:-}" = "usage" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 hooks emit` (GL45) checks its own session passport and transport
# (cell socket first), and must not need the owner token's PORT/TOKEN pair.
[ "${1:-}" = "hooks" ] && [ "${2:-}" = "emit" ] && _skip_conn_gate=1
# `k2 checkin` is retired: its stub prints pointers and never calls the daemon.
[ "${1:-}" = "checkin" ] && _skip_conn_gate=1
# `k2 tickets template` prints the static brief skeleton (H42).
case "${1:-}" in
    tickets|feedback) [ "${2:-}" = "template" ] && _skip_conn_gate=1 ;;
    # W0 (0.46.0): `k2 wiki chat on` is refused locally (exit 3), never sent.
    wiki)
        [ "${2:-}" = "chat" ] && case "${3:-}" in on|enable|true|1) _skip_conn_gate=1 ;; esac
        ;;
esac

# The virtual-desktop guard (LD-10): with no passport, only the pages that
# never reach the daemon run; everything else (k2 daemon … too, which reads
# the owner token itself) is refused, saying why.
_k2_offline_verb=""
case "${1:-}" in ""|study|help|--help|-h|--schema|version|--version) _k2_offline_verb=1 ;; esac
for _a in "$@"; do case "$_a" in -h|--help) _k2_offline_verb=1 ;; esac; done
if [ -n "$_k2_no_owner_fallback" ] && [ -z "$TOKEN" ] && [ -z "$_k2_offline_verb" ]; then
    echo "Error: k2 won't act as this server's owner here." >&2
    echo "This program runs on a K2 virtual desktop (K2_NO_OWNER_FALLBACK=1) and has no session passport, so k2 doesn't read the owner token (~/.k2/daemon.token)." >&2
    echo "Use k2 from a K2 terminal tab, or from your own device signed in to this server." >&2
    exit 3
fi
if [ "$_skip_conn_gate" != "1" ] && { [ -z "$PORT" ] || [ -z "$TOKEN" ]; }; then
    echo "Error: Cannot connect to K2." >&2
    echo "Either run this from a K2 terminal, or ensure K2 is running." >&2
    echo "(Connection details: ~/.k2/heartbeat.port and ~/.k2/heartbeat.token)" >&2
    exit 1
fi
unset _skip_conn_gate

BASE_URL="http://${HOST}:${PORT}"

# HTTP helper
# 75 = local_transport_denied (unused by curl 8.x). Survives $(cli_request)
# subshells; curl 7 / connection refused stay daemon_unreachable.
_CLI_LOCAL_TRANSPORT_DENIED=75

# True when curl/OS failure is a local restriction (daemon health unobserved).
_cli_transport_is_denied() {
    local rc="$1" err="$2"
    if [[ "$err" == *[Cc]onnection\ refused* ]]; then
        return 1
    fi
    if [[ "$err" == *[Oo]peration\ not\ permitted* || "$err" == *EPERM* \
       || "$err" == *[Pp]ermission\ denied* || "$err" == *EACCES* ]]; then
        return 0
    fi
    if [[ "$err" == *Address\ family\ not\ supported* \
       || "$err" == *Protocol\ family\ not\ supported* ]]; then
        return 0
    fi
    if [[ "$err" == *"::1"* && ( "$err" == *Network\ is\ unreachable* \
       || "$err" == *No\ route\ to\ host* ) ]]; then
        return 0
    fi
    if [[ "$err" == *temporary\ file* || "$err" == *temp\ file* || "$err" == *TMPDIR* ]]; then
        return 0
    fi
    if [ "$rc" -ne 0 ]; then
        local td="${TMPDIR:-/tmp}"
        if [ ! -d "$td" ] || [ ! -w "$td" ]; then
            return 0
        fi
    fi
    return 1
}

# Body on stdout. -v so modern curl's OS errno (EPERM vs refused) is visible.
# `{ } 4>&1` + `1>&4` keeps the body on this function's stdout (so
# `resp=$(cli_request …)` works); stderr is classified, never printed.
# A leading `--k2-cfg CFG` hands curl secret config lines on stdin (_k2_curl).
_cli_curl() {
    local err rc=1 cfg="" try=1 a once=0
    if [ "${1:-}" = "--k2-cfg" ]; then
        cfg="$2"
        shift 2
    fi
    for a in "$@"; do
        case "$a" in *@-*|-T) once=1 ;; esac
    done
    local _had_e=0
    case $- in *e*) _had_e=1; set +e ;; esac
    while :; do
        {
            err=$(_k2_curl "$cfg" "$@" 2>&1 1>&4)
            rc=$?
        } 4>&1
        # k2-ptyd S5 (P47): a connect that timed out (curl 28 with no
        # "Connected to" in the -v trace) never reached the daemon: retry
        # like a refused one. A timeout after connecting is never retried.
        if [ "$rc" -eq 28 ] && [[ "$err" != *"Connected to"* ]] && [ "$once" -eq 0 ] && [ "$try" -lt "$_K2_CURL_TRIES" ]; then
            try=$((try + 1))
            sleep 1
            continue
        fi
        break
    done
    [ "$_had_e" -eq 1 ] && set -e
    if [ "$rc" -eq 0 ]; then
        return 0
    fi
    if _cli_transport_is_denied "$rc" "$err"; then
        return "$_CLI_LOCAL_TRANSPORT_DENIED"
    fi
    return "$rc"
}

# k2-ptyd S5 (P47): the daemon's pre-ready 503 (`"state":"migrating"`):
# no handler ran, so the request may be sent again. Security review F4: the
# HTTP status decides, never the body alone — a 200 whose body happens to
# contain that text is final. The request functions append the status
# (`-w "$_K2_WSTATUS"`) and `_cli_split_status` strips it off again, so the
# bytes callers see are unchanged.
_K2_WSTATUS=$'\n__k2http:%{http_code}'
# Strip the appended status from the caller's `out` into `_K2_HTTP` (the
# last one: a refused try that `_k2_curl` retried wrote its own `000`, so
# every marker goes).
_cli_split_status() {
    _K2_HTTP=""
    case "$out" in
        *$'\n__k2http:'*)
            _K2_HTTP="${out##*$'\n__k2http:'}"
            out="${out//$'\n__k2http:'[0-9][0-9][0-9]/}"
            ;;
    esac
}
_cli_migrating() { [ "${_K2_HTTP:-}" = 503 ] && [[ "$1" == *'"state":"migrating"'* ]]; }

cli_request() {
    local endpoint="$1"
    shift
    local params="token=${TOKEN}&project=$(urlencode "$PROJECT")"
    for param in "$@"; do
        params="${params}&${param}"
    done
    local _K2_CFG="" out rc=0 try=1
    _k2_cfg_add data "$params"
    # Captured (with a sentinel, so trailing newlines survive) only so the
    # permission-cards closing line can be noted; the bytes are unchanged.
    while :; do
        rc=0
        out="$(r=0; _cli_curl --k2-cfg "$_K2_CFG" -sS -v -G "${BASE_URL}${endpoint}" -w "$_K2_WSTATUS" \
            --connect-timeout 5 --max-time 30 || r=$?; printf x; exit "$r")" || rc=$?
        out="${out%x}"
        _cli_split_status
        if [ "$rc" -eq 0 ] && _cli_migrating "$out" && [ "$try" -lt "$_K2_CURL_TRIES" ]; then
            try=$((try + 1)); sleep 1; continue
        fi
        break
    done
    if [ "$rc" -eq 0 ] && _cli_retry_with_healed_passport "$out"; then
        cli_request "$endpoint" "$@"
        return $?
    fi
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

# 0.44.4 — POST twin of cli_request for routes that change state (the
# daemon answers GET with 405 on them). Same contract: pre-urlencoded
# `key=value` args ride the query string, the body is empty, and transport
# errors classify exactly like cli_request (_cli_curl).
#
# A pre-0.44.4 daemon (e.g. a remote host via `--host`) only routes these
# paths as GET and answers the POST with its top-level
# `method not allowed for this route` 405 before any handler runs, so the
# same request is replayed once as GET there (side-effect safe).
cli_request_post() {
    local endpoint="$1"
    shift
    local params="token=${TOKEN}&project=$(urlencode "$PROJECT")"
    for param in "$@"; do
        params="${params}&${param}"
    done
    local out rc=0 _K2_CFG="" try=1
    _k2_cfg_add url "${BASE_URL}${endpoint}?${params}"
    while :; do
        rc=0
        out="$(_cli_curl --k2-cfg "$_K2_CFG" -sS -v -X POST -w "$_K2_WSTATUS" \
            --connect-timeout 5 --max-time 30 \
            --data-raw "")" || rc=$?
        _cli_split_status
        if [ "$rc" -eq 0 ] && _cli_migrating "$out" && [ "$try" -lt "$_K2_CURL_TRIES" ]; then
            try=$((try + 1)); sleep 1; continue
        fi
        break
    done
    if [ "$rc" -eq 0 ] && [[ "$out" == *'method not allowed for this route'* ]]; then
        cli_request "$endpoint" "$@"
        return $?
    fi
    if [ "$rc" -eq 0 ] && _cli_retry_with_healed_passport "$out"; then
        cli_request_post "$endpoint" "$@"
        return $?
    fi
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

# UDS twin of cli_request_post (cli_send picks `<fn>_uds`).
cli_request_post_uds() {
    local endpoint="$1"; shift
    local params=""
    for param in "$@"; do params="${params}${params:+&}${param}"; done
    local _K2_CFG="" out rc=0 try=1
    _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    while :; do
        rc=0
        out="$(r=0; _cli_curl --k2-cfg "$_K2_CFG" -sS -v -X POST -w "$_K2_WSTATUS" --unix-socket "$(_hook_sock)" "http://localhost${endpoint}${params:+?${params}}" \
            --connect-timeout 5 --max-time 30 \
            --data-raw "" || r=$?; printf x; exit "$r")" || rc=$?
        out="${out%x}"
        _cli_split_status
        if [ "$rc" -eq 0 ] && _cli_migrating "$out" && [ "$try" -lt "$_K2_CURL_TRIES" ]; then
            try=$((try + 1)); sleep 1; continue
        fi
        break
    done
    if [ "$rc" -eq 0 ] && _cli_retry_with_healed_passport "$out"; then
        cli_request_post_uds "$endpoint" "$@"
        return $?
    fi
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

# POST a JSON body. Used by /cli/awareness/publish (Phase 3).
# Token rides in the query string (the daemon parses it there
# before looking at the body). Caller supplies the full endpoint
# and the JSON body on stdin via a pipe, or as the second arg.
cli_post_json() {
    local endpoint="$1"
    local body="${2:-}"
    if [ -z "$body" ]; then
        body="$(cat)"
    fi
    # Bodies carry passwords (k2 users add, k2 app user password): config too.
    local _K2_CFG="" out rc=0
    _k2_cfg_add url "${BASE_URL}${endpoint}?token=${TOKEN}"
    _k2_cfg_add data-raw "$body"
    # 0.46.6 (client papercut 12): note the refusal's (or hold's) `permission`
    # object, so a held `k2 publish run` ends with its Ask-with line.
    out="$(r=0; _k2_curl "$_K2_CFG" -s -X POST \
        --connect-timeout 5 --max-time 30 \
        -H "Content-Type: application/json" 2>/dev/null || r=$?; printf x; exit "$r")" || rc=$?
    out="${out%x}"
    if [ "$rc" -eq 0 ] && _cli_retry_with_healed_passport "$out"; then
        cli_post_json "$endpoint" "$body"
        return $?
    fi
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

# POST a JSON body read from STDIN (`--data-binary @-`). For bodies that
# carry a secret (API keys): the body never sits on any process's argv
# (argv is readable by every OS user via `ps`). The URL, with the token,
# reaches curl as a config line on fd 3, so it is off curl's argv too.
# Same wire request as cli_post_json.
cli_post_json_stdin() {
    local endpoint="$1"
    if [ -n "$_K2_WINDOWS" ]; then
        # Git for Windows' curl is a native program: it can't open bash's
        # /dev/fd/3 (curl exit 26, nothing printed). cli_post_json reads
        # the body from stdin and hands curl the URL and body as config
        # on its stdin, so neither the token nor the body is on argv.
        cli_post_json "$endpoint"
        return
    fi
    local url="${BASE_URL}${endpoint}?token=${TOKEN}"
    url="${url//\\/\\\\}"
    url="${url//\"/\\\"}"
    curl -s -K /dev/fd/3 -X POST \
        --connect-timeout 5 --max-time 30 \
        -H "Content-Type: application/json" \
        --data-binary @- 2>/dev/null 3<<<"url = \"${url}\""
}

# Read one secret from stdin into _K2_SECRET: a hidden prompt when stdin
# is a terminal (`read -rs`), else one line from the pipe. Never echoed.
_cli_read_secret_stdin() {
    local prompt="${1:-Paste the key: }"
    _K2_SECRET=""
    if [ -t 0 ]; then
        printf '%s' "$prompt" >&2
        IFS= read -rs _K2_SECRET || _K2_SECRET=""
        echo >&2
    else
        IFS= read -r _K2_SECRET || true
    fi
}

# One-line notice for the old key-on-the-command-line forms.
_cli_warn_key_on_argv() {
    echo "warning: passing a key on the command line is deprecated and will be refused in the next release; use --key-stdin" >&2
}

urlencode() {
    python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$1" 2>/dev/null || echo "$1"
}

# Absolute form of a path whose parents may not exist yet. Resolves the
# deepest folder that does exist and keeps the missing tail as typed.
# The old `$(cd "$(dirname p)" && pwd)/$(basename p)` form returned
# "/<basename>" when the parent was missing: `k2 workspace create
# /home/k2/ai/hearth` with no ~/ai asked the daemon for /hearth, which
# failed with "Permission denied (os error 13)" (2026-10-16).
cli_abs_path() {
    local p="$1"
    [ -n "$p" ] || return 0
    case "$p" in /*) ;; *) p="$PWD/$p" ;; esac
    while [ "$p" != "/" ] && [ "${p%/}" != "$p" ]; do p="${p%/}"; done
    local head="$p" tail="" base
    while [ ! -d "$head" ]; do
        tail="/$(basename "$head")$tail"
        head="$(dirname "$head")"
    done
    base="$(cd "$head" 2>/dev/null && pwd)" || base="$head"
    [ "$base" = "/" ] && base=""
    printf '%s%s\n' "${base}" "${tail}"
}

# 0.40.24 (B4): true when any argument is --help/-h. Used to intercept
# help requests on every workspace/connections/agent subcommand BEFORE
# the args reach a handler — `k2 workspace create --help` used to
# literally mkdir + register a workspace named `--help`.
_wants_help() {
    local a
    for a in "$@"; do
        case "$a" in
            --help|-h) return 0 ;;
        esac
    done
    return 1
}

# POST a form-encoded body (0.39.45, GH #35/#37/#29). Token + project
# ride the query string (the daemon's auth gate parses the query);
# everything else — message text, inbox title/body — rides the POST
# body so long payloads dodge the daemon's 16KB request-head cap that
# used to silently truncate query-string values at ~2.7KB. Callers pass
# pre-urlencoded `key=value` pairs, same contract as cli_request.
cli_post_form() {
    local endpoint="$1"
    shift
    local body=""
    for param in "$@"; do
        body="${body}${body:+&}${param}"
    done
    local _K2_CFG=""
    _k2_cfg_add url "${BASE_URL}${endpoint}?token=${TOKEN}&project=$(urlencode "$PROJECT")"
    _k2_cfg_add data-raw "$body"
    _k2_curl "$_K2_CFG" -s -X POST \
        --connect-timeout 5 --max-time 30 \
        -H "Content-Type: application/x-www-form-urlencoded" 2>/dev/null
}

