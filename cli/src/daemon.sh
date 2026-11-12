# ── Daemon lifecycle ─────────────────────────────────────────────────
# These verbs wrap `launchctl` directly because the daemon might not be
# running when they're invoked (so we can't go through its HTTP surface).
# `install` / `uninstall` live in the Tauri app's Settings pane — we do
# NOT duplicate that wiring here; the CLI only does start/stop/restart
# against an already-installed plist.

# dev.k2.daemon post-0.40; fall back to the pre-rename label when only
# the old plist is installed (machine not yet migrated).
_daemon_label="dev.k2.daemon"
_daemon_plist="$HOME/Library/LaunchAgents/${_daemon_label}.plist"
if [ ! -f "$_daemon_plist" ] && [ -f "$HOME/Library/LaunchAgents/com.k2so.k2so-daemon.plist" ]; then
    _daemon_label="com.k2so.k2so-daemon"
    _daemon_plist="$HOME/Library/LaunchAgents/${_daemon_label}.plist"
fi
_daemon_port_file="$K2_HOME/daemon.port"
_daemon_token_file="$K2_HOME/daemon.token"
_daemon_stdout_log="$K2_HOME/daemon.stdout.log"

# k2-ptyd S3 (prd-k2-ptyd-v1 §7.6, P54, P82): quitting the daemon ends its
# agents; restarting keeps them. A stop leaves a short-lived stop intent the
# daemon reads at shutdown (`$K2_HOME/run/stop-intent.json`, 0600, honoured
# for 120 s, deleted on read), so a supervisor's SIGTERM is known to be a
# stop and not a restart.
_daemon_write_stop_intent() {
    local by="${1:-cli}" dir="$K2_HOME/run" now tmp
    now=$(date +%s)
    mkdir -p "$dir" 2>/dev/null || return 1
    chmod 700 "$dir" 2>/dev/null || true
    tmp="$dir/stop-intent.json.tmp.$$"
    ( umask 077; printf '{"by":"%s","at":%s,"nonce":"%s-%s"}\n' "$by" "$now" "$$" "$now" > "$tmp" ) || return 1
    mv -f "$tmp" "$dir/stop-intent.json"
}

# OS family for the daemon lifecycle verbs: macos | linux | windows.
_daemon_os() {
    case "$(uname -s)" in
        Darwin) echo macos ;;
        Linux) echo linux ;;
        MINGW*|MSYS*|CYGWIN*|Windows_NT) echo windows ;;
        *) echo other ;;
    esac
}

# Linux: "<scope> <unit>" of the K2 daemon's systemd unit (scope = user or
# system), or nothing. The running daemon's own cgroup names it (any unit
# name the provisioning used); otherwise the known names are tried, current
# before legacy.
_daemon_linux_unit() {
    local pid="" cg="" unit="" scope="system" u
    [ -f "$K2_HOME/daemon.lock" ] && pid=$(tr -dc '0-9' < "$K2_HOME/daemon.lock" 2>/dev/null)
    if [ -n "$pid" ] && [ -r "/proc/$pid/cgroup" ]; then
        cg=$(grep '^0::' "/proc/$pid/cgroup" 2>/dev/null | head -1)
        unit=$(printf '%s\n' "${cg#0::}" | tr '/' '\n' | grep '\.service$' | grep -v '^user@' | tail -1)
        case "$cg" in */user@*) scope=user ;; esac
        if [ -n "$unit" ]; then
            echo "$scope $unit"
            return 0
        fi
    fi
    command -v systemctl >/dev/null 2>&1 || return 1
    for u in k2-daemon.service k2so-daemon.service; do
        if systemctl --user cat "$u" >/dev/null 2>&1; then echo "user $u"; return 0; fi
    done
    for u in k2-daemon.service k2so-daemon.service; do
        if systemctl cat "$u" >/dev/null 2>&1; then echo "system $u"; return 0; fi
    done
    return 1
}

# _daemon_systemctl <scope> <args…>
_daemon_systemctl() {
    local scope="$1"; shift
    if [ "$scope" = "user" ]; then
        systemctl --user "$@"
    else
        systemctl "$@"
    fi
}

# POST a lifecycle route on the LOCAL daemon with the on-disk owner token
# (query string, never argv). Prints the body; returns curl's status.
_daemon_local_post() {
    local path="$1" token="" _K2_CFG=""
    [ -f "$_daemon_token_file" ] && token=$(tr -d '[:space:]' < "$_daemon_token_file" 2>/dev/null)
    if [ -z "$token" ]; then
        echo "no owner token at $_daemon_token_file" >&2
        return 1
    fi
    case "$path" in *\?*) path="${path}&token=${token}" ;; *) path="${path}?token=${token}" ;; esac
    _k2_cfg_add url "${BASE_URL}${path}"
    _k2_curl "$_K2_CFG" -s -X POST --connect-timeout 5 --max-time 30
}

# The k2-ptyd numbers `k2 daemon survive status` shows: whether agents
# survive restarts here, and how many came back at the last boot. The
# daemon on this machine answers /boot-status with the full view on
# loopback, so no token is sent. Prints "<ptyd line><TAB><adopted>", or
# nothing when the daemon doesn't answer or reports no ptyd (switch off and
# nothing adopted).
_daemon_status_ptyd() {
    local port="$1" body
    [ -n "$port" ] || return 0
    body=$(_K2_CURL_TRIES=1 _k2_curl "" -s --connect-timeout 2 --max-time 4 "http://127.0.0.1:${port}/boot-status" 2>/dev/null) || return 0
    printf '%s' "$body" | python3 -c '
import json, sys
try:
    b = json.loads(sys.stdin.read() or "null")
except Exception:
    sys.exit(0)
if isinstance(b, dict) and isinstance(b.get("ptyd"), str):
    a = b.get("adopted")
    print("%s\t%d" % (b["ptyd"].replace("\t", " "), a if isinstance(a, int) else 0))
' 2>/dev/null || true
}

# Print the two status lines for _daemon_status_ptyd's answer ($1).
_daemon_status_ptyd_lines() {
    [ -n "$1" ] || return 0
    echo "  Agents survive: ${1%%$'\t'*}  (k2-ptyd; k2 daemon survive status)"
    echo "  Came back at the last restart: ${1##*$'\t'}"
}

cmd_daemon_status() {
    local as_json=0
    [ "${1:-}" = "--json" ] && as_json=1
    if [ "$(_daemon_os)" = "linux" ]; then
        _daemon_status_linux "$as_json"
        return $?
    fi

    local installed="no"
    [ -f "$_daemon_plist" ] && installed="yes"

    # launchctl `print` returns info on the loaded service, including
    # the PID when it's running. Extract just the PID for a clean one-
    # line summary.
    local uid pid="" running="no" last_exit_code=""
    uid=$(id -u)
    local print_output
    print_output=$(launchctl print "gui/${uid}/${_daemon_label}" 2>/dev/null || true)
    if [ -n "$print_output" ]; then
        # POSIX classes, not \s — macOS ships BWK awk where \s never
        # matches, which made "Running: no" while the daemon was up.
        pid=$(printf '%s\n' "$print_output" | awk -F'=' '/^[[:space:]]*pid[[:space:]]*=/{gsub(/ /,"",$2); print $2; exit}')
        last_exit_code=$(printf '%s\n' "$print_output" | awk -F'=' '/^[[:space:]]*last exit code[[:space:]]*=/{gsub(/^[ \t]+|[ \t]+$/,"",$2); print $2; exit}')
        [ -n "$pid" ] && [ "$pid" != "0" ] && running="yes"
    fi

    local port="" token_present="no" ptyd=""
    [ -f "$_daemon_port_file" ] && port=$(cat "$_daemon_port_file" 2>/dev/null | tr -d '[:space:]')
    [ -f "$_daemon_token_file" ] && [ -s "$_daemon_token_file" ] && token_present="yes"
    ptyd=$(_daemon_status_ptyd "$port")

    if [ "$as_json" = "1" ]; then
        printf '{"installed":"%s","running":"%s","pid":"%s","port":"%s","token":"%s","last_exit_code":"%s","plist":"%s","log":"%s","ptyd":"%s","adopted":"%s"}\n' \
            "$installed" "$running" "${pid:-}" "${port:-}" "$token_present" "${last_exit_code:-}" "$_daemon_plist" "$_daemon_stdout_log" \
            "$([ -n "$ptyd" ] && printf '%s' "${ptyd%%$'\t'*}")" "$([ -n "$ptyd" ] && printf '%s' "${ptyd##*$'\t'}")"
        return 0
    fi

    echo "K2 Daemon"
    echo "  Installed:     $installed  ($_daemon_plist)"
    echo "  Running:       $running$([ "$running" = "yes" ] && echo "  (PID $pid)")"
    if [ "$running" = "yes" ]; then
        [ -n "$port" ] && echo "  Port:          $port"
        echo "  Auth token:    $([ "$token_present" = "yes" ] && echo "present" || echo "missing")"
    fi
    _daemon_status_ptyd_lines "$ptyd"
    if [ -n "$last_exit_code" ] && [ "$last_exit_code" != "0" ]; then
        echo "  Last exit code: $last_exit_code  (launchd restarted after a non-zero exit)"
    fi
    echo "  Log file:      $_daemon_stdout_log"
    if [ "$installed" = "no" ]; then
        echo ""
        echo "  Daemon plist is not installed. Open K2 → Settings → General → K2 Daemon"
        echo "  to install the launch agent (dev builds require K2SO_INSTALL_DAEMON=1)."
    fi
}

# P82: `k2 daemon status` on Linux (systemd user or system unit, or none).
_daemon_status_linux() {
    local as_json="$1" su="" scope="" unit="" active="unknown" pid="" port="" token_present="no"
    su=$(_daemon_linux_unit) || su=""
    if [ -n "$su" ]; then
        scope=${su%% *}; unit=${su#* }
        active=$(_daemon_systemctl "$scope" is-active "$unit" 2>/dev/null) || true
        pid=$(_daemon_systemctl "$scope" show -p MainPID --value "$unit" 2>/dev/null) || true
        [ "$pid" = "0" ] && pid=""
    fi
    [ -f "$_daemon_port_file" ] && port=$(tr -d '[:space:]' < "$_daemon_port_file" 2>/dev/null)
    [ -f "$_daemon_token_file" ] && [ -s "$_daemon_token_file" ] && token_present="yes"
    local ptyd
    ptyd=$(_daemon_status_ptyd "$port")
    if [ "$as_json" = "1" ]; then
        printf '{"installed":"%s","running":"%s","pid":"%s","port":"%s","token":"%s","unit":"%s","scope":"%s","log":"%s","ptyd":"%s","adopted":"%s"}\n' \
            "$([ -n "$unit" ] && echo yes || echo no)" "$([ "$active" = "active" ] && echo yes || echo no)" \
            "${pid:-}" "${port:-}" "$token_present" "${unit:-}" "${scope:-}" "$_daemon_stdout_log" \
            "$([ -n "$ptyd" ] && printf '%s' "${ptyd%%$'\t'*}")" "$([ -n "$ptyd" ] && printf '%s' "${ptyd##*$'\t'}")"
        return 0
    fi
    echo "K2 Daemon"
    if [ -z "$unit" ]; then
        echo "  Unit:          none found (k2-daemon.service, user or system)"
    else
        echo "  Unit:          $unit ($scope)"
        echo "  State:         ${active:-unknown}$([ -n "$pid" ] && echo "  (PID $pid)")"
    fi
    [ -n "$port" ] && echo "  Port:          $port"
    echo "  Auth token:    $([ "$token_present" = "yes" ] && echo "present" || echo "missing")"
    _daemon_status_ptyd_lines "$ptyd"
}

cmd_daemon_start() {
    if [ ! -f "$_daemon_plist" ]; then
        echo "Daemon plist not installed at $_daemon_plist" >&2
        echo "Open K2 → Settings → General → K2 Daemon → Install." >&2
        exit 1
    fi
    # `launchctl load -w` is persistent across reboots. On an already-
    # loaded plist it returns a harmless "service already loaded" which
    # we swallow.
    local err
    err=$(launchctl load -w "$_daemon_plist" 2>&1) || true
    # The menu-bar icon's Quit daemon also unloads the icon's own job. Load
    # it again when the desktop app installed it on this Mac (never installs).
    local helper_plist="$HOME/Library/LaunchAgents/dev.k2.menubar.plist"
    if [ -f "$helper_plist" ]; then
        launchctl load -w "$helper_plist" >/dev/null 2>&1 || true
    fi
    if echo "$err" | grep -q "service already loaded\|Load failed: 37"; then
        echo "Daemon already running."
        return 0
    fi
    if [ -n "$err" ]; then
        echo "launchctl load: $err" >&2
    fi
    echo "Daemon started."
}

# Quitting the daemon ENDS its agent sessions (prd-k2-ptyd-v1 L2); use
# `k2 daemon restart` to keep them running.
cmd_daemon_stop() {
    case "$(_daemon_os)" in
        linux)
            local su scope unit
            su=$(_daemon_linux_unit) || su=""
            _daemon_write_stop_intent cli || true
            if [ -z "$su" ]; then
                # No systemd unit (a supervisor-less box, dev): ask the daemon.
                if _daemon_local_post /cli/daemon/stop 2>/dev/null | grep -q '"stopping":true'; then
                    echo "Daemon stopped (its agents were ended)."
                    return 0
                fi
                rm -f "$K2_HOME/run/stop-intent.json"
                echo "Daemon was not running (no systemd unit, and nothing answered at $BASE_URL)."
                return 0
            fi
            scope=${su%% *}; unit=${su#* }
            if _daemon_systemctl "$scope" stop "$unit"; then
                echo "Daemon stopped ($unit; its agents were ended)."
                return 0
            fi
            rm -f "$K2_HOME/run/stop-intent.json"
            if [ "$scope" = "system" ]; then
                echo "Stopping a system unit needs root: sudo systemctl stop $unit" >&2
            fi
            exit 1
            ;;
        windows)
            if _daemon_local_post /cli/daemon/stop 2>/dev/null | grep -q '"stopping":true'; then
                echo "Daemon stopped (its agents were ended)."
                return 0
            fi
            echo "daemon stop: the daemon did not answer at $BASE_URL" >&2
            exit 1
            ;;
    esac
    if [ ! -f "$_daemon_plist" ]; then
        echo "Daemon plist not installed; nothing to stop."
        return 0
    fi
    _daemon_write_stop_intent cli || true
    local err
    err=$(launchctl unload -w "$_daemon_plist" 2>&1) || true
    if echo "$err" | grep -qi "Could not find\|No such"; then
        echo "Daemon was not running."
        return 0
    fi
    if [ -n "$err" ]; then
        echo "launchctl unload: $err" >&2
    fi
    echo "Daemon stopped."
}

# Poll <base>/boot-status until phase=ready (or timeout). On success prints
# "Daemon back up on <version>" and returns 0; on timeout prints a message
# and returns 1. boot-status answers without a token, but a remote host in
# ghost mode tells only a signed-in caller its version (PRD boot-status-
# ghost), so the token rides `Authorization: Bearer` (curl config on stdin,
# never argv) when one is given.
# Args: $1 = base URL (no trailing slash), $2 = token (optional).
# Polls every ~2s up to ~60s.
_daemon_wait_ready() {
    local base="$1" token="${2:-}"
    local attempt=0 max_attempts=30 status_json phase version _K2_CFG=""
    _k2_cfg_add url "${base}/boot-status"
    if [ -n "$token" ]; then
        _k2_cfg_add header "Authorization: Bearer ${token}"
    fi
    # Give the supervisor a moment to tear down the old process before the
    # first poll, so we don't immediately see the OUTGOING daemon as ready.
    sleep 2
    while [ "$attempt" -lt "$max_attempts" ]; do
        status_json=$(_k2_curl "$_K2_CFG" -s \
            --connect-timeout 3 --max-time 5 2>/dev/null) || true
        case "$status_json" in
            *'"phase":"ready"'*|*'"phase": "ready"'*)
                version=$(printf '%s' "$status_json" \
                    | grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' \
                    | head -1 \
                    | sed 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
                if [ -n "$version" ]; then
                    echo "Daemon back up on ${version}"
                else
                    echo "Daemon back up (version unknown)"
                fi
                return 0
                ;;
        esac
        attempt=$((attempt + 1))
        sleep 2
    done
    echo "daemon restart --wait: timed out after ~60s waiting for ${base} to report phase=ready" >&2
    return 1
}

cmd_daemon_restart() {
    # Two modes:
    #   LOCAL  (default): launchctl kickstart the local launch agent.
    #   REMOTE (--host):  POST /cli/daemon/restart to a remote K2 daemon
    #                     (e.g. over K2 Connect at https://<sub>.k2.dev),
    #                     no GUI required. The remote route is supervisor-
    #                     agnostic — it triggers graceful shutdown and the
    #                     remote supervisor (launchd/systemd) respawns it.
    local host="" token="" wait=0 end_agents=0
    while [ $# -gt 0 ]; do
        case "$1" in
            # k2-ptyd P57: a restart keeps agent sessions running; with
            # --sessions they are ended and start fresh.
            --sessions) end_agents=1; shift ;;
            --host)  host="${2:-}";  shift 2 ;;
            --host=*) host="${1#--host=}"; shift ;;
            --token) token="${2:-}"; shift 2 ;;
            --token=*) token="${1#--token=}"; shift ;;
            --wait) wait=1; shift ;;
            *) echo "daemon restart: unknown argument: $1" >&2; exit 2 ;;
        esac
    done

    # ── REMOTE restart ────────────────────────────────────────────────
    if [ -n "$host" ]; then
        # Default the owner token from ~/.k2/daemon.token when --token
        # is omitted (works when targeting the local daemon's own base URL).
        if [ -z "$token" ]; then
            if [ -f "$_daemon_token_file" ] && [ -s "$_daemon_token_file" ]; then
                token=$(cat "$_daemon_token_file" 2>/dev/null | tr -d '[:space:]')
            fi
        fi
        if [ -z "$token" ]; then
            echo "daemon restart --host: no owner token (pass --token <tok> or populate $_daemon_token_file)" >&2
            exit 1
        fi
        # Strip a trailing slash so we don't emit a double-slash URL.
        local base="${host%/}"
        # OWNER-ONLY route: token rides the query string (the daemon's
        # require_owner reads it there), no body.
        local resp _K2_CFG=""
        local q="token=${token}"
        [ "$end_agents" = "1" ] && q="${q}&end_agents=1"
        _k2_cfg_add url "${base}/cli/daemon/restart?${q}"
        resp=$(_k2_curl "$_K2_CFG" -s -X POST \
            --connect-timeout 5 --max-time 30 2>/dev/null) || true
        if [ -z "$resp" ]; then
            echo "daemon restart --host: no response from ${base} (unreachable, or the daemon exited before flushing)" >&2
            exit 1
        fi
        # Friendly parse: success when the JSON carries "restarting":true.
        case "$resp" in
            *'"restarting":true'*)
                echo "Remote daemon at ${base} is restarting."
                echo "$resp"
                if [ "$wait" = "1" ]; then
                    _daemon_wait_ready "$base" "$token" || exit 1
                fi
                return 0
                ;;
            *'"error"'*|*'403'*|*'405'*)
                echo "daemon restart --host: rejected by ${base}: $resp" >&2
                exit 1
                ;;
            *)
                echo "daemon restart --host: unexpected response from ${base}: $resp" >&2
                exit 1
                ;;
        esac
    fi

    # ── LOCAL restart ─────────────────────────────────────────────────
    # --sessions: the daemon's own route ends the agents, then the
    # supervisor respawns it (every OS).
    if [ "$end_agents" = "1" ]; then
        if _daemon_local_post "/cli/daemon/restart?end_agents=1" 2>/dev/null | grep -q '"restarting":true'; then
            echo "Daemon restarting; its agent sessions were ended."
            [ "$wait" = "1" ] && { _daemon_wait_ready "$BASE_URL" || exit 1; }
            return 0
        fi
        echo "daemon restart --sessions: the daemon did not answer at $BASE_URL" >&2
        exit 1
    fi
    case "$(_daemon_os)" in
        linux|windows)
            local su="" scope unit
            [ "$(_daemon_os)" = "linux" ] && { su=$(_daemon_linux_unit) || su=""; }
            if [ -n "$su" ]; then
                scope=${su%% *}; unit=${su#* }
                if ! _daemon_systemctl "$scope" restart "$unit"; then
                    [ "$scope" = "system" ] && echo "Restarting a system unit needs root: sudo systemctl restart $unit" >&2
                    exit 1
                fi
                echo "Daemon restarted ($unit)."
            elif _daemon_local_post /cli/daemon/restart 2>/dev/null | grep -q '"restarting":true'; then
                echo "Daemon restarting."
            else
                echo "daemon restart: no systemd unit found and the daemon did not answer at $BASE_URL" >&2
                exit 1
            fi
            [ "$wait" = "1" ] && { _daemon_wait_ready "$BASE_URL" || exit 1; }
            return 0
            ;;
    esac
    local uid
    uid=$(id -u)
    # `launchctl kickstart -k` sends SIGTERM and respawns (thanks to
    # KeepAlive=true). Cleaner than unload+load because the plist
    # config stays untouched.
    local err
    err=$(launchctl kickstart -k "gui/${uid}/${_daemon_label}" 2>&1) || true
    if echo "$err" | grep -qi "Could not find"; then
        # Fall through to a plain start for users who called `restart`
        # before the daemon was ever loaded.
        cmd_daemon_start
        return $?
    fi
    if [ -n "$err" ]; then
        echo "launchctl kickstart: $err" >&2
    fi
    echo "Daemon restarted."
    if [ "$wait" = "1" ]; then
        # Local daemon's own base URL (PORT is resolved at CLI startup).
        _daemon_wait_ready "$BASE_URL" || exit 1
    fi
}

# k2-ptyd (prd-k2-ptyd-v1 P67; on by default since 0.46.5): `k2 daemon
# survive [on|off|status]`. With it on, NEW agent sessions run under k2-ptyd and
# keep running through daemon updates, restarts and crashes; quitting the
# daemon still ends them. The switch is the daemon's own `terminal.ptyd`
# setting (Settings → General → Server shows the same toggle).
cmd_daemon_survive() {
    local sub="${1:-status}" resp
    case "$sub" in
        on|off)
            resp=$(cli_post_json "/cli/settings/update" "{\"terminal\":{\"ptyd\":\"$sub\"}}")
            printf '%s' "$resp" | SV_WANT="$sub" python3 -c '
import json, os, sys
try:
    d = json.loads(sys.stdin.read() or "null")
except Exception:
    print("Error: the K2 daemon gave no usable answer", file=sys.stderr); sys.exit(1)
if not isinstance(d, dict) or d.get("error"):
    print("Error: %s" % (d.get("error") if isinstance(d, dict) else d), file=sys.stderr); sys.exit(3)
got = ((d.get("terminal") or {}).get("ptyd") or "off")
if got != os.environ["SV_WANT"]:
    print("Error: the daemon kept terminal.ptyd=%s" % got, file=sys.stderr); sys.exit(1)
if got == "on":
    print("Agents keep running through updates and restarts: on.")
    print("New agent sessions run under k2-ptyd; sessions already running stop at the next restart.")
else:
    print("Agents keep running through updates and restarts: off. New sessions stop with the daemon.")
' || exit $?
            ;;
        status)
            resp=$(cli_request "/cli/settings/get")
            local boot
            boot=$(_k2_curl "" -s --connect-timeout 3 --max-time 5 "${BASE_URL}/boot-status?token=${TOKEN}" 2>/dev/null || true)
            printf '%s\n%s' "$resp" "$boot" | python3 -c '
import json, sys
lines = sys.stdin.read().split("\n", 1)
try:
    s = json.loads(lines[0])
except Exception:
    print("Error: the K2 daemon gave no usable answer", file=sys.stderr); sys.exit(1)
setting = ((s.get("terminal") or {}).get("ptyd") or "")
effective = "off" if setting.strip().lower() == "off" else "on"
print("Setting:  terminal.ptyd = %s (%s)" % (setting or "unset", effective))
try:
    b = json.loads(lines[1]) if len(lines) > 1 and lines[1].strip() else {}
except Exception:
    b = {}
if "ptyd" in b:
    print("Backend:  %s" % b["ptyd"])
    print("Adopted at the last boot: %s" % b.get("adopted", 0))
'
            ;;
        -h|--help|help)
            echo "Usage: k2 daemon survive [on|off|status]"
            echo "  On by default: agents keep running through K2 updates, restarts and"
            echo "  crashes (macOS and Linux). Quitting the daemon still ends them."
            echo "  'off' makes new sessions stop with the daemon again."
            ;;
        *)
            echo "Usage: k2 daemon survive [on|off|status]" >&2
            exit 2
            ;;
    esac
}

cmd_daemon_log() {
    local tail_n="200"
    local follow=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --tail|-n) tail_n="$2"; shift 2 ;;
            --follow|-f) follow=1; shift ;;
            *) shift ;;
        esac
    done
    if [ ! -f "$_daemon_stdout_log" ]; then
        echo "No daemon log yet at $_daemon_stdout_log"
        return 0
    fi
    if [ "$follow" = "1" ]; then
        tail -n "$tail_n" -f "$_daemon_stdout_log"
    else
        tail -n "$tail_n" "$_daemon_stdout_log"
    fi
}

cmd_daemon_uninstall() {
    local force=0
    [ "${1:-}" = "--yes" ] || [ "${1:-}" = "-y" ] && force=1
    if [ "$force" != "1" ]; then
        echo "This will remove the K2 daemon launch agent and stop the daemon."
        echo "Your workspace data (~/.k2 database + agent files) is preserved."
        printf "Proceed? [y/N] "
        read -r answer
        case "$answer" in
            y|Y|yes|YES) ;;
            *) echo "Cancelled."; return 1 ;;
        esac
    fi
    if [ -f "$_daemon_plist" ]; then
        launchctl unload -w "$_daemon_plist" 2>/dev/null || true
        rm -f "$_daemon_plist"
    fi
    rm -f "$_daemon_port_file" "$_daemon_token_file"
    echo "Daemon uninstalled."
}

