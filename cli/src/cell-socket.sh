# ── #58 Phase-1 close: scoped per-cell UDS arms ──────────────────────────
# These mirror cli_request / cli_post_form / cli_post_json but talk over the
# per-cell Unix socket ($K2_HOOK_SOCK) with the SCOPED token as a Bearer
# header. They deliberately OMIT `token=` (Bearer instead) and `project=` /
# `from=` (the daemon stamps the sender identity + the agent's own-workspace
# path SERVER-SIDE from the socket-bound principal — the body is never
# trusted for identity). Recipient/routing args (`workspace=`, inbox `id=`,
# etc.) are passed through by the caller exactly as for the TCP arms.
_hook_sock() { local s; _envn_into s HOOK_SOCK; echo "$s"; }

cli_request_uds() {
    local endpoint="$1"; shift
    local params=""
    for param in "$@"; do params="${params}${params:+&}${param}"; done
    local _K2_CFG="" out rc=0 try=1 t0="${_K2_REQ_T0:-$SECONDS}"
    # 0.46.17: no passport, no Authorization line (never an empty Bearer).
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    fi
    if [ -n "$params" ]; then
        _k2_cfg_add data-raw "$params"
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
    _cli_no_answer "$rc" "$out" "$endpoint" "$t0" || true
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
    local _K2_CFG="" out rc=0 t0="${_K2_REQ_T0:-$SECONDS}"
    # 0.46.17: no passport, no Authorization line (never an empty Bearer).
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    fi
    _k2_cfg_add data-raw "$body"
    out="$(r=0; _k2_curl "$_K2_CFG" -s -X POST --unix-socket "$(_hook_sock)" "http://localhost${endpoint}" \
        --connect-timeout 5 --max-time "${_K2_POST_MAX_TIME:-30}" \
        -H "Content-Type: application/x-www-form-urlencoded" 2>/dev/null || r=$?; printf x; exit "$r")" || rc=$?
    out="${out%x}"
    # No status is read here: a 5xx from an earlier request must not count.
    _K2_HTTP=""
    _cli_no_answer "$rc" "$out" "$endpoint" "$t0" || true
    if [ "$rc" -eq 0 ] && _cli_retry_with_healed_passport "$out"; then
        cli_post_form_uds "$endpoint" "$@"
        return $?
    fi
    # 0.46.16: the send's own refusal closes the command (`k2 msg` to an
    # agent that isn't a connection: Ask with: … connections.add).
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

cli_post_json_uds() {
    local endpoint="$1"
    local body="${2:-}"
    if [ -z "$body" ]; then body="$(cat)"; fi
    local _K2_CFG="" out rc=0 t0="${_K2_REQ_T0:-$SECONDS}"
    # 0.46.17: no passport, no Authorization line (never an empty Bearer).
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    fi
    _k2_cfg_add data-raw "$body"
    # 0.46.6: the permission note, and the healed passport, as cli_post_json.
    out="$(r=0; _k2_curl "$_K2_CFG" -s -X POST --unix-socket "$(_hook_sock)" "http://localhost${endpoint}" \
        --connect-timeout 5 --max-time "${_K2_POST_MAX_TIME:-30}" \
        -H "Content-Type: application/json" 2>/dev/null || r=$?; printf x; exit "$r")" || rc=$?
    out="${out%x}"
    # No status is read here: a 5xx from an earlier request must not count.
    _K2_HTTP=""
    _cli_no_answer "$rc" "$out" "$endpoint" "$t0" || true
    if [ "$rc" -eq 0 ] && _cli_retry_with_healed_passport "$out"; then
        cli_post_json_uds "$endpoint" "$body"
        return $?
    fi
    _permission_note "$out"
    printf '%s' "$out"
    return "$rc"
}

# Verbs the scoped per-cell socket actually serves (the daemon allowlist).
# Anything else stays on TCP+owner.
# _K2_UDS_TRY=1 (set by the caller for one send): try the socket for any
# route. The socket answers a route agents can't use with its refusal (a
# permission card when cards are on), where TCP has only a plain 403.
_uds_eligible() {
    [ "${_K2_UDS_TRY:-0}" = 1 ] && return 0
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
        # Postal (0.46.16): the agent's own address, friends, cards and
        # mail. Never code, settings or owner-link (people only: TCP).
        /cli/postal/status|/cli/postal/friends|/cli/postal/invite|/cli/postal/remove|/cli/postal/block) return 0 ;;
        /cli/postal/link-p5|/cli/postal/claim|/cli/postal/decide|/cli/postal/enable|/cli/postal/send|/cli/postal/route) return 0 ;;
        *) return 1 ;;
    esac
}

# cli_send <tcp_fn> <endpoint> [args...] — channel selector.
# When a per-cell scoped socket is present AND the endpoint is UDS-eligible,
# try the matching `<tcp_fn>_uds` arm; on success (rc==0, non-empty, and NOT
# the daemon's "route not served on cell socket" 404 sentinel) print + return.
# Otherwise fall through to the TCP arm `<tcp_fn>`, except for a POST (below).
#   UDS arm  → $SCOPED_TOKEN (Bearer)
#   TCP arm  → $TOKEN, which _cli_apply_auth_for_verb already chose:
#              Open tools may be disk-owner (COMPAT-58); ID/locked tools are
#              SCOPED_TOKEN only (never the owner inversion). For a route
#              the cell socket serves, in a session whose socket failed
#              (its file is missing, or it gave no answer), the TCP arm
#              runs with the passport, never the owner token from disk
#              (_k2_sock_fallback_token).
#
# 0.46.15: a POST goes again over TCP only when the cell socket took none
# of it: the connect failed (curl 7, or 75 when the OS refused it
# locally) or the daemon answered "not served on cell socket". Any other
# failure (a timeout, an empty or broken reply, 76 with TMPDIR unusable)
# may have run it, so it is not sent again: _cli_send_unanswered says so.
# GETs keep the fallback. _K2_SEND_NO_RESEND=1 gives a GET the POST rule too.
cli_send() {
    local fn="$1" endpoint="$2"; shift 2
    local sock; sock="$(_hook_sock)"
    # The no-answer line counts from here, cell-socket try included.
    local _K2_REQ_T0="${_K2_REQ_T0:-$SECONDS}"
    # After _apply_remote_host, every hop must use BASE_URL+Connect token.
    # UDS would hit THIS box's cell socket (Bug A: foreign staged path).
    if _cli_skip_uds; then
        "$fn" "$endpoint" "$@"
        return
    fi
    local out=""
    if [ -n "$sock" ] && [ -S "$sock" ] && _uds_eligible "$endpoint"; then
        # `|| rc=$?`, never `; rc=$?`: under `set -e` a cell socket that
        # refused or timed out ended the command right here, before the TCP
        # fallback, and the caller printed an empty answer (0.46.12:
        # `k2 inbox read` → "no usable answer: ''" from a session whose
        # socket nobody served).
        local rc=0 t0="$SECONDS"
        out="$(_K2_NOANSWER_SOFT=1 "${fn}_uds" "$endpoint" "$@")" || rc=$?
        if [ "$rc" -eq 0 ] && [ -n "$out" ] && [[ "$out" != *"not served on cell socket"* ]]; then
            printf '%s' "$out"
            return 0
        fi
        if _cli_send_is_post "$fn" || [ "${_K2_SEND_NO_RESEND:-0}" = 1 ]; then
            case "$rc" in
                7|"$_CLI_LOCAL_TRANSPORT_DENIED") ;;
                *)
                    if [[ "$out" != *"not served on cell socket"* ]]; then
                        _cli_send_unanswered "$endpoint" "$t0" "$rc" "$out"
                        return
                    fi
                    ;;
            esac
        fi
    fi
    if [ -n "$sock" ] && _uds_eligible "$endpoint"; then
        # The TCP arm reads $TOKEN: the passport in its place for this one
        # request when the socket failed in a session, then $TOKEN again.
        local _K2_TCP_TOKEN="${TOKEN:-}" _K2_TOKEN_SAVED="${TOKEN:-}" trc
        _k2_sock_fallback_token "$sock" "$out"
        TOKEN="$_K2_TCP_TOKEN"
        "$fn" "$endpoint" "$@"
        trc=$?
        TOKEN="$_K2_TOKEN_SAVED"
        return "$trc"
    fi
    "$fn" "$endpoint" "$@"
}

# The TCP arms cli_send takes. cli_request is the only GET; any other
# (cli_request_post, cli_post_form, cli_post_json, cli_post, or one added
# later) counts as a POST.
_cli_send_is_post() {
    [ "$1" != cli_request ]
}

# _cli_send_unanswered ENDPOINT T0 RC OUT: a send over the cell socket that
# may have run and is not sent again. A caller that reports for itself
# (_K2_NOANSWER_SOFT=1, as _k2_idem_send) gets OUT and RC back; otherwise
# the line below, and the command ends with 1.
_cli_send_unanswered() {
    if [ "${_K2_NOANSWER_SOFT:-0}" = 1 ]; then
        printf '%s' "$4"
        return "$3"
    fi
    echo "Error: K2's server didn't answer $1 ($((SECONDS - $2)) s); the request may have been applied. Check before running it again." >&2
    # A caller that pipes this send into a reader sets _K2_PIPE_MARK=1: the
    # reader then sees only this mark (never in a JSON answer: a raw control
    # byte) and stays quiet, so the line above is the only one even when the
    # reader runs before the command has ended. Other callers' stdout stays
    # clean.
    if [ "${_K2_PIPE_MARK:-0}" = 1 ]; then printf '%s' "$_K2_NOANSWER_MARK"; fi
    _k2_end_command 1
}
# What _cli_send_unanswered writes on stdout before it ends the command.
_K2_NOANSWER_MARK=$'\036k2:not-answered\036'

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
# GET payload = a pre-urlencoded query; POST payload = a JSON body. A GET
# query goes to curl as `data-raw`, sent as written: never read as a file.
# The JSON twin of cli_send that keeps the HTTP status (so callers can
# print the daemon's {"error","code"} and exit 3) and, unlike
# cli_request_post_uds, sends a real JSON body over the cell socket.
#   In a K2 terminal (passport + $K2_HOOK_SOCK): the cell socket with the
#   passport as Bearer, no `token=`. The daemon scopes the call to the
#   passport's own workspace.
#   Otherwise, or when the cell socket doesn't serve the route (an older
#   daemon answers "not served on cell socket"), loopback TCP with
#   $TOKEN as Bearer (owner outside K2; the passport inside, which the
#   owner-only routes refuse with 403 rather than elevate). In a session
#   whose cell socket failed, that request carries the passport, never
#   the owner token from disk (_k2_sock_fallback_token).
# Credentials and bodies ride curl config on stdin (_k2_curl), never argv.
# Status 000 = the daemon did not answer.
# Each request may take up to 60 s; a caller whose route blocks longer
# (k2 sim boot/install/wait) sets _K2_SEND_MAX_TIME for that one call.
cli_send_json() {
    local method="$1" path="$2" payload="${3:-}"
    local mt="${_K2_SEND_MAX_TIME:-60}"
    local sock out="" _K2_CFG="" rc=0 t0="$SECONDS"
    sock="$(_hook_sock)"
    if ! _cli_skip_uds && [ -n "$sock" ] && [ -S "$sock" ] && [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        if [ "$method" = "GET" ]; then
            [ -n "$payload" ] && _k2_cfg_add data-raw "$payload"
            out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time "$mt" -w $'\n%{http_code}' 2>/dev/null)" || rc=$?
        else
            _k2_cfg_add data-raw "$payload"
            out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time "$mt" -H "Content-Type: application/json" \
                -w $'\n%{http_code}' 2>/dev/null)" || rc=$?
        fi
        if [ "$rc" -eq 0 ] && [ -n "$out" ] && [[ "$out" != *"not served on cell socket"* ]] && [[ "$out" != *$'\n000' ]]; then
            if _cli_retry_with_healed_passport "$out"; then
                cli_send_json "$method" "$path" "$payload"
                return $?
            fi
            _permission_note "$out"
            printf '%s' "$out"
            return 0
        fi
        # 0.46.15: a POST goes again over TCP only after a failed connect
        # (curl 7) or "not served on cell socket", as cli_send. Soft
        # callers get status 000, the TCP arm's "no answer".
        if [ "$method" != "GET" ] && [ "$rc" -ne 7 ] && [[ "$out" != *"not served on cell socket"* ]]; then
            _cli_send_unanswered "$path" "$t0" 0 $'\n000'
            return
        fi
    fi
    # TCP: in a session whose cell socket failed, the passport, never the
    # owner token from disk (_k2_sock_fallback_token).
    local _K2_TCP_TOKEN="${TOKEN:-}"
    _k2_sock_fallback_token "$sock" "$out"
    _K2_CFG=""
    if [ "$method" = "GET" ]; then
        _k2_cfg_auth "$_K2_TCP_TOKEN"
        [ -n "$payload" ] && _k2_cfg_add data-raw "$payload"
        out="$(_k2_curl "$_K2_CFG" -s -G "${BASE_URL}${path}" \
            --connect-timeout 5 --max-time "$mt" -w $'\n%{http_code}' 2>/dev/null || printf '\n000'; printf x)"
    else
        _k2_cfg_add url "${BASE_URL}${path}"
        _k2_cfg_auth "$_K2_TCP_TOKEN"
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
# keyed by the CLI's own pid ($$ is the parent's pid in a subshell). It
# lives in $TMPDIR when that is a folder this user can write, else in /tmp
# or /var/tmp. Every write to it is silent: with no folder to write, there
# is no note and no closing line, never an error line or a line printed
# before the command's own output.
_k2_note_dir() {
    local d
    for d in "${TMPDIR:-}" /tmp /var/tmp; do
        if [ -n "$d" ] && [ -d "$d" ] && [ -w "$d" ] && [ -x "$d" ]; then
            _K2_NOTE_DIR="${d%/}"
            [ -n "$_K2_NOTE_DIR" ] || _K2_NOTE_DIR="/"
            return 0
        fi
    done
    _K2_NOTE_DIR="/tmp"
}
_k2_note_dir
_K2_PERMISSION_NOTE="${_K2_NOTE_DIR%/}/k2-permission-note.$$"

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
p = v.get("permission") if isinstance(v, dict) else None; p = dict(p, command="k2 <tail>".replace("<tail>", p["tail"])) if isinstance(p, dict) and isinstance(p.get("tail"), str) and p["tail"] else p
if not isinstance(p, dict):
    sys.exit(0)
cmd = p.get("command")
if isinstance(cmd, str) and cmd:
    print("Ask with: " + cmd)
elif p.get("requestable") is False:
    hint = p.get("hint")
    if isinstance(hint, str) and hint:
        print(hint)
    print("It can'"'"'t be requested.", *([__import__("os").environ["_K2_NO_CARD_FEEDBACK"]] if p.get("feedback") is True else []), sep="\n")
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
                { printf '%s %s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" >"${_K2_PERMISSION_NOTE}.bulletin"; } 2>/dev/null || true
            fi
            ;;
    esac
    # A passport refused: the flush checks whether it is really dead.
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        case "${1:-}" in *"Invalid or missing auth token"*) { : >"${_K2_PERMISSION_NOTE}.passport"; } 2>/dev/null || true ;; esac
    fi
    # A lookup on the way to the command's own request (the sender's name)
    # is no refusal of the command: never its closing line (0.46.15).
    [ "${_K2_PERMISSION_QUIET:-0}" = 1 ] && return 0
    case "${1:-}" in *'"permission"'*) ;; *) return 0 ;; esac
    local lines
    lines="$(_permission_closing_lines "$1")" || return 0
    [ -n "$lines" ] || return 0
    # Kept for the exit trap, which prints it only when the command failed.
    { printf '%s\n' "$lines" >"$_K2_PERMISSION_NOTE"; } 2>/dev/null || true
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
    stamp="${_K2_NOTE_DIR%/}/k2-bulletin.${key}"
    now="$(date +%s)"
    last=""
    [ -f "$stamp" ] && read -r last <"$stamp" 2>/dev/null
    case "$last" in ''|*[!0-9]*) last=0 ;; esac
    if [ $((now - last)) -ge 1800 ]; then
        echo "K2 bulletin for ${day} is unread: k2 organization bulletin" >&2
        { printf '%s\n' "$now" >"$stamp"; } 2>/dev/null || true
    fi
    return 0
}
# The closing line belongs to the request whose answer the command reports.
# A send calls this right before it goes out, so a refusal a lookup met on
# the way (the sender's own name: `/cli/workspace/handle` refuses a
# passport over TCP) never closes the send's output. 0.46.15 field report:
# `k2 msg` and `k2 thread ask` ended with "It can't be requested." under a
# delivery that waited on folder trust, a timeout and a not_found address.
_permission_note_reset() {
    rm -f "$_K2_PERMISSION_NOTE" 2>/dev/null || true
}
# END_CLI_PERMISSION_NOTE

# BEGIN_CLI_SEND_ONCE
# Idempotent sends (0.46.15). `k2 msg` (live, --inbox-*, agent::host),
# `k2 thread "text"` and `k2 thread ask` send one key per invocation: a
# random UUID, or the one `--idempotency-key` names. The daemon answers a
# repeat of a key with the first answer (same deliveryId / card / package)
# and delivers nothing again (`idempotency_key`; a Thread post's own
# `clientMsgId`). So a send that got no answer is asked again with the
# SAME key, a few times; then the command says it may have landed and how
# to ask once more without a duplicate.
#
# Only a daemon that lists `send-once` in /boot-status `features` dedupes
# keys. Any other (an older one mid-update, a --host server) gets the send
# once: no retry, no cell-socket-to-TCP resend after a try that may have
# run, and on no answer "may have been delivered; check before resending".
_K2_IDEM_KEY=""
# "" not asked yet, 1 the daemon dedupes keys, 0 it doesn't (or didn't say).
_K2_SEND_ONCE=""
_k2_send_once_supported() {
    if [ -z "$_K2_SEND_ONCE" ]; then
        local _K2_CFG="" body=""
        _k2_cfg_auth "${TOKEN:-}"
        body="$(_K2_CURL_TRIES=1 _k2_curl "$_K2_CFG" -s -G --connect-timeout 3 --max-time 5 "${BASE_URL}/boot-status" 2>/dev/null)" || body=""
        case "$body" in
            *'"send-once"'*) _K2_SEND_ONCE=1 ;;
            *) _K2_SEND_ONCE=0 ;;
        esac
    fi
    [ "$_K2_SEND_ONCE" = 1 ]
}
_k2_idem_valid() {
    [[ "${1:-}" =~ ^[A-Za-z0-9_-]{8,64}$ ]]
}
# _k2_idem_init [KEY]: the key for the send about to go (KEY when given,
# else a new one: each send of an invocation is its own).
_k2_idem_init() {
    if [ -n "${1:-}" ]; then
        if ! _k2_idem_valid "$1"; then
            echo "Error: --idempotency-key takes 8-64 characters of A-Z, a-z, 0-9, '-' or '_' (the key a 'may have been delivered' line printed)" >&2
            exit 2
        fi
        _K2_IDEM_KEY="$1"
        _k2_send_once_supported || true
        return 0
    fi
    # Asked here, in the command's own shell, so every send of the run
    # reuses the answer.
    _k2_send_once_supported || true
    _K2_IDEM_KEY="$(python3 -c 'import uuid; print(uuid.uuid4())' 2>/dev/null)" || _K2_IDEM_KEY=""
    if ! _k2_idem_valid "$_K2_IDEM_KEY"; then
        echo "Error: k2 could not make a send key (python3 failed or is missing); nothing was sent." >&2
        exit 1
    fi
}
# True when a send got no answer: curl timed out (28), the connection
# closed empty (52) or broke while receiving (56), nothing came back, or
# the daemon says the first try with this key still runs (409 in_progress).
_k2_idem_unanswered() {
    local rc="$1" out="$2"
    case "$rc" in 28|52|56) return 0 ;; esac
    [ "$rc" = 0 ] && [ -z "$out" ] && return 0
    [[ "$out" == *'"code":"in_progress"'* ]] && return 0
    return 1
}
# _k2_idem_send <transport> <endpoint> [args…]: cli_send, asked again with
# the same key while unanswered ($_K2_IDEM_TRIES tries, default 3), then the
# "may have been delivered" line and exit 1. The caller put the key in args.
_k2_idem_send() {
    local endpoint="$2" tries="${_K2_IDEM_TRIES:-3}" n=1 out rc t0="$SECONDS"
    case "$tries" in ''|*[!0-9]*|0) tries=3 ;; esac
    if ! _k2_send_once_supported; then
        rc=0
        out="$(_K2_NOANSWER_SOFT=1 _K2_SEND_NO_RESEND=1 cli_send "$@")" || rc=$?
        if ! _k2_idem_unanswered "$rc" "$out"; then
            printf '%s' "$out"
            return "$rc"
        fi
        echo "Error: K2's server didn't answer ${endpoint} ($((SECONDS - t0)) s). It may have been delivered; check before resending (this server doesn't say it drops a repeated send, so k2 didn't try again)." >&2
        _k2_end_command 1
    fi
    while :; do
        rc=0
        out="$(_K2_NOANSWER_SOFT=1 cli_send "$@")" || rc=$?
        if ! _k2_idem_unanswered "$rc" "$out"; then
            printf '%s' "$out"
            return "$rc"
        fi
        [ "$n" -ge "$tries" ] && break
        n=$((n + 1))
        sleep 1
    done
    echo "Error: K2's server didn't answer ${endpoint} (${n} tries, $((SECONDS - t0)) s); it may have been delivered. It is safe to run the same command again with --idempotency-key ${_K2_IDEM_KEY}: K2 delivers one key once." >&2
    _k2_end_command 1
}
# A repeat the daemon answered from its first try: say so on stderr.
_k2_idem_replay_note() {
    case "${1:-}" in
        *'"replayed":true'*|*'"duplicate":true'*)
            echo "note: K2 already had this send (same idempotency key); it was not delivered again." >&2 ;;
    esac
    return 0
}
# END_CLI_SEND_ONCE

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
    [ -z "${K5_PROJECT_PATH:-${K2_PROJECT_PATH:-${K2SO_PROJECT_PATH:-}}}" ] || return 0
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
        _k2_cfg_auth "$SCOPED_TOKEN"
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

