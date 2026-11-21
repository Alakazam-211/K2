# ── CLI tool identity policy ──
# The CLI's own table: which catalog tool a command is, whether that tool
# sends the session's passport in a K2 terminal (id) or not (open), and
# which tools are locked (never open). Nothing else holds a copy, and no
# setting overrides it. The daemon still decides every request. The pure
# helpers below are unit-testable via the BEGIN/END markers (see tests/cli/).
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
        connections|reserve|release) echo "connections" ;;
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
        # 0.46.17: `k2 focus-group` is an `id` tool and locked, so inside a K2
        # terminal it sends the session passport (an agent lists; its
        # changes are refused and asked for with a card), never the owner token.
        focus-group|focus-groups) echo "focus-groups" ;;
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
        # k2 remote logout ends this computer's session on another server
        # with a Connect token the person holds: the same locked tool.
        remote) echo "connect-account" ;;
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

# The tool id for a whole `k2 <verb> <args...>`: the first argument's, as
# above, or, when that maps to no tool, the id of the first word after any
# leading flags (and the values of the flags that take one). A family whose
# parser reads flags before its verb (`k2 setup --json chief-of-staff`)
# then names the same tool as without them, so a locked tool stays locked
# when flags come before its name. Only ever the stricter of the two: a
# first argument that names a tool keeps it. A help flag ends the search
# (help stays help).
_cli_tool_id_for_args() {
    local verb="${1:-}" id
    [ $# -gt 0 ] && shift
    id="$(_cli_tool_id_for_verb "$verb" "${1:-}")"
    if [ -z "$id" ]; then
        while [ $# -gt 0 ]; do
            case "$1" in
                -h|--help) break ;;
                --) shift; break ;;
                -*)
                    # The flags k2 setup reads with a value.
                    case "$verb:$1" in
                        setup:--tool|setup:--name|setup:--handle|setup:--about|setup:--focus-group)
                            [ $# -gt 1 ] && shift ;;
                    esac
                    shift ;;
                *) break ;;
            esac
        done
        case "${1:-}" in
            -h|--help) ;;
            *) [ $# -gt 0 ] && id="$(_cli_tool_id_for_verb "$verb" "$1")" ;;
        esac
    fi
    printf '%s\n' "$id"
}

# Default identity mode for a tool id: open | id.
# Locked production tools are always id (cannot be Open in the catalog).
_cli_tool_default_mode() {
    case "${1:-}" in
        help|schema|whats-new|checkin|activity|read|skills|wiki|tools-policy|study|"") echo "open" ;;
        msg|inbox|respond|awareness|connections|heartbeat|sidecar|project|organization) echo "id" ;;
        mail|db|store|dns|publish|connect-account|sessions-spawn|fs|daemon-admin|users-admin|api-key|skin|remote-session) echo "id" ;;
        browser-open|settings|llm-accounts|compute|vault|sources|agent-teams|agent-identity|agent-harness|changes|usage|hostchat|jobs|sim|desktop|chat|migrate|docs|focus-groups) echo "id" ;;
        *) echo "open" ;;
    esac
}

# True when the tool is a locked production floor (cannot be set Open).
_cli_tool_is_locked() {
    case "${1:-}" in
        mail|db|store|dns|publish|connect-account|sessions-spawn|fs|daemon-admin|users-admin|api-key|skin|remote-session|llm-accounts|compute|vault|sources|agent-teams|agent-identity|agent-harness|changes|usage|hostchat|jobs|sim|desktop|chat|migrate|docs|focus-groups) return 0 ;;
        *) return 1 ;;
    esac
}

# Teaching error when an ID/locked verb has no session passport in-cell.
_cli_require_session_passport() {
    local tool_id="${1:-unknown}"
    echo "Error: tool '${tool_id}' requires session ID (passport)." >&2
    echo "Run it from a K2 agent session that carries its passport." >&2
    echo "No setting turns this off; locked production tools always need one." >&2
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
    local verb="${1:-}"
    local tool_id mode
    tool_id="$(_cli_tool_id_for_args "$@")"
    mode="$(_cli_tool_default_mode "$tool_id")"
    local hook_sock="${K5_HOOK_SOCK:-${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}}"

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
        if [ -n "$hook_sock" ] && _cli_verb_sends_session_passport "$verb" "${2:-}"; then
            # In a session these send the session's own passport, or no
            # credential when it has none: never the owner token from disk.
            TOKEN="${SCOPED_TOKEN:-}"
        elif [ -n "$hook_sock" ] && _cli_tickets_sends_passport "$@"; then
            # Tickets: K2's server takes the passport as this agent (its own
            # workspace and tickets). With none, refuse here: never the owner.
            if [ -z "${SCOPED_TOKEN:-}" ]; then
                echo "Error: k2 tickets in a K2 session sends this session's passport, and the session has none, so nothing was sent." >&2
                exit 3
            fi
            TOKEN="$SCOPED_TOKEN"
        elif [ -n "$hook_sock" ] && [ -n "${DISK_OWNER_TOKEN:-}" ]; then
            # Open / ungoverned: keep COMPAT-58 inversion for TCP ergonomics.
            TOKEN="$DISK_OWNER_TOKEN"
        fi
    fi
}

# Open verbs that still act as the caller in an agent's session: the
# workspace verbs that list, create, open, start or change a workspace and
# its agent profile (`k2 agent profile|update` are the same commands), and
# `workspace triage`, which lists an inbox.
# Outside a session they send $TOKEN, as every Open verb does.
_cli_verb_sends_session_passport() {
    case "${1:-} ${2:-}" in
        "workspace list"|"workspace create"|"workspace open"|"workspace launch"|"workspace profile"|"workspace update"|"workspace triage") return 0 ;;
        "agent profile"|"agent update") return 0 ;;
    esac
    return 1
}

# `k2 tickets ask|list|show|comment|resolve|assign|settings` (alias
# `feedback`): in an agent's session they send the session's passport,
# never the owner token. Help and `template` make no request.
_cli_tickets_sends_passport() {
    case "${1:-}" in tickets|feedback) ;; *) return 1 ;; esac
    case "${2:-}" in ask|list|show|comment|resolve|assign|settings) ;; *) return 1 ;; esac
    shift 2
    ! _wants_help "$@"
}

# The credential project membership changes ride from verbs that are not
# themselves `id` tools (`k2 agent hire|set --project`): the session passport
# inside a K2 terminal (so the agent's "Manage K2 projects" switch applies),
# else $TOKEN (owner / external terminal).
_k2_project_token() {
    if [ -n "${K5_HOOK_SOCK:-${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}}" ] && [ -n "${SCOPED_TOKEN:-}" ]; then
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

# Git Bash (Windows): curl there is a native Windows program. Git Bash
# converts the paths in curl's argv, but not the ones in the config curl
# reads from stdin, so a `data-binary = "@/tmp/…"` body file isn't found
# (curl exit 26) unless it is spelled the Windows way (cygpath -w).
_K2_CYGPATH=""
case "${OSTYPE:-}" in
    msys*|cygwin*) command -v cygpath >/dev/null 2>&1 && _K2_CYGPATH=1 ;;
esac

# _k2_cfg_add NAME VALUE — append `NAME = "VALUE"` to the caller's
# `local _K2_CFG` (bash dynamic scope; no subshell).
_k2_cfg_add() {
    local v="$2"
    if [ -n "$_K2_CYGPATH" ] && [ "$1" = data-binary ] && [ "${v#@/}" != "$v" ]; then
        v="@$(cygpath -w "${v#@}")"
    fi
    _k2_cq "$v"
    _K2_CFG="${_K2_CFG}${1} = ${_K2_CQ}"$'\n'
}

# _k2_cfg_auth TOKEN: the credential of a daemon request, as an
# `Authorization: Bearer` line in the caller's _K2_CFG (curl config on
# stdin, never argv). The CLI never puts a credential in the query string.
# The daemon reads the header the same way it reads `?token=` (it folds the
# Bearer into the query before any route runs), and still accepts
# `?token=` from older clients. An empty TOKEN adds nothing.
_k2_cfg_auth() {
    [ -n "${1:-}" ] || return 0
    _k2_cfg_add header "Authorization: Bearer $1"
}

# A Codex call (Codex sets CODEX_THREAD_ID on every one) while the switch
# K2_CODEX_PASSPORT_BIND is on (the default; 0 turns it off). See
# BEGIN_CLI_CODEX_TAB_BIND for what the switch governs.
_cli_codex_bind_on() {
    [ -n "${CODEX_THREAD_ID:-}" ] || return 1
    case "${K2_CODEX_PASSPORT_BIND:-}" in 0|off|false|no) return 1 ;; esac
    return 0
}

# The conversation headers of a Codex call while the switch is on, for
# every daemon request (_k2_curl adds them to curl's config, never argv; the
# embedded Python gets them with _k5_py_headers): X-K2-Conversation is the
# conversation's root thread (CODEX_SESSION_ID: a subagent's call names its
# parent conversation), X-K2-Thread the thread that made the call
# (CODEX_THREAD_ID). An id that isn't letters, digits, '-' and '_' (at most
# 128) is not sent. Sets the array _K2_CODEX_HDRS, one `Name: value` each.
_K2_CODEX_ID_RE='^[A-Za-z0-9_-]{1,128}$'
_k2_codex_headers() {
    _K2_CODEX_HDRS=()
    _cli_codex_bind_on || return 0
    local root="${CODEX_SESSION_ID:-}" thread="${CODEX_THREAD_ID:-}"
    if [[ "$root" =~ $_K2_CODEX_ID_RE ]]; then _K2_CODEX_HDRS+=("X-K2-Conversation: $root"); fi
    if [[ "$thread" =~ $_K2_CODEX_ID_RE ]]; then _K2_CODEX_HDRS+=("X-K2-Thread: $thread"); fi
    return 0
}
# The same headers as curl config lines in _K2_CODEX_CFG ("" for none).
_k2_codex_cfg() {
    local _K2_CFG="" h
    _k2_codex_headers
    if [ "${#_K2_CODEX_HDRS[@]}" -gt 0 ]; then
        for h in "${_K2_CODEX_HDRS[@]}"; do _k2_cfg_add header "$h"; done
    fi
    _K2_CODEX_CFG="$_K2_CFG"
}

# The hand-over headers. A call the Rust front handed to this script
# (K5_CLI_VIA=rust, read into _K5_CLI_VIA by the prelude) says so on every
# request to the daemon, over the cell socket and TCP alike, from curl
# (_k2_curl) and from the embedded Python (_K5_PY_HTTP) alike, in this
# order:
#   X-K5-Would-Refuse: 1       only when the front's passport rule would
#                              refuse the call (K5_CLI_WOULD_REFUSE=1, read
#                              into _K5_WOULD_REFUSE)
#   X-K5-Cli-Via: fallthrough  always
#   X-K5-Cli-Noun: <noun>      always: the router's top-level word, as
#                              typed (_K5_CLI_NOUN). `other` when it is not
#                              a-z then up to 31 of a-z, 0-9 and -, or the
#                              router has not read it yet
#   X-K5-Owner-Source: disk    only when the call has no passport
#                              (SCOPED_TOKEN empty) and runs on the owner
#                              token read from disk (DISK_OWNER_TOKEN);
#                              never with a passport
# Without _K5_CLI_VIA=rust, none of them. They are not secrets, so they
# ride argv. The daemon reads headers by name and ignores the ones it
# doesn't know. _k5_headers sets the array _K5_HDRS to them, one
# `Name: value` per element.
_K5_NOUN_RE='^[abcdefghijklmnopqrstuvwxyz][abcdefghijklmnopqrstuvwxyz0123456789-]{0,31}$'
_k5_headers() {
    _K5_HDRS=()
    [ "${_K5_CLI_VIA:-}" = "rust" ] || return 0
    local noun="${_K5_CLI_NOUN:-}"
    [[ "$noun" =~ $_K5_NOUN_RE ]] || noun=other
    if [ "${_K5_WOULD_REFUSE:-}" = "1" ]; then
        _K5_HDRS+=("X-K5-Would-Refuse: 1")
    fi
    _K5_HDRS+=("X-K5-Cli-Via: fallthrough" "X-K5-Cli-Noun: ${noun}")
    if [ -z "${SCOPED_TOKEN:-}" ] && [ -n "${DISK_OWNER_TOKEN:-}" ] && [ "${TOKEN:-}" = "$DISK_OWNER_TOKEN" ]; then
        _K5_HDRS+=("X-K5-Owner-Source: disk")
    fi
}
# The same headers, one per line, for a python3 command line.
_k5_py_headers() {
    _k5_headers
    if [ "${#_K5_HDRS[@]}" -gt 0 ]; then printf '%s\n' "${_K5_HDRS[@]}"; fi
    # A Codex call's conversation headers (_k2_codex_headers).
    _k2_codex_headers
    if [ "${#_K2_CODEX_HDRS[@]}" -gt 0 ]; then printf '%s\n' "${_K2_CODEX_HDRS[@]}"; fi
}
# The embedded Python's side. A python3 block that calls urlopen gets this
# text and the headers on its own command line, never exported, runs the
# text first and wraps every request in k5_request():
#   _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="$(_k5_py_headers)" python3 - <<'PY'
#   import os, urllib.request
#   exec(os.environ.pop("_K5_PY_HTTP"))
#   ... urllib.request.urlopen(k5_request(req), timeout=30) ...
# A block that talks to something other than the daemon passes
# _K5_PY_HEADERS="" instead. Both are taken out of the Python's
# environment as it reads them, so nothing it starts inherits them.
# tests/cli/k5_urlopen_gate.sh fails on a urlopen in cli/src that skips
# any of this.
_K5_PY_HTTP='
import os as _k5_os, urllib.request as _k5_urlreq
k5_headers = [tuple(h.split(": ", 1)) for h in _k5_os.environ.pop("_K5_PY_HEADERS").split("\n") if h]
def k5_request(req):
    """req (a urllib Request, or a URL) with the hand-over headers on it."""
    if isinstance(req, str):
        req = _k5_urlreq.Request(req)
    for name, value in k5_headers:
        req.add_header(name, value)
    return req
'

# _k2_curl CFG curl-args… — run curl with CFG (config lines) on stdin.
# CFG empty → plain curl. The caller's stdin is NOT available to curl.
#
# k2-ptyd S5 (PRD P47, P49): a daemon restart must not fail an agent's
# `k2` call. curl exit 7 (nothing accepted the connection: the daemon is
# between processes and nothing held its socket) is retried, at most
# $_K2_CURL_TRIES tries, 1 s apart; such a request never reached a
# handler, so a retry can't run it twice. Never `curl --retry` (it also
# retries after a response). A request that reads stdin (an argument
# that is exactly `@-`, as in `--data-binary @-`, or `-T`) is not
# retried: the first try consumed it. An `@-` inside a URL or another
# value is not stdin and keeps the retries. A refused
# loopback request on $PORT also tries the port the daemon published in
# ~/.k2/daemon.port, when that one differs (the daemon came back on
# another port).
#
# Every daemon request also carries the hand-over headers (_k5_headers),
# first on the argv, in their order. A caller that talks to something
# other than the daemon sets _K2_CURL_NOT_DAEMON=1 for that call: no
# hand-over headers (the proxy rule below still applies).
#
# Loopback (127.0.0.1, ::1, localhost) and plain http never use a proxy;
# HTTPS to a remote K2 may use the person's proxy (a CONNECT tunnel hides
# the request; a company proxy that intercepts TLS is the organisation's
# own policy). So a daemon call to the loopback port, the cell socket or
# any http:// address gets `--noproxy '*'` (http_proxy, https_proxy,
# ALL_PROXY and their other spellings are not used for it); an https://
# call to a remote K2 server keeps the person's proxy settings
# (_k2_direct_url decides, from the URL argument or the config's url line).
# It is not a secret, so it rides argv. The rule holds for every call,
# _K2_CURL_NOT_DAEMON ones too (the K2 Connect control plane on loopback
# or plain http never goes through a proxy either).
#
# _k2_direct_url URL: true when a request to URL must skip every proxy
# setting. Only https:// to a host that is not loopback may use one; a
# call with no URL found skips the proxy too. A 127 address counts only
# as four numbers (_k2_url_is_literal_loopback), so 127.example.com is a
# name and may use the proxy, as the Python and Rust rules say.
_k2_direct_url() {
    local h
    case "$1" in
        https://*|HTTPS://*) ;;
        *) return 0 ;;
    esac
    h="${1#*://}"; h="${h%%/*}"; h="${h%%\?*}"; h="${h##*@}"
    case "$h" in
        "[::1]"|"[::1]:"*) return 0 ;;
    esac
    h="${h%%:*}"; h="${h%.}"
    h="$(printf '%s' "$h" | tr '[:upper:]' '[:lower:]')"
    case "$h" in
        localhost|*.localhost) return 0 ;;
    esac
    _k2_url_is_literal_loopback "https://$h"
}
# _k2_url_is_literal_loopback URL: true when URL's host, AS WRITTEN, is
# this computer: an IPv4 address in 127.0.0.0/8 written as four numbers,
# [::1], or the name localhost. A name is never resolved (a name that
# resolves to loopback today may not tomorrow: DNS rebinding), so
# x.localhost, 127.0.0.1.example and any other name are not loopback.
# The scheme may be anything or missing.
_k2_url_is_literal_loopback() {
    local h="${1#*://}" re='^127\.(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])(\.(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])){2}$'
    h="${h%%/*}"; h="${h%%\?*}"; h="${h%%#*}"; h="${h##*@}"
    case "$h" in
        "[::1]"|"[::1]:"*) return 0 ;;
    esac
    h="${h%%:*}"
    h="$(printf '%s' "$h" | tr '[:upper:]' '[:lower:]')"
    [ "$h" = localhost ] && return 0
    [[ "$h" =~ $re ]]
}
# _k2_daemon_base VAR: sets VAR to the URL of the daemon at HOST:PORT.
# HOST is a name or address (plain http), or https://<name> (https: K2's
# TLS or K2 Connect in front of the daemon).
_k2_daemon_base() {
    local h="${HOST:-127.0.0.1}"
    case "$h" in
        https://*|HTTPS://*) h="${h#*://}"; printf -v "$1" 'https://%s:%s' "${h%/}" "${PORT:-}" ;;
        *) printf -v "$1" 'http://%s:%s' "$h" "${PORT:-}" ;;
    esac
}
# _k2_daemon_is_plain_remote: true when the daemon at HOST is reached over
# plain http and HOST, as written, isn't this computer
# (_k2_url_is_literal_loopback; a bare ::1 counts as [::1]).
_k2_daemon_is_plain_remote() {
    local h="${HOST:-127.0.0.1}"
    case "$h" in https://*|HTTPS://*) return 1 ;; esac
    case "$h" in \[*) ;; *:*) h="[$h]" ;; esac
    ! _k2_url_is_literal_loopback "http://$h"
}
# _k2_password_transport_ok: a password never goes over plain http to
# another computer (_k2_daemon_is_plain_remote), whether it came from a
# flag, a prompt or a pipe. The password-flag check below calls it, and so
# does each command that sends a typed or piped password, PIN, API key or
# secret value to the daemon, before it asks or reads, so nobody types one
# that is then refused. Not the prompts whose answer goes only to this
# computer's vault socket (the vault confirms, k2 vault send --stdin,
# confirm-password set, restore): nothing of theirs travels over http.
# Refused: the one line below on stderr and exit 2. K2_PLAIN_HTTP_OK=1
# and a tailnet host don't change it.
_k2_password_transport_ok() {
    _k2_daemon_is_plain_remote || return 0
    local base
    _k2_daemon_base base
    echo "Error: a password never goes over plain http:// to another computer; use K2_HOST=https://<name> (got: ${base})" >&2
    exit 2
}
# _k2_host_is_tailnet_literal HOST: true when HOST, AS WRITTEN (never
# resolved), is a Tailscale address: IPv4 in 100.64.0.0/10 as four
# numbers, IPv6 in fd7a:115c:a1e0::/48, or a name ending in .ts.net.
_k2_host_is_tailnet_literal() {
    local h re='^100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])(\.(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])){2}$'
    h="$(printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]')"
    h="${h#\[}"; h="${h%\]}"
    case "$h" in
        fd7a:115c:a1e0:*) return 0 ;;
        ?*.ts.net) return 0 ;;
    esac
    [[ "$h" =~ $re ]]
}
# _k2_refuse_plain_http URL: a token goes over plain http only to this
# computer. Run it before a request that carries a token to a URL that
# isn't this computer's daemon, before the token is even looked up. https
# passes; http, or no scheme (curl guesses http), passes only to a literal
# loopback host (_k2_url_is_literal_loopback). Anything else prints
#   Error: plain http:// only to this computer; use https:// (got: <url>)
# and returns 2. Proxied or not: a proxy doesn't make plain http private.
_k2_refuse_plain_http() {
    local scheme="${1%%://*}"
    [ "$scheme" != "$1" ] || scheme=""
    scheme="$(printf '%s' "$scheme" | tr '[:upper:]' '[:lower:]')"
    [ "$scheme" = https ] && return 0
    _k2_url_is_literal_loopback "$1" && return 0
    echo "Error: plain http:// only to this computer; use https:// (got: $1)" >&2
    return 2
}
# _K2_PY_PROXY_RULE: the same rule for the CLI's embedded Python. Every
# Python block that calls the daemon runs exec(os.environ["_K2_PY_PROXY_RULE"])
# first: it installs one urllib opener that skips the proxy for a direct
# URL (k2_direct, the same answers as _k2_direct_url and
# k2_client::proxy_rule::direct; tests/cli/proxy_rule_parity.sh holds the
# table) and uses the person's proxy settings for https to a remote host.
# The same opener follows no redirect: every request it makes carries a
# token (some a password too), so a 3xx is refused with a plain line and
# exit 3, and nothing goes to the place it names (k2_redirect_refused; a
# block may define its own after running the rule, as k2 clone does).
# Exported so every python3 the CLI starts can read it. Not a secret.
_K2_PY_PROXY_RULE='
import ipaddress as _k2_ip, sys as _k2_sys, urllib.parse as _k2_up, urllib.request as _k2_ur

def k2_direct(url):
    # Loopback (127.0.0.1, ::1, localhost) and plain http never use a proxy;
    # HTTPS to a remote K2 may use the person'"'"'s proxy.
    try:
        u = _k2_up.urlsplit(url)
        host = u.hostname
    except ValueError:
        return True
    if u.scheme != "https" or not host:
        return True
    host = host.rstrip(".")
    if host == "localhost" or host.endswith(".localhost"):
        return True
    try:
        return _k2_ip.ip_address(host).is_loopback
    except ValueError:
        return False

class _K2ProxyRule(_k2_ur.ProxyHandler):
    # The person'"'"'s proxy settings (ProxyHandler reads them), except for a
    # direct URL: returning None hands the request to the plain handlers.
    def proxy_open(self, req, proxy, type):
        if k2_direct(req.full_url):
            return None
        return super().proxy_open(req, proxy, type)

def k2_redirect_refused(line):
    # The line on stderr and exit 3. SystemExit, not an Exception, so a
    # block that turns errors into daemon_unreachable can'"'"'t hide it.
    _k2_sys.stderr.write(line + "\n")
    _k2_sys.stderr.flush()
    raise SystemExit(3)

class _K2NoRedirect(_k2_ur.HTTPRedirectHandler):
    # No request follows a redirect: the new location would get its token.
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        fp.close()
        here = _k2_up.urlsplit(req.full_url).netloc.rpartition("@")[2]
        there = _k2_up.urlsplit(newurl).netloc.rpartition("@")[2] or newurl
        there = "".join(c if c.isprintable() else "?" for c in there)
        k2_redirect_refused("refused: %s answered %s, sending K2 to %s; K2 does not follow a redirect with your token or password, so nothing was sent there" % (here, code, there))

_k2_ur.install_opener(_k2_ur.build_opener(_K2ProxyRule(), _K2NoRedirect()))
'
export _K2_PY_PROXY_RULE
# A secret for an embedded Python script that is itself read from stdin (a
# heredoc) rides stdin ahead of the script, never argv or the environment (ps
# shows a command line to every account on this computer, and the environment
# to this user's processes):
#   { _k2_stdin_secrets NAME "$value" ...; cat; } <<'PY' | VAR=… python3 -c "$_K2_PY_STDIN_BOOT" args…
# stdin is NAME NUL value NUL for each pair, one more NUL, then the script.
# The script reads the values from the dict K2_STDIN_SECRETS. Bash values
# can't hold a NUL, so the split is exact. Not a secret itself.
_K2_PY_STDIN_BOOT='
import sys
_b = sys.stdin.buffer.read()
K2_STDIN_SECRETS = {}
while True:
    _k, _s, _b = _b.partition(b"\0")
    if not _s:
        sys.exit("k2: the embedded script did not arrive on stdin")
    if not _k:
        break
    _v, _s, _b = _b.partition(b"\0")
    K2_STDIN_SECRETS[_k.decode()] = _v.decode("utf-8", "surrogateescape")
exec(compile(_b, "<stdin>", "exec"), {"__name__": "__main__", "__builtins__": __builtins__, "K2_STDIN_SECRETS": K2_STDIN_SECRETS})
'
_k2_stdin_secrets() {
    while [ $# -ge 2 ]; do
        printf '%s\0%s\0' "$1" "$2"
        shift 2
    done
    printf '\0'
}
_K2_CURL_TRIES=4
_k2_curl() {
    local cfg="$1"; shift
    local try=1 rc once=0 a i hdr_file="" hdr_arg
    for a in "$@"; do
        case "$a" in @-|-T) once=1 ;; esac
    done
    local url="" l
    for a in "$@"; do
        case "$a" in http://*|https://*|HTTP://*|HTTPS://*) url="$a"; break ;; esac
    done
    if [ -z "$url" ] && [ -n "$cfg" ]; then
        while IFS= read -r l; do
            case "$l" in 'url = "'*) url="${l#url = \"}"; break ;; esac
        done <<< "$cfg"
    fi
    if _k2_direct_url "$url"; then
        set -- --noproxy '*' "$@"
    fi
    if [ -z "${_K2_CURL_NOT_DAEMON:-}" ]; then
        # A Codex call's conversation headers ride the config, never argv.
        # A request whose body is stdin with no config gets them from a
        # config file of its own (stdin is the body).
        _k2_codex_cfg
        if [ -n "$_K2_CODEX_CFG" ]; then
            if [ -n "$cfg" ]; then
                case "$cfg" in *$'\n') ;; *) cfg="${cfg}"$'\n' ;; esac
                cfg="${cfg}${_K2_CODEX_CFG}"
            elif [ "$once" -eq 0 ]; then
                cfg="$_K2_CODEX_CFG"
            else
                hdr_file="$(mktemp "${TMPDIR:-/tmp}/k2-hdr.XXXXXX")" || {
                    echo "Error: could not write the request headers to a temp file; nothing was sent." >&2
                    return 1
                }
                printf '%s' "$_K2_CODEX_CFG" >"$hdr_file"
                hdr_arg="$hdr_file"
                if [ -n "$_K2_CYGPATH" ]; then hdr_arg="$(cygpath -w "$hdr_file")"; fi
                set -- -K "$hdr_arg" "$@"
            fi
        fi
        _k5_headers
        for ((i = ${#_K5_HDRS[@]} - 1; i >= 0; i--)); do
            set -- -H "${_K5_HDRS[$i]}" "$@"
        done
    fi
    while :; do
        rc=0
        if [ -z "$cfg" ]; then
            curl "$@" || rc=$?
        else
            printf '%s' "$cfg" | curl -K - "$@" || rc=$?
        fi
        if [ "$rc" -ne 7 ] || [ "$once" -eq 1 ] || [ "$try" -ge "$_K2_CURL_TRIES" ]; then
            if [ -n "$hdr_file" ]; then rm -f "$hdr_file"; fi
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

# BEGIN_CLI_SOCKET_FIRST_AUTH
# A socket-first sender (cli_send, cli_send_json, _compute_send,
# _sidecar_send, _doc_xfer, _app_rt_post_file) tries the session's cell
# socket with the passport, then goes on over loopback TCP. In a session,
# when the socket failed (its file is missing, the connect failed, or it
# gave no answer), that TCP request carries the session's passport, never
# the owner token k2 read from disk ($DISK_OWNER_TOKEN). With no passport,
# nothing is sent. When the socket works, nothing changes, and a route the
# socket says it doesn't serve goes on over TCP as before.
#
# A session: a passport ($SCOPED_TOKEN) or the session marker
# ($K2_HOOK_SOCK, any of its names).
_K2_NO_OWNER_WRITE_LINE="Error: in a session, K2 doesn't act as the owner; this session has no passport, so nothing was sent."

# True when $TOKEN is the owner token from disk and this is a session.
_k2_token_is_session_owner() {
    [ -n "${DISK_OWNER_TOKEN:-}" ] && [ "${TOKEN:-}" = "$DISK_OWNER_TOKEN" ] || return 1
    [ -n "${SCOPED_TOKEN:-}" ] || [ -n "${K5_HOOK_SOCK:-${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}}" ]
}

# _k2_session_tcp_token: set _K2_TCP_TOKEN to $TOKEN, or to the session's
# passport in place of the owner token from disk. A session with no
# passport: the line above, and the command ends with 1 before anything is
# sent (_k2_end_command, so a sender inside $(...) ends the whole command).
_k2_session_tcp_token() {
    _K2_TCP_TOKEN="${TOKEN:-}"
    _k2_token_is_session_owner || return 0
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        _K2_TCP_TOKEN="$SCOPED_TOKEN"
        return 0
    fi
    echo "$_K2_NO_OWNER_WRITE_LINE" >&2
    _k2_end_command 1
}

# _k2_sock_fallback_token SOCK OUT: set _K2_TCP_TOKEN for a socket-first
# sender's TCP request. SOCK is the session's cell socket path ("" when
# this shell has none: TCP is the normal path, $TOKEN as before); OUT is
# what the socket answered, if it was tried. The socket's own "not served
# on cell socket" answer keeps $TOKEN: the socket works, and the route is
# served over TCP. A remote host (_cli_skip_uds) keeps its own token.
_k2_sock_fallback_token() {
    _K2_TCP_TOKEN="${TOKEN:-}"
    [ -n "${1:-}" ] || return 0
    _cli_skip_uds && return 0
    case "${2:-}" in *"not served on cell socket"*) return 0 ;; esac
    _k2_session_tcp_token
}
# END_CLI_SOCKET_FIRST_AUTH

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

# The processes above this call, for the shared-server check. `ps` first;
# where it can't run or gives an answer that can't be read, a walk of the
# parent chain from python3 (/proc on Linux; libproc and sysctl on macOS,
# where Codex's own sandbox refuses `ps`). Either answer is used only when
# it is complete: every process from this shell up to pid 1.
#
# _cli_ps_chain START: stdin is `ps -axo pid=,ppid=,command=`; prints the
# chain from START up to pid 1, one row each. Exit 3 when a row can't be
# read or the chain is missing a process.
_cli_ps_chain() {
    awk -v start="${1:-}" '
        !/^[ \t]*[0-9]+[ \t]+[0-9]+([ \t]|$)/ { bad = 1; exit }
        { line[$1] = $0; parent[$1] = $2 }
        END {
            if (bad || NR == 0) exit 3
            p = start; n = 0
            while ((p + 0) > 1) {
                if (!(p in line) || n >= 64) exit 3
                print line[p]; p = parent[p]; n++
            }
            exit 0
        }'
}

# The walk without ps. argv: k2-codex-ancestry START. Prints the chain as
# _cli_ps_chain does; exit 2 when the host refuses to show a process
# (EPERM, EACCES) or the platform has no walk here, 3 for anything else
# that stops a complete walk.
_K2_ANCESTRY_PY='
import ctypes, errno, struct, sys
DENIED, BROKEN = 2, 3
start = int(sys.argv[2])
if sys.platform.startswith("linux"):
    def info(pid):
        with open("/proc/%d/stat" % pid, "rb") as f:
            stat = f.read()
        ppid = int(stat[stat.rindex(b")") + 2:].split()[1])
        with open("/proc/%d/cmdline" % pid, "rb") as f:
            cmd = f.read()
        return ppid, cmd.rstrip(b"\0").replace(b"\0", b" ").decode("utf-8", "replace")
elif sys.platform == "darwin":
    lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    lib.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
    lib.sysctl.argtypes = [ctypes.POINTER(ctypes.c_int), ctypes.c_uint, ctypes.c_void_p,
                           ctypes.POINTER(ctypes.c_size_t), ctypes.c_void_p, ctypes.c_size_t]
    def fail(what):
        raise OSError(ctypes.get_errno() or errno.EIO, what)
    argmax = ctypes.c_int(0)
    size = ctypes.c_size_t(ctypes.sizeof(argmax))
    if lib.sysctl((ctypes.c_int * 2)(1, 8), 2, ctypes.byref(argmax), ctypes.byref(size), None, 0) != 0:
        fail("kern.argmax")
    def info(pid):
        # struct proc_bsdinfo (PROC_PIDTBSDINFO, 136 bytes): pbi_ppid at 16.
        bsd = ctypes.create_string_buffer(136)
        if lib.proc_pidinfo(pid, 3, 0, bsd, 136) != 136:
            fail("proc_pidinfo")
        ppid = struct.unpack_from("I", bsd.raw, 16)[0]
        comm = bsd.raw[48:64].split(b"\0")[0].decode("utf-8", "replace")
        # kern.procargs2: argc, the exec path, NUL padding, then argv.
        buf = ctypes.create_string_buffer(argmax.value)
        size = ctypes.c_size_t(argmax.value)
        if lib.sysctl((ctypes.c_int * 3)(1, 49, pid), 3, buf, ctypes.byref(size), None, 0) != 0:
            # A process of another user: its name is enough to rule it out
            # unless it could be a Codex server (codex, or a script runner).
            low = comm.lower()
            if not comm or "codex" in low or low in ("node", "bun", "deno"):
                fail("kern.procargs2")
            return ppid, comm
        data = buf.raw[:size.value]
        argc = struct.unpack_from("i", data, 0)[0]
        rest = data[4:]
        i = rest.index(b"\0")
        while i < len(rest) and rest[i:i + 1] == b"\0":
            i += 1
        args = rest[i:].split(b"\0")[:argc]
        if len(args) != argc:
            raise ValueError("short argv")
        return ppid, " ".join(a.decode("utf-8", "replace") for a in args)
else:
    # No walk is handled on this platform: counts as refused.
    sys.exit(DENIED)
rows = []
pid = start
try:
    while pid > 1:
        if len(rows) >= 64 or any(r[0] == pid for r in rows):
            sys.exit(BROKEN)
        ppid, cmd = info(pid)
        rows.append((pid, ppid, cmd.replace("\n", " ")))
        pid = ppid
except OSError as e:
    sys.exit(DENIED if e.errno in (errno.EPERM, errno.EACCES) else BROKEN)
except Exception:
    sys.exit(BROKEN)
for r in rows:
    print("%d %d %s" % r)
'

# _cli_codex_ancestry: set _K2_ANC_STATE for this shell ($$):
#   shared  a shared Codex app-server is above it;
#   clear   the chain is complete and has none;
#   blind   every way to look was refused (permission denied, the
#           command refused to run, or a platform with no walk, such as
#           Windows): the tab marker alone decides;
#   broken  an answer was missing, partial or unreadable: counts as shared.
# Called in this shell, never in $(...), so ps is this shell's child.
_K2_ANC_STATE=""
_cli_codex_ancestry() {
    local out low rc=0 r_ps r_py rows=""
    # Windows (Git Bash, MSYS, Cygwin: the prelude's _K2_WINDOWS): no walk
    # is handled there, which counts as every way refused.
    if [ -n "${_K2_WINDOWS:-}" ]; then
        _K2_ANC_STATE=blind
        return 0
    fi
    # exec: ps runs as this shell's own child on every bash (bash 3.2 forks
    # once more for a plain $(ps ...)).
    out="$(exec ps -axo pid=,ppid=,command= 2>&1)" || rc=$?
    # A refusal reads "Operation not permitted" (bash, exit 126) or
    # "operation not permitted" (zsh, exit 127), or "Permission denied".
    low="$(printf '%s' "$out" | tr '[:upper:]' '[:lower:]')"
    if [ "$rc" -eq 0 ] && rows="$(printf '%s\n' "$out" | _cli_ps_chain "$$")"; then
        r_ps=0
    elif [ "$rc" -ne 0 ] && { [ "$rc" -eq 126 ] || [[ "$low" == *"operation not permitted"* ]] || [[ "$low" == *"permission denied"* ]]; }; then
        r_ps=2
    else
        r_ps=3
    fi
    if [ "$r_ps" != 0 ]; then
        rc=0
        out="$(python3 -c "$_K2_ANCESTRY_PY" k2-codex-ancestry "$$" 2>/dev/null)" || rc=$?
        case "$rc" in
            0) rows="$out"; r_py=0 ;;
            2|126) r_py=2 ;;
            *) r_py=3 ;;
        esac
    fi
    if [ "$r_ps" = 0 ] || [ "${r_py:-3}" = 0 ]; then
        if printf '%s\n' "$rows" | _cli_ancestry_has_shared_codex_server "$$"; then
            _K2_ANC_STATE=shared
        else
            _K2_ANC_STATE=clear
        fi
    elif [ "$r_ps" = 2 ] && [ "$r_py" = 2 ]; then
        _K2_ANC_STATE=blind
    else
        _K2_ANC_STATE=broken
    fi
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
        codex_unchecked)
            echo "Error: K2 couldn't read the processes above this Codex call, so it can't rule out a" >&2
            echo "shared Codex app-server started by another session; nothing was sent." >&2
            echo "Restart Codex in this tab; K2 starts Codex without the shared server (--no-daemon)." >&2
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

# The shared-server part of the id-tool check. A Codex tool call
# (CODEX_THREAD_ID) reads the processes above it with the same walk as the
# Codex tab gate and passes only when the walk is complete and clear: a
# shared server, a walk that can't be read, or one every way refused is
# refused, so K2_CODEX_PASSPORT_BIND=0 (the gate off) never makes this
# check weaker. When the gate already confirmed the tab it read them
# (_K2_ANC_STATE set) and decided. A shell with only other CODEX_* names
# keeps the ps-only look.
_cli_id_check_codex() {
    if [ -n "${CODEX_THREAD_ID:-}" ]; then
        [ -z "$_K2_ANC_STATE" ] || return 0
        _cli_codex_ancestry
        case "$_K2_ANC_STATE" in
            clear) return 0 ;;
            shared) _cli_passport_stale_die codex_shared "$1" ;;
            *) _cli_passport_stale_die codex_unchecked "$1" ;;
        esac
    fi
    if _cli_under_shared_codex_server; then
        _cli_passport_stale_die codex_shared "$1"
    fi
}

# Before an ID/locked verb in a scoped cell: refuse a passport that is not
# this session's (shared Codex app-server) or that is dead (socket gone AND
# the daemon rejects the token). A missing socket alone is not enough — a
# failed socket bind still serves the scoped token over TCP.
# The tool is the one the whole command names (_cli_tool_id_for_args, the
# same as the credential choice), so a tool keyed on a sub-verb (`k2 agent
# team`, `k2 setup --json chief-of-staff`) gets the check too. Only ever
# stricter: a verb whose first word alone names an id tool keeps it.
_cli_check_session_passport() {
    local verb="${1:-}"
    local hook_sock="${K5_HOOK_SOCK:-${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}}"
    [ -n "$hook_sock" ] || return 0
    local tool_id mode
    tool_id="$(_cli_tool_id_for_args "$@")"
    mode="$(_cli_tool_default_mode "$tool_id")"
    if [ "$mode" != "id" ]; then
        tool_id="$(_cli_tool_id_for_verb "$verb")"
        mode="$(_cli_tool_default_mode "$tool_id")"
    fi
    [ "$mode" = "id" ] || return 0
    _cli_id_check_codex "$hook_sock"
    if [ ! -S "$hook_sock" ] && _cli_scoped_token_rejected; then
        _cli_passport_stale_die stale "$hook_sock"
    fi
    return 0
}
# END_CLI_STALE_PASSPORT

# A bare `--` ends k2's own words and starts a command's (or plain words
# that may start with a dash). Only the verbs in _k2_takes_dashdash take
# one. On any other verb's line a `--` is a usage error: exit 2, nothing
# sent, before every other gate and the router. So `k2 update -- -h` can
# never start an update, and no help scan has to guess what a `--` means.
# A new verb whose parser handles `--` goes in this list, in the same
# change, next to the file that parses it.
_k2_takes_dashdash() {
    case "${1:-}" in
        # Whole families: their shared parsers take `--` on most verbs.
        ci) return 0 ;;                       # ci.sh cmd_ci
        job|jobs) return 0 ;;                 # job.sh cmd_job
        doc|docs) return 0 ;;                 # doc.sh _doc_parse, _doc_blob_tool, _doc_cad
        wiki) return 0 ;;                     # wiki.sh _wiki_parse_args
        organization) return 0 ;;             # permission.sh cmd_organization
        send-feedback) return 0 ;;            # send-feedback.sh _sf_send
        respond) return 0 ;;                  # respond.sh cmd_respond
        done) return 0 ;;                     # session-complete.sh cmd_session_complete
        # Free text: after `--` the words are the message, as typed, so a
        # message may start with a dash or be --help (K2's ruling: message
        # text is data). Before `--`, -h/--help is still help.
        msg)                                  # msg-whoami-done.sh cmd_msg, msg-inbox-staging.sh cmd_msg_v2
            case "${2:-}" in who|verify) return 1 ;; esac   # these take --text, no free text
            return 0 ;;
        talk) return 0 ;;                     # talk-and-workspace.sh cmd_talk
        thread) return 0 ;;                   # browser.sh cmd_thread, cmd_thread_ask
        postal) return 0 ;;                   # postal.sh cmd_postal
        remind) return 0 ;;                   # remind.sh _remind_add
        chat) return 0 ;;                     # chat.sh cmd_chat
        # A group name may start with a dash: after `--` every word is a name.
        focus-group|focus-groups) return 0 ;; # focus-group.sh:177 cmd_focus_group
    esac
    case "${1:-} ${2:-}" in
        "compute run") return 0 ;;            # compute-run.sh cmd_compute_run
        "compute test") return 0 ;;           # compute-run.sh cmd_compute_test
        "compute rightsize") return 0 ;;      # compute-jobs.sh cmd_compute_rightsize
        "vault run") return 0 ;;              # migrate.sh _vault_run
        "db run") return 0 ;;                 # db-store.sh cmd_db run
        "app exec"|"app shell") return 0 ;;   # app-doctor.sh cmd_app_exec, cmd_app_shell
        "changes diff"|"changes stage"|"changes add"|"changes unstage"|"changes discard") return 0 ;;  # changes.sh
        "sim type") return 0 ;;               # sim.sh _sim_type
        "zen garden"|"zen gardens") return 0 ;;  # garden-access.sh cmd_zen_garden
        "sidecar new") return 0 ;;            # sidecar.sh cmd_sidecar_new
        "sessions archives") return 0 ;;      # awareness.sh cmd_sessions_list
        "workspace resources") return 0 ;;    # agent-context.sh cmd_workspace_resources
        "workspace api-key") return 0 ;;      # talk-and-workspace.sh cmd_workspace_set_api_key
        "heartbeat remind") return 0 ;;       # remind.sh _remind_add
        "inbox respond") return 0 ;;          # inbox.sh cmd_inbox_respond
        "tickets comment"|"feedback comment") return 0 ;;  # feedback.sh cmd_feedback_comment
        "project msg") return 0 ;;            # project.sh cmd_project_msg
        "browser note") return 0 ;;           # browser.sh cmd_browser_note
        "terminal write") return 0 ;;         # daemon-install.sh cmd_terminal_write
        # k2-approve gets these words as they are (daemon-install.sh cmd_approve_local).
        "approve verify"|"approve show"|"approve key-id") return 0 ;;
    esac
    return 1
}
if ! _k2_takes_dashdash "$@"; then
    for _a in "$@"; do
        if [ "$_a" = "--" ]; then
            if [ "${1:-}" = "--" ]; then
                echo "usage: name a command before '--' (k2 help)" >&2
            else
                echo "usage: ${1:-} takes no '--' (k2 help ${1:-})" >&2
            fi
            exit 2
        fi
    done
fi

# BEGIN_CLI_CODEX_TAB_BIND
# A Codex tool call (Codex sets CODEX_THREAD_ID on every one) keeps this
# tab's credentials only when K2 can tell it runs in a tab K2 started with
# its own Codex server. K2 sets K2_CODEX_TAB_ISOLATED to the session id of
# the tab's passport when it starts Codex with --no-daemon, or in a microVM
# cell. Any other Codex call in a K2 session may run under a Codex server
# another tab started, with that tab's credentials, so it counts as shared:
# no marker (a tab opened before this update, codex typed in a tab's shell,
# a Codex K2 could not check), a marker for another passport, or a shared
# Codex app-server above this process. Outside a K2 session nothing changes
# (_cli_codex_gate_applies). A shared call sends nothing: a command
# that would make a request ends with exit 3. Only help, schema, version
# and explain run, and `<command> -h|--help` prints `k2 help <command>`
# without running the command (some commands' own help path asks the
# daemon). The owner token from disk is never used for it.
# K2_CODEX_PASSPORT_BIND=0 turns this off, and the conversation headers
# with it (_cli_codex_bind_on and _k2_codex_headers, with the curl config
# helpers in BEGIN_CLI_CURL_CFG).

# True when this Codex call runs in a tab K2 confirmed: the marker names the
# session of the passport this call carries, and the processes above it hold
# no shared Codex app-server. Where the host lets nothing look at them
# (blind), the marker alone confirms the tab, with one note on stderr.
_cli_codex_tab_confirmed() {
    local m="${K2_CODEX_TAB_ISOLATED:-}" t="${SCOPED_TOKEN:-}"
    [ -n "$m" ] && [[ "$t" == *.* ]] && [ "${t%%.*}" = "$m" ] || return 1
    _cli_codex_ancestry
    case "$_K2_ANC_STATE" in
        clear) return 0 ;;
        blind)
            echo "Note: this host lets nothing look at the processes above this Codex call, so K2's tab marker alone confirms the tab." >&2
            return 0
            ;;
    esac
    return 1
}

_cli_codex_tab_refuse() {
    echo "Error: this Codex tab was started before this update or outside K2: reopen it from K2." >&2
    echo "K2 can't tell this call from one under a Codex server another tab started, so nothing was sent." >&2
    case "$_K2_ANC_STATE" in
        shared) echo "It runs under a shared Codex app-server; stop that server once: codex app-server daemon stop" >&2 ;;
        broken) echo "K2 couldn't read the processes above this call, so it can't rule out a shared Codex server." >&2 ;;
    esac
    echo "If a tab K2 opened still says this, set K2_CODEX_NO_DAEMON=1 for the K2 daemon, then reopen the tab." >&2
    echo "Help still works here: k2 help ${1:-}" >&2
    exit 3
}

# True when the Codex tab gate applies: this runs in a K2 session. The
# session marks are the cell socket ($K2_HOOK_SOCK, any of its names) or a
# pane id with a passport. Without them it is today's behaviour, like k2 in
# your own terminal: a terminal outside K2 (no marks at all), or a K2
# terminal on a server with scoped passports off (a pane id, but no
# passport and no socket: every tab there acts as the owner today and has
# no identity of its own to mix up). Codex typed by hand in a K2 tab has
# the socket and stays gated.
_cli_codex_gate_applies() {
    local pane
    [ -z "${K5_HOOK_SOCK:-${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}}" ] || return 0
    _envn_into pane PANE_ID
    [ -n "$pane" ] && [ -n "${SCOPED_TOKEN:-}" ]
}

# Before any command: a Codex call K2 can't confirm runs only the local
# words, never the owner token from disk.
_cli_codex_tab_gate() {
    _cli_codex_bind_on || return 0
    if ! _cli_codex_gate_applies; then
        return 0
    fi
    _cli_codex_tab_confirmed && return 0
    DISK_OWNER_TOKEN=""
    TOKEN="${SCOPED_TOKEN:-}"
    # Help and its pages ask the daemon two things (permission cards on?
    # agents may report?); here they take the answer a failed ask gives,
    # without asking.
    _K2_PERMISSION_ON=0
    _K2_SF_AGENTS_OFF=0
    local verb="${1:-help}" a
    case "$verb" in
        help|--help|-h|schema|--schema|version|--version|-v) return 0 ;;
        explain)
            # A Markdown kit id is answered by k2 doc, which asks the daemon.
            for a in "${@:2}"; do
                case "$a" in [Mm][Pp][Cc]-*|[Mm][Pp][Cc][Ee]-*) _cli_codex_tab_refuse "$verb" ;; esac
            done
            return 0
            ;;
    esac
    for a in "${@:2}"; do
        case "$a" in
            -h|--help) cmd_help "$verb"; exit 0 ;;
        esac
    done
    _cli_codex_tab_refuse "$verb"
}

# END_CLI_CODEX_TAB_BIND

# DAEMON_INSTALL_EARLY_BYPASS — `k2 daemon install` provisions the
# standalone daemon on a FRESH headless box where no daemon is running
# yet, so it cannot satisfy the PORT/TOKEN connection gate below (and
# doesn't need to: it talks only to GitHub + the filesystem). Skip the
# connection gate for it; the normal router dispatches it to
# cmd_daemon_install (which is defined before the router runs).
_skip_conn_gate=0
[ "${1:-}" = "daemon" ] && [ "${2:-}" = "install" ] && _skip_conn_gate=1
# Which `k2 update` form the words ask for (cmd_update_v2 dispatches on the
# same answer): help when -h/--help is anywhere before a bare `--`; else
# list for a --list anywhere; else the last of --app and --cli (app when
# neither). Other words are ignored, as they always were.
_k2_update_form() {
    local a form=app
    for a in "$@"; do
        [ "$a" = "--" ] && break
        case "$a" in -h|--help) echo help; return 0 ;; esac
    done
    for a in "$@"; do
        case "$a" in
            --list) echo list; return 0 ;;
            --app) form=app ;;
            --cli) form=cli ;;
        esac
    done
    echo "$form"
}
# `k2 update --help` only prints the usage, and `k2 update` / `--app` /
# `--list` talk only to GitHub (cmd_app_update refuses them under
# air-gap): no daemon, like `k2 daemon install`. `--cli` reads the local
# daemon's version, so it keeps the gate.
if [ "${1:-}" = "update" ]; then
    case "$(_k2_update_form "${@:2}")" in help|app|list) _skip_conn_gate=1 ;; esac
fi
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
    # -h or --help anywhere only prints help (the router answers it).
    for _a in "${@:2}"; do case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# Headless Connect account commands: login talks to Supabase first
# (no daemon needed for the account step); status/logout can run from
# disk alone. Daemon-touching steps (tunnel/config + start) check
# PORT/TOKEN themselves and fail loud with a start-daemon hint.
[ "${1:-}" = "connect" ] && _skip_conn_gate=1
# k2 remote talks only to the other server (k2 remote logout): no local
# daemon needed.
[ "${1:-}" = "remote" ] && _skip_conn_gate=1
# Air-gap: refuse hosted-network verbs before any curl/GitHub, even
# without a running daemon (fresh image). --help still prints. Every help
# scan below stops at the first bare `--`, as _wants_help does.
_k2_cli_wants_help=0
for _a in "$@"; do
    [ "$_a" = "--" ] && break
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
    for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 approve` with no args or --help only prints help; verify, show and
# key-id run k2-approve locally (no daemon).
if [ "${1:-}" = "approve" ]; then
    case "${2:-}" in ""|help|-h|--help|verify|show|key-id) _skip_conn_gate=1 ;; esac
    for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 send-feedback` with no args, help or --help only prints help.
if [ "${1:-}" = "send-feedback" ]; then
    case "${2:-}" in ""|help|-h|--help) _skip_conn_gate=1 ;; esac
fi
# `k2 postal`, its help and `adopt` (which only says "not available yet")
# talk to nothing (0.46.16, Postal Beta): no daemon. `send` and `route`
# once said "not available yet" too and still skip this gate, but they now
# send requests: with no port or token, the request itself fails. In a
# session every Postal request carries the passport (_postal_session_token).
if [ "${1:-}" = "postal" ]; then
    case "${2:-}" in ""|help|-h|--help|send|route|adopt) _skip_conn_gate=1 ;; esac
    for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 organization`, `k2 organization --help` and any verb's --help only
# print help (the Org Chart): no daemon.
if [ "${1:-}" = "organization" ]; then
    case "${2:-}" in ""|help|--help|-h) _skip_conn_gate=1 ;; esac
    for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 agent hire --help`, `k2 agent template` and any `k2 agent template
# … --help` only print help (agent templates T9): no daemon.
if [ "${1:-}" = "agent" ]; then
    case "${2:-}" in
        hire) for _a in "${@:3}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
        template|templates)
            case "${3:-}" in ""|help|--help|-h) _skip_conn_gate=1 ;; esac
            for _a in "${@:3}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
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
                [ "$_a" = "--" ] && break
                case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac
            done
            ;;
        *)
            for _a in "${@:2}"; do
                [ "$_a" = "--" ] && break
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
        [ "$_a" = "--" ] && break
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
                [ "$_a" = "--" ] && break
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
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 sim`, `k2 sim help` and any `k2 sim … --help` only print help.
if [ "${1:-}" = "sim" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 doc` / `k2 doc --help` / `k2 doc help` only print help.
if [ "${1:-}" = "doc" ] || [ "${1:-}" = "docs" ]; then
    case "${2:-}" in ""|help) _skip_conn_gate=1 ;; esac
    for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 changes --help` (any sub) and `k2 changes help` only print help.
if [ "${1:-}" = "changes" ]; then
    [ "${2:-}" = "help" ] && _skip_conn_gate=1
    for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 chat` / `k2 chat --help` only print help.
if [ "${1:-}" = "chat" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 voice` / `k2 voice --help` only print help.
if [ "${1:-}" = "voice" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 hostchat` / `k2 hostchat --help` only print help.
if [ "${1:-}" = "hostchat" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 app recording … --help` (0.46.11) only prints help.
if [ "${1:-}" = "app" ] && { [ "${2:-}" = "recording" ] || [ "${2:-}" = "recordings" ]; }; then
    for _a in "${@:3}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 sources` / `k2 sources --help` only print help.
if [ "${1:-}" = "sources" ] || [ "${1:-}" = "source" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 llm` / `k2 llm tokens` (alias `accounts`) / any `--help` only print help.
if [ "${1:-}" = "llm" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        tokens|token|accounts|account)
            case "${3:-}" in ""|help|--help|-h) _skip_conn_gate=1 ;; esac
            for _a in "${@:3}"; do
                [ "$_a" = "--" ] && break
                case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac
            done
            ;;
    esac
fi
# `k2 vault` / `k2 vault <verb> --help` only print help (no daemon needed).
if [ "${1:-}" = "vault" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 publish secret [<verb>] --help` (and bare `k2 publish secret`) only
# print help (no daemon needed).
if [ "${1:-}" = "publish" ] && { [ "${2:-}" = "secret" ] || [ "${2:-}" = "secrets" ]; }; then
    case "${3:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:3}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 migrate` / `k2 migrate <verb> --help` only print help (no daemon needed).
if [ "${1:-}" = "migrate" ]; then
    case "${2:-}" in
        ""|help|--help|-h) _skip_conn_gate=1 ;;
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
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
        *) for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done ;;
    esac
fi
# `k2 hooks emit` (GL45) checks its own session passport and transport
# (cell socket first), and must not need the owner token's PORT/TOKEN pair.
[ "${1:-}" = "hooks" ] && [ "${2:-}" = "emit" ] && _skip_conn_gate=1
# `k2 hooks <verb> --help` only prints help (no daemon needed).
if [ "${1:-}" = "hooks" ]; then
    for _a in "${@:2}"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _skip_conn_gate=1 ;; esac; done
fi
# `k2 checkin` is retired: its stub prints pointers and never calls the daemon.
[ "${1:-}" = "checkin" ] && _skip_conn_gate=1
# Signed approvals: verify, show and key-id run k2-approve locally (no daemon).
[ "${1:-}" = "approve" ] && case "${2:-}" in verify|show|key-id|""|-h|--help|help) _skip_conn_gate=1 ;; esac
# `k2 tickets template` prints the static brief skeleton (H42).
case "${1:-}" in
    tickets|feedback) [ "${2:-}" = "template" ] && _skip_conn_gate=1 ;;
    # W0 (0.46.0): `k2 wiki chat on` is refused locally (exit 3), never sent.
    wiki)
        [ "${2:-}" = "chat" ] && case "${3:-}" in on|enable|true|1) _skip_conn_gate=1 ;; esac
        ;;
esac

# The no-owner refusal (LD-10; also a command under k2 vault run, whose
# helper sets K2_NO_OWNER_FALLBACK=1): says why and exits 3. The guard below
# calls it, and so do the commands that read the owner token file
# themselves (heartbeat wake, the Connect account commands) when
# K2_NO_OWNER_FALLBACK is set and they have no session passport.
_k2_refuse_owner_fallback() {
    echo "Error: k2 won't act as this server's owner here: no session passport (K2_NO_OWNER_FALLBACK=1, a K2 virtual desktop or k2 vault run)." >&2
    echo "Run k2 from a K2 terminal tab or your signed-in device, not under vault run, or give the command the secret it needs." >&2
    exit 3
}

# The virtual-desktop guard (LD-10): with no passport, only the pages that
# never reach the daemon run; everything else (k2 daemon … too, which reads
# the owner token itself) is refused, saying why.
_k2_offline_verb=""
case "${1:-}" in ""|study|help|--help|-h|--schema|version|--version) _k2_offline_verb=1 ;; esac
for _a in "$@"; do [ "$_a" = "--" ] && break; case "$_a" in -h|--help) _k2_offline_verb=1 ;; esac; done
if [ -n "$_k2_no_owner_fallback" ] && [ -z "$TOKEN" ] && [ -z "$_k2_offline_verb" ]; then
    _k2_refuse_owner_fallback
fi
if [ "$_skip_conn_gate" != "1" ] && { [ -z "$PORT" ] || [ -z "$TOKEN" ]; }; then
    echo "Error: Cannot connect to K2." >&2
    echo "Either run this from a K2 terminal, or ensure K2 is running." >&2
    echo "(Connection details: ~/.k2/heartbeat.port and ~/.k2/heartbeat.token)" >&2
    exit 1
fi
unset _skip_conn_gate

_k2_daemon_base BASE_URL
# A daemon on another computer over plain http (K2_HOST is a name or an
# address that isn't loopback as written): the owner token crosses the
# network unencrypted. Allowed, with one warning line per run, printed here
# before the command's first request (the senders send curl's own stderr
# to /dev/null, so a line printed at the request itself would be lost).
# None for a literal Tailscale host or with K2_PLAIN_HTTP_OK=1. A password
# never goes there: a command line with a password flag is refused before
# any request. help, study, --schema, --version and -h/--help stay local
# and print nothing.
if [ -z "$_k2_offline_verb" ] && _k2_daemon_is_plain_remote; then
    for _a in "$@"; do
        case "$_a" in
            --password|--password-stdin|--password=*) _k2_password_transport_ok ;;
        esac
    done
    unset _a
    _envn_into _k2_plain_ok PLAIN_HTTP_OK
    if [ "$_k2_plain_ok" != 1 ] && ! _k2_host_is_tailnet_literal "$HOST"; then
        # The command's own name (white-label), not a typed one.
        echo "warning: ${0##*/} is sending this server's owner token to ${HOST} over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https." >&2
        # Said once: a command's own copy of this warning (clone's) skips it.
        _K2_PLAIN_HTTP_WARNED=1
    fi
    unset _k2_plain_ok
fi

# HTTP helper
# 75 = local_transport_denied: curl 7, a connect the OS refused locally
# (EPERM, EACCES, no IPv6 loopback). Nothing was sent.
# 76 = tmpdir_unwritable: curl failed while TMPDIR can't be used. The
# request may have gone out (cli_send never sends such a POST again).
# Neither is a curl 8.x code; both survive $(cli_request) subshells.
# A plain refused connect stays curl 7 (daemon_unreachable).
_CLI_LOCAL_TRANSPORT_DENIED=75
_CLI_TMPDIR_UNWRITABLE=76

# _cli_transport_failure RC ERR: 75 or 76 on stdout when the failure is a
# local restriction (see above), nothing otherwise.
_cli_transport_failure() {
    local rc="$1" err="$2"
    [ "$rc" -ne 0 ] || return 0
    if [[ "$err" == *[Cc]onnection\ refused* ]]; then
        return 0
    fi
    if [ "$rc" -eq 7 ]; then
        if [[ "$err" == *[Oo]peration\ not\ permitted* || "$err" == *EPERM* \
           || "$err" == *[Pp]ermission\ denied* || "$err" == *EACCES* ]]; then
            echo "$_CLI_LOCAL_TRANSPORT_DENIED"; return 0
        fi
        if [[ "$err" == *Address\ family\ not\ supported* \
           || "$err" == *Protocol\ family\ not\ supported* ]]; then
            echo "$_CLI_LOCAL_TRANSPORT_DENIED"; return 0
        fi
        if [[ "$err" == *"::1"* && ( "$err" == *Network\ is\ unreachable* \
           || "$err" == *No\ route\ to\ host* ) ]]; then
            echo "$_CLI_LOCAL_TRANSPORT_DENIED"; return 0
        fi
    fi
    if [[ "$err" == *temporary\ file* || "$err" == *temp\ file* || "$err" == *TMPDIR* ]]; then
        echo "$_CLI_TMPDIR_UNWRITABLE"; return 0
    fi
    local td="${TMPDIR:-/tmp}"
    if [ ! -d "$td" ] || [ ! -w "$td" ]; then
        echo "$_CLI_TMPDIR_UNWRITABLE"
    fi
    return 0
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
        case "$a" in @-|-T) once=1 ;; esac
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
    local local_rc
    local_rc="$(_cli_transport_failure "$rc" "$err")"
    return "${local_rc:-$rc}"
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

# BEGIN_CLI_NO_ANSWER
# A request the daemon never answered ends the command with one plain
# line that names the route and how long it waited, never a JSON parse
# error from the reader downstream (`… | python3 -c 'json.loads…'` used to
# print "Expecting value: line 1 column 1 (char 0)" after a stall).
# "Never answered": curl timed out (28), the connection closed with no
# reply (52), or a 5xx came back with an empty body.
#
# The request helpers run inside $(…) or on the left of a pipe, where
# `exit` ends only that subshell and the reader still runs.
# _k2_end_command ends the whole command from there:
#   - in a $(…) with no reader beside it: the main shell's USR1 trap
#     (exit 1 once the substitution returns; EXIT traps run);
#   - with a reader beside it (`… | python3 -c 'json.loads…'`): the shells
#     between here and the main one get SIGPIPE, then the reader SIGTERM,
#     so nobody is left to print the reader's error or bash's "Terminated"
#     job report. The exit status is then 141 (SIGPIPE), still non-zero.
# Any other process of this command (a spinner) ends with the reader.
# No temp files (here-strings, mktemp): the disk may be full.
trap 'exit 1' USR1
_k2_end_command() {
    local st="${1:-1}" probe="" table="" plan="" anc="" kids=""
    # The main shell itself: a plain exit.
    [ -n "${BASHPID:-}" ] && [ "$BASHPID" = "$$" ] && exit "$st"
    kill -USR1 "$$" 2>/dev/null || true
    # Which pid is this shell? bash 3.2 has no BASHPID, so a probe child
    # names it: the probe's parent in the table is this shell.
    sleep 30 &
    probe=$!
    table="$(ps -A -o pid= -o ppid= 2>/dev/null)" || table=""
    kill "$probe" 2>/dev/null || true
    if [ -n "$table" ]; then
        # Line 1: the shells between here and the main one (main included).
        # Line 2: the command's other processes (not mine, not below me).
        # Nothing when there are none of the second kind.
        plan="$(printf '%s\n' "$table" | awk -v main="$$" -v probe="$probe" '
            { par[$1] = $2 }
            END {
                if (!(probe in par)) exit
                me = par[probe]
                if (me == main || !(me in par)) exit
                p = par[me]
                while (p != "" && p != main && (p in par)) { anc[p] = 1; p = par[p] }
                if (p != main) exit
                anc[main] = 1
                live[main] = 1; grew = 1
                while (grew) {
                    grew = 0
                    for (c in par) if (!(c in live) && (par[c] in live)) { live[c] = 1; grew = 1 }
                }
                mine[me] = 1; grew = 1
                while (grew) {
                    grew = 0
                    for (c in par) if (!(c in mine) && (par[c] in mine)) { mine[c] = 1; grew = 1 }
                }
                k = ""
                for (c in live) if (!(c in anc) && !(c in mine)) k = k " " c
                if (k == "") exit
                a = ""
                for (c in anc) a = a " " c
                print a
                print k
            }')" || plan=""
    fi
    if [ -n "$plan" ]; then
        anc="${plan%%$'\n'*}"
        kids="${plan#*$'\n'}"
        # shellcheck disable=SC2086
        kill -PIPE $anc 2>/dev/null || true
        # shellcheck disable=SC2086
        kill -TERM $kids 2>/dev/null || true
    fi
    exit "$st"
}

# _cli_no_answer RC OUT ENDPOINT T0 — returns 1 (carry on) unless the
# request went unanswered; then prints the line and ends the command.
# Quiet inside cli_send's cell-socket try (_K2_NOANSWER_SOFT=1): the TCP
# try that follows reports. T0 is $SECONDS when the request began.
_cli_no_answer() {
    local rc="$1" out="$2" endpoint="$3" t0="${4:-$SECONDS}"
    case "$rc" in
        28|52) ;;
        0)
            if [ -n "$out" ] || [[ "${_K2_HTTP:-}" != 5* ]]; then return 1; fi
            ;;
        *) return 1 ;;
    esac
    [ "${_K2_NOANSWER_SOFT:-0}" = 1 ] && return 1
    local waited=$((SECONDS - t0))
    if [ "$rc" = 28 ]; then
        echo "Error: K2's server didn't answer ${endpoint} within ${waited} s; run k2 daemon status" >&2
    else
        echo "Error: K2's server gave an empty answer to ${endpoint} after ${waited} s; run k2 daemon status" >&2
    fi
    _k2_end_command 1
}
# END_CLI_NO_ANSWER

cli_request() {
    local endpoint="$1"
    shift
    local params="project=$(urlencode "$PROJECT")"
    for param in "$@"; do
        params="${params}&${param}"
    done
    local _K2_CFG="" out rc=0 try=1 t0="${_K2_REQ_T0:-$SECONDS}"
    _k2_cfg_auth "$TOKEN"
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
    _cli_no_answer "$rc" "$out" "$endpoint" "$t0" || true
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
    local params="project=$(urlencode "$PROJECT")"
    for param in "$@"; do
        params="${params}&${param}"
    done
    local out rc=0 _K2_CFG="" try=1 t0="${_K2_REQ_T0:-$SECONDS}" sep='?'
    # An endpoint that has a query already gets ours after `&`.
    [[ "$endpoint" == *'?'* ]] && sep='&'
    _k2_cfg_add url "${BASE_URL}${endpoint}${sep}${params}"
    _k2_cfg_auth "$TOKEN"
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
    _cli_no_answer "$rc" "$out" "$endpoint" "$t0" || true
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
    local _K2_CFG="" out rc=0 try=1 t0="${_K2_REQ_T0:-$SECONDS}" sep='?'
    [[ "$endpoint" == *'?'* ]] && sep='&'
    # 0.46.17: no passport, no Authorization line (never an empty Bearer).
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    fi
    while :; do
        rc=0
        out="$(r=0; _cli_curl --k2-cfg "$_K2_CFG" -sS -v -X POST -w "$_K2_WSTATUS" --unix-socket "$(_hook_sock)" "http://localhost${endpoint}${params:+${sep}${params}}" \
            --connect-timeout 5 --max-time 30 \
            --data-raw "" || r=$?; printf x; exit "$r")" || rc=$?
        out="${out%x}"
        _cli_split_status
        if [ "$rc" -eq 0 ] && _cli_migrating "$out" && [ "$try" -lt "$_K2_CURL_TRIES" ]; then
            try=$((try + 1)); sleep 1; continue
        fi
        break
    done
    _cli_no_answer "$rc" "$out" "$endpoint" "$t0" || true
    if [ "$rc" -eq 0 ] && _cli_retry_with_healed_passport "$out"; then
        cli_request_post_uds "$endpoint" "$@"
        return $?
    fi
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

# POST a JSON body. Used by /cli/awareness/publish (Phase 3).
# The token rides an Authorization header (_k2_cfg_auth). Caller supplies
# the full endpoint
# and the JSON body on stdin via a pipe, or as the second arg.
cli_post_json() {
    local endpoint="$1"
    local body="${2:-}"
    if [ -z "$body" ]; then
        body="$(cat)"
    fi
    # Bodies carry passwords (k2 users add, k2 app user password): config too.
    local _K2_CFG="" out rc=0 t0="${_K2_REQ_T0:-$SECONDS}"
    _k2_cfg_add url "${BASE_URL}${endpoint}"
    _k2_cfg_auth "$TOKEN"
    _k2_cfg_add data-raw "$body"
    # 0.46.6 (client papercut 12): note the refusal's (or hold's) `permission`
    # object, so a held `k2 publish run` ends with its Ask-with line.
    out="$(r=0; _k2_curl "$_K2_CFG" -s -X POST \
        --connect-timeout 5 --max-time "${_K2_POST_MAX_TIME:-30}" \
        -H "Content-Type: application/json" 2>/dev/null || r=$?; printf x; exit "$r")" || rc=$?
    out="${out%x}"
    # No status is read here: a 5xx from an earlier request must not count.
    _K2_HTTP=""
    _cli_no_answer "$rc" "$out" "$endpoint" "$t0" || true
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
# (argv is readable by every OS user via `ps`). The URL and the token's
# header reach curl as config lines on fd 3, so they are off argv too.
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
    local _K2_CFG=""
    _k2_cfg_add url "${BASE_URL}${endpoint}"
    _k2_cfg_auth "$TOKEN"
    # _k2_curl with no config: its stdin is the body, so the URL and the
    # header stay on fd 3. A body read from stdin is never retried.
    local out rc=0
    out="$(r=0; _k2_curl "" -s -K /dev/fd/3 -X POST \
        --connect-timeout 5 --max-time 30 \
        -H "Content-Type: application/json" \
        --data-binary @- 2>/dev/null 3<<<"$_K2_CFG" || r=$?; printf x; exit "$r")" || rc=$?
    out="${out%x}"
    # 0.46.16: the refusal's `permission` object, as cli_post_json.
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

# Read one secret from stdin into _K2_SECRET: a hidden prompt when stdin
# is a terminal (`read -rs`), else one line from the pipe. Never echoed.
# _cli_read_secret_stdin [PROMPT] [password]
_cli_read_secret_stdin() {
    local prompt="${1:-Paste the key: }"
    _K2_SECRET=""
    # A second argument "password": the value is a password or API key for
    # the daemon, refused over plain http to another computer, typed or
    # piped, before anything is read (_k2_password_transport_ok).
    [ "${2:-}" != password ] || _k2_password_transport_ok
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

# Percent-encode one value for a query string: every byte except ASCII
# letters, digits and `_.-~` becomes %XX. Byte-safe: a value that is not
# valid UTF-8 is encoded byte by byte (os.fsencode gives back the bytes the
# shell passed). Never falls back to the raw value: a raw `&` or `=` would
# start a new query parameter. When it cannot encode (no python3), it says
# so and ends the whole command.
urlencode() {
    local out
    if out="$(python3 -c "import os, sys, urllib.parse; print(urllib.parse.quote_from_bytes(os.fsencode(sys.argv[1]), safe=''))" "$1")"; then
        printf '%s\n' "$out"
        return 0
    fi
    echo "Error: k2 could not encode a value for its request (python3 failed or is missing); nothing was sent." >&2
    _k2_end_command 1
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
# The scan stops at the first bare `--`: the words after it belong to the
# command being run (`k2 compute run -- ls -h`), never to k2.
_wants_help() {
    local a
    for a in "$@"; do
        [ "$a" = "--" ] && break
        case "$a" in
            --help|-h) return 0 ;;
        esac
    done
    return 1
}

# POST a form-encoded body (0.39.45, GH #35/#37/#29). Project rides the
# query string and the token an Authorization header (_k2_cfg_auth);
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
    local _K2_CFG="" out rc=0 t0="${_K2_REQ_T0:-$SECONDS}"
    _k2_cfg_add url "${BASE_URL}${endpoint}?project=$(urlencode "$PROJECT")"
    _k2_cfg_auth "$TOKEN"
    _k2_cfg_add data-raw "$body"
    out="$(r=0; _k2_curl "$_K2_CFG" -s -X POST \
        --connect-timeout 5 --max-time "${_K2_POST_MAX_TIME:-30}" \
        -H "Content-Type: application/x-www-form-urlencoded" 2>/dev/null || r=$?; printf x; exit "$r")" || rc=$?
    out="${out%x}"
    # No status is read here: a 5xx from an earlier request must not count.
    _K2_HTTP=""
    _cli_no_answer "$rc" "$out" "$endpoint" "$t0" || true
    if [ "$rc" -eq 0 ] && _cli_retry_with_healed_passport "$out"; then
        cli_post_form "$endpoint" "$@"
        return $?
    fi
    # 0.46.16: note the refusal's `permission` object, as cli_post_json
    # (`k2 msg`, `k2 inbox`: the form sends never noted it, so a refused
    # send ended with another request's line, or none).
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

