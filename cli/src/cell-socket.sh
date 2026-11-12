# ── #58 Phase-1 close: scoped per-cell UDS arms ──────────────────────────
# These mirror cli_request / cli_post_form / cli_post_json but talk over the
# per-cell Unix socket ($K2_HOOK_SOCK) with the SCOPED token as a Bearer
# header. They deliberately OMIT `token=` (Bearer instead) and `project=` /
# `from=` (the daemon stamps the sender identity + the agent's own-workspace
# path SERVER-SIDE from the socket-bound principal — the body is never
# trusted for identity). Recipient/routing args (`workspace=`, inbox `id=`,
# etc.) are passed through by the caller exactly as for the TCP arms.
_hook_sock() { echo "${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}"; }

cli_request_uds() {
    local endpoint="$1"; shift
    local params=""
    for param in "$@"; do params="${params}${params:+&}${param}"; done
    local _K2_CFG="" out rc=0 try=1
    _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    if [ -n "$params" ]; then
        _k2_cfg_add data "$params"
    fi
    while :; do
        rc=0
        out="$(r=0; _cli_curl --k2-cfg "$_K2_CFG" -sS -v -G -w "$_K2_WSTATUS" --unix-socket "$(_hook_sock)" "http://localhost${endpoint}" \
            --connect-timeout 5 --max-time 30 || r=$?; printf x; exit "$r")" || rc=$?
        out="${out%x}"
        _cli_split_status
        if [ "$rc" -eq 0 ] && _cli_migrating "$out" && [ "$try" -lt "$_K2_CURL_TRIES" ]; then
            try=$((try + 1)); sleep 1; continue
        fi
        break
    done
    if [ "$rc" -eq 0 ] && _cli_retry_with_healed_passport "$out"; then
        cli_request_uds "$endpoint" "$@"
        return $?
    fi
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

cli_post_form_uds() {
    local endpoint="$1"; shift
    local body=""
    for param in "$@"; do body="${body}${body:+&}${param}"; done
    local _K2_CFG=""
    _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    _k2_cfg_add data-raw "$body"
    _k2_curl "$_K2_CFG" -s -X POST --unix-socket "$(_hook_sock)" "http://localhost${endpoint}" \
        --connect-timeout 5 --max-time 30 \
        -H "Content-Type: application/x-www-form-urlencoded" 2>/dev/null
}

cli_post_json_uds() {
    local endpoint="$1"
    local body="${2:-}"
    if [ -z "$body" ]; then body="$(cat)"; fi
    local _K2_CFG="" out rc=0
    _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    _k2_cfg_add data-raw "$body"
    # 0.46.6: the permission note, as cli_post_json.
    out="$(r=0; _k2_curl "$_K2_CFG" -s -X POST --unix-socket "$(_hook_sock)" "http://localhost${endpoint}" \
        --connect-timeout 5 --max-time 30 \
        -H "Content-Type: application/json" 2>/dev/null || r=$?; printf x; exit "$r")" || rc=$?
    out="${out%x}"
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

# Verbs the scoped per-cell socket actually serves (the daemon allowlist).
# Anything else stays on TCP+owner.
_uds_eligible() {
    case "$1" in
        /cli/workspace/msg|/cli/awareness/publish) return 0 ;;
        # Folder trust (SS10): agents read status and ask (the card).
        /cli/folder-trust/status|/cli/folder-trust/request) return 0 ;;
        # C2: peer-gated live terminal peek (k2 read <ws>).
        /cli/terminal/read) return 0 ;;
        /cli/inbox/*|/cli/review-checklist/*) return 0 ;;
        # DNS K1 — cell_server serves /cli/dns/* POSTs + is_agent_verb GETs.
        /cli/dns/*) return 0 ;;
        # Custom domains: GET list + hostname add/remove + certs are
        # agent verbs (dns_manage). Apex attach/remove stay TCP owner.
        /cli/domains|/cli/domains/names|/cli/domains/names/remove|/cli/certs|/cli/certs/*) return 0 ;;
        # Publish hosted services — /cli/publish/* (agent verb, not /cli/tunnel/).
        /cli/publish/*) return 0 ;;
        # C1 — connections list/add/remove (mutate toggle-gated server-side).
        /cli/connections) return 0 ;;
        # PR1 federation dual-auth — peers/peer-roster/send under passport.
        # pair/confirm stay TCP owner-or-admin only (not here).
        /cli/federation/peers|/cli/federation/peer-roster|/cli/federation/peer-rosters|/cli/federation/send) return 0 ;;
        # Named heartbeat schedule CRUD + fire/status (agent-owned workspace
        # schedules). OS tick install + fleet-wide list stay TCP owner-only.
        /cli/heartbeat/install-launchd|/cli/heartbeat/uninstall-launchd|/cli/heartbeat/apply-wake-scheduler) return 1 ;;
        /cli/heartbeat/list-all|/cli/heartbeat/fires-list-all|/cli/heartbeat/set-show-sessions) return 1 ;;
        /cli/heartbeat/*|/cli/heartbeat-log) return 0 ;;
        /cli/thread|/cli/thread/*) return 0 ;;
        # Design mode notes (agent passport, own workspace only).
        /cli/browser/notes|/cli/browser/notes/redeliver) return 0 ;;
        *) return 1 ;;
    esac
}

# cli_send <tcp_fn> <endpoint> [args...] — channel selector.
# When a per-cell scoped socket is present AND the endpoint is UDS-eligible,
# try the matching `<tcp_fn>_uds` arm; on success (rc==0, non-empty, and NOT
# the daemon's "route not served on cell socket" 404 sentinel) print + return.
# Otherwise fall through to the TCP arm `<tcp_fn>`.
#   UDS arm  → $SCOPED_TOKEN (Bearer)
#   TCP arm  → $TOKEN, which _cli_apply_auth_for_verb already chose:
#              Open tools may be disk-owner (COMPAT-58); ID/locked tools are
#              SCOPED_TOKEN only (never the owner inversion).
cli_send() {
    local fn="$1" endpoint="$2"; shift 2
    local sock; sock="$(_hook_sock)"
    # After _apply_remote_host, every hop must use BASE_URL+Connect token.
    # UDS would hit THIS box's cell socket (Bug A: foreign staged path).
    if _cli_skip_uds; then
        "$fn" "$endpoint" "$@"
        return
    fi
    if [ -n "$sock" ] && [ -S "$sock" ] && _uds_eligible "$endpoint"; then
        # `|| rc=$?`, never `; rc=$?`: under `set -e` a cell socket that
        # refused or timed out ended the command right here, before the TCP
        # fallback, and the caller printed an empty answer (0.46.12:
        # `k2 inbox read` → "no usable answer: ''" from a session whose
        # socket nobody served).
        local out rc=0
        out="$("${fn}_uds" "$endpoint" "$@")" || rc=$?
        if [ "$rc" -eq 0 ] && [ -n "$out" ] && [[ "$out" != *"not served on cell socket"* ]]; then
            printf '%s' "$out"
            return 0
        fi
    fi
    "$fn" "$endpoint" "$@"
}

# BEGIN_CLI_SEND_TCP_LOCK
# Set when a command has repointed BASE_URL at a remote Connect host so
# inbox/fs/resolve/deliver cannot UDS-shortcut to the local cell socket.
_K2_FORCE_TCP=0
_cli_skip_uds() {
    [ "${_K2_FORCE_TCP:-0}" = "1" ] || [ "${_K2_TRAY_REMOTE_APPLIED:-0}" = "1" ]
}
# END_CLI_SEND_TCP_LOCK

# BEGIN_CLI_SEND_JSON
# cli_send_json METHOD PATH [payload] → "<body>\n<status>" on stdout.
# GET payload = a pre-urlencoded query; POST payload = a JSON body.
# The JSON twin of cli_send that keeps the HTTP status (so callers can
# print the daemon's {"error","code"} and exit 3) and, unlike
# cli_request_post_uds, sends a real JSON body over the cell socket.
#   In a K2 terminal (passport + $K2_HOOK_SOCK): the cell socket with the
#   passport as Bearer, no `token=`. The daemon scopes the call to the
#   passport's own workspace.
#   Otherwise, or when the cell socket doesn't serve the route (an older
#   daemon answers "not served on cell socket"), loopback TCP with
#   `?token=$TOKEN` (owner outside K2; the passport inside, which the
#   owner-only routes refuse with 403 rather than elevate).
# Credentials and bodies ride curl config on stdin (_k2_curl), never argv.
# Status 000 = the daemon did not answer.
# Each request may take up to 60 s; a caller whose route blocks longer
# (k2 sim boot/install/wait) sets _K2_SEND_MAX_TIME for that one call.
cli_send_json() {
    local method="$1" path="$2" payload="${3:-}"
    local mt="${_K2_SEND_MAX_TIME:-60}"
    local sock out="" _K2_CFG=""
    sock="$(_hook_sock)"
    if ! _cli_skip_uds && [ -n "$sock" ] && [ -S "$sock" ] && [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        if [ "$method" = "GET" ]; then
            [ -n "$payload" ] && _k2_cfg_add data "$payload"
            out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time "$mt" -w $'\n%{http_code}' 2>/dev/null)" || out=""
        else
            _k2_cfg_add data-raw "$payload"
            out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time "$mt" -H "Content-Type: application/json" \
                -w $'\n%{http_code}' 2>/dev/null)" || out=""
        fi
        if [ -n "$out" ] && [[ "$out" != *"not served on cell socket"* ]] && [[ "$out" != *$'\n000' ]]; then
            if _cli_retry_with_healed_passport "$out"; then
                cli_send_json "$method" "$path" "$payload"
                return $?
            fi
            _permission_note "$out"
            printf '%s' "$out"
            return 0
        fi
    fi
    _K2_CFG=""
    if [ "$method" = "GET" ]; then
        _k2_cfg_add data-urlencode "token=${TOKEN}"
        [ -n "$payload" ] && _k2_cfg_add data "$payload"
        out="$(_k2_curl "$_K2_CFG" -s -G "${BASE_URL}${path}" \
            --connect-timeout 5 --max-time "$mt" -w $'\n%{http_code}' 2>/dev/null || printf '\n000'; printf x)"
    else
        _k2_cfg_add url "${BASE_URL}${path}?token=$(urlencode "$TOKEN")"
        _k2_cfg_add data-raw "$payload"
        out="$(_k2_curl "$_K2_CFG" -s -X POST --connect-timeout 5 --max-time "$mt" \
            -H "Content-Type: application/json" -w $'\n%{http_code}' 2>/dev/null || printf '\n000'; printf x)"
    fi
    out="${out%x}"
    if _cli_retry_with_healed_passport "$out"; then
        cli_send_json "$method" "$path" "$payload"
        return $?
    fi
    _permission_note "$out"
    printf '%s' "$out"
}
# END_CLI_SEND_JSON

# BEGIN_CLI_PERMISSION_NOTE
# Permission cards (Beta; PRD rev 3.1 §3.1, §11.3, vs-live C4). While both
# switches are on for the calling agent, the daemon adds a top-level
# `permission` object to every refusal it sends that agent. The shared
# transports above hand every body to `_permission_note`, which keeps the
# one closing line — "Ask with: k2 permission request …" or (after a
# machine-only hint) "It can't be requested." — and `_permission_note_flush`
# prints it once, on stderr, when the command exits: after the family's own
# "Error […]" line, so a refusal ends with it. Bodies without a
# `permission` object (every body while the switches are off) cost one
# string match and print nothing; exit codes are untouched (except
# `cli_print_or_fail`, below).
#
# The transports run inside $(…) subshells, so the note is a small file
# keyed by the CLI's own pid ($$ is the parent's pid in a subshell).
_K2_PERMISSION_NOTE="${TMPDIR:-/tmp}/k2-permission-note.$$"

# The closing line(s) for a body (cli_send_json's "\n<status>" tail is
# ignored), or nothing.
_permission_closing_lines() {
    printf '%s' "$1" | python3 -c '
import json, re, sys
raw = sys.stdin.read()
m = re.match(r"(?s)(.*)\n[0-9]{3}$", raw)
if m:
    raw = m.group(1)
try:
    v = json.loads(raw)
except Exception:
    sys.exit(0)
p = v.get("permission") if isinstance(v, dict) else None
if not isinstance(p, dict):
    sys.exit(0)
cmd = p.get("command")
if isinstance(cmd, str) and cmd:
    print("Ask with: " + cmd)
elif p.get("requestable") is False:
    hint = p.get("hint")
    if isinstance(hint, str) and hint:
        print(hint)
    print("It can'"'"'t be requested.")
' 2>/dev/null
}

_permission_note() {
    # The Org Chart's feed (prd-org-chart-v1 §6.3): the daemon adds
    # `"bulletin":{"day":…,"unread":true,"key":…}` to an agent's answers
    # while today's bulletin is unread. Note the day and key; the flush
    # prints one line on success, at most every 30 minutes per agent.
    case "${1:-}" in
        *'"bulletin":{"day":"'*)
            if [[ "${1:-}" =~ \"bulletin\":\{\"day\":\"([0-9-]+)\",\"unread\":true,\"key\":\"([0-9a-f]+)\" ]]; then
                printf '%s %s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" >"${_K2_PERMISSION_NOTE}.bulletin" 2>/dev/null || true
            fi
            ;;
    esac
    # A passport refused: the flush checks whether it is really dead.
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        case "${1:-}" in *"Invalid or missing auth token"*) : >"${_K2_PERMISSION_NOTE}.passport" 2>/dev/null ;; esac
    fi
    case "${1:-}" in *'"permission"'*) ;; *) return 0 ;; esac
    local lines
    lines="$(_permission_closing_lines "$1")" || return 0
    [ -n "$lines" ] || return 0
    if ! printf '%s\n' "$lines" > "$_K2_PERMISSION_NOTE" 2>/dev/null; then
        printf '%s\n' "$lines" >&2
    fi
    return 0
}

# True when a body carries the refusal's `permission` object.
_permission_in_body() {
    case "${1:-}" in *'"permission"'*) ;; *) return 1 ;; esac
    [ -n "$(_permission_closing_lines "$1")" ]
}

# The EXIT trap (installed by the router): print the noted line, once,
# and ONLY when the command itself failed: refused (exit 3), held (exit 2),
# or a family that still maps a refusal to exit 1. Never on success: a
# command that succeeded may still have met a refusal on the way (the cell
# socket, then TCP; a probe), and its "Ask with …" / "It can't be
# requested." line read like a failure under a success (0.46.4 field
# report: `k2 thread`). `$1` is the exit status when the trap ran
# something first. Never fails, so the command's own exit code stands.
_permission_note_flush() {
    local rc="${1:-$?}" f
    f="$_K2_PERMISSION_NOTE"
    # That text also answers a good passport on a route agents can't use, so
    # ask the daemon about the passport itself (whoami is an agent verb).
    if [ -f "${f}.passport" ]; then
        rm -f "${f}.passport" 2>/dev/null || true
        _cli_use_healed_passport || true
        if ! _cli_skip_uds && _cli_scoped_token_rejected; then
            echo "This session's passport isn't valid on this daemon. Restart this tab to get a new one." >&2
        fi
    fi
    if [ -f "$f" ]; then
        if [ "$rc" != 0 ]; then
            cat "$f" >&2 2>/dev/null || true
        fi
        rm -f "$f" 2>/dev/null || true
    fi
    _org_bulletin_flush "$rc"
    return 0
}

# The Org Chart's feed line: once a command succeeded, while today's
# bulletin is unread for this agent, at most every 30 minutes (the stamp
# file is per agent: the daemon's key). A missing stamp shows it again.
_org_bulletin_flush() {
    local rc="${1:-0}" f="${_K2_PERMISSION_NOTE}.bulletin" day key stamp last now
    [ -f "$f" ] || return 0
    read -r day key <"$f" 2>/dev/null || true
    rm -f "$f" 2>/dev/null || true
    [ "$rc" = 0 ] && [ -n "${day:-}" ] && [ -n "${key:-}" ] || return 0
    stamp="${TMPDIR:-/tmp}/k2-bulletin.${key}"
    now="$(date +%s)"
    last=""
    [ -f "$stamp" ] && read -r last <"$stamp" 2>/dev/null
    case "$last" in ''|*[!0-9]*) last=0 ;; esac
    if [ $((now - last)) -ge 1800 ]; then
        echo "K2 bulletin for ${day} is unread: k2 organization bulletin" >&2
        printf '%s\n' "$now" >"$stamp" 2>/dev/null || true
    fi
    return 0
}
# END_CLI_PERMISSION_NOTE

# BEGIN_CLI_PASSPORT_PROJECT
# 0.46.3 (agent CLI audit A2): `k2 tickets list`, `k2 feedback list` and
# `k2 activity` name a workspace with `project=`, which comes from the
# folder the caller stands in ($PWD walk). An agent that ran them outside
# its folder got "workspace not registered". In a K2 terminal (session
# passport), ask the daemon which workspace the passport belongs to
# (`/cli/whoami` → `path`) and use that. An explicit K2_PROJECT_PATH wins;
# people (no passport), or a daemon that answers no path, keep the $PWD
# result.
_cli_use_passport_project() {
    [ -z "${K2_PROJECT_PATH:-${K2SO_PROJECT_PATH:-}}" ] || return 0
    [ -n "${SCOPED_TOKEN:-}" ] || return 0
    _cli_skip_uds && return 0
    local sock out="" path="" _K2_CFG=""
    sock="$(_hook_sock)"
    if [ -n "$sock" ] && [ -S "$sock" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost/cli/whoami" \
            --connect-timeout 5 --max-time 15 2>/dev/null)" || out=""
    fi
    if [ -z "$out" ] || [[ "$out" == *"not served on cell socket"* ]]; then
        _K2_CFG=""
        _k2_cfg_add data-urlencode "token=${SCOPED_TOKEN}"
        out="$(_k2_curl "$_K2_CFG" -s -G "${BASE_URL}/cli/whoami" \
            --connect-timeout 5 --max-time 15 2>/dev/null)" || out=""
    fi
    [ -n "$out" ] || return 0
    path="$(printf '%s' "$out" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception:
    sys.exit(0)
p = d.get("path") if isinstance(d, dict) else None
if isinstance(p, str) and p.strip():
    print(p.strip())
' 2>/dev/null)" || path=""
    if [ -n "$path" ]; then
        PROJECT="$path"
    fi
    return 0
}
# END_CLI_PASSPORT_PROJECT

