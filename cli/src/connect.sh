# ── K2 Connect — Pro multi-subdomain (PRD k2-connect-e2e-encryption.md §7) ──
#
# `k2 publish subdomain <create|list|point|rm>` manages the nested
# `<label>.<sub>.k2.dev` routing entries for a Pro account against the
# hosted control plane. The bearer is the SAME tunnel token the cert broker
# uses. Since 0.45.7 the daemon makes these calls (POST /cli/connect/proxy,
# see _connect_curl_daemon): a desktop keeps the token in the K2 vault and
# `~/.k2/tunnel.json`'s `token` is empty. The CLI reads tunnel.json's token
# and calls the control plane itself only when no daemon answers. The
# control-plane base defaults to https://connect.k2.dev and is overridable
# via $K2_CONNECT_BASE (for the daemon: in ITS environment) for staging /
# integration tests.
K2_CONNECT_BASE="${K2_CONNECT_BASE:-https://connect.k2.dev}"

# Read the tunnel bearer token from ~/.k2/tunnel.json. Returns empty when
# missing so the caller can fail loud with guidance.
_connect_bearer_token() {
    python3 -c '
import json, sys
try:
    with open(sys.argv[1]) as fh:
        tok = (json.load(fh).get("token") or "").strip()
except Exception:
    tok = ""
sys.stdout.write(tok)
' "${K2_HOME}/tunnel.json" 2>/dev/null
}

# Read the connected subdomain LABEL from ~/.k2/tunnel.json (e.g. "rosson").
# This is the primary subdomain this machine's tunnel is bound to; we use it
# to pick the connected row out of GET /subdomains for the plan display.
# (The server-side gate matches on the tunnel TOKEN, not this label — this is
# UX-only row selection. Returns empty when missing.)
_connect_self_subdomain() {
    python3 -c '
import json, sys
try:
    with open(sys.argv[1]) as fh:
        sub = (json.load(fh).get("subdomain") or "").strip()
except Exception:
    sub = ""
sys.stdout.write(sub)
' "${K2_HOME}/tunnel.json" 2>/dev/null
}

# Render a control-plane response. Reads "<http_status>\n<body>" on stdin
# (status first so we can branch before parsing). On 2xx, runs the supplied
# python `pretty` snippet (which sees the parsed dict as `d`). On error,
# maps the documented control-plane error codes to clear messages and exits
# non-zero — fail loud, never swallow.
_connect_subdomain_render() {
    local pretty="$1"
    python3 -c '
import json, sys
pretty = sys.argv[1]
raw = sys.stdin.read()
nl = raw.find("\n")
if nl < 0:
    sys.stderr.write("Error: no response from the control plane (network down?)\n")
    sys.exit(1)
status, body = raw[:nl].strip(), raw[nl+1:]
try:
    code = int(status)
except ValueError:
    sys.stderr.write("Error: bad status line %r\n" % status); sys.exit(1)
try:
    d = json.loads(body) if body.strip() else {}
except Exception:
    d = {}
if 200 <= code < 300:
    exec(pretty, {"d": d}); sys.exit(0)
err = (d.get("error") or "").strip() if isinstance(d, dict) else ""
detail = (d.get("detail") or "").strip() if isinstance(d, dict) else ""
# 403 pro_required is a friendly UPSELL, not a raw error. The server may
# put the gated subdomain label in `detail`; render the PRD form
# "<sub> needs the Pro plan — manage your plan on the dashboard". This is UX
# only — enforcement is server-side; we are just dressing the 403 it sent.
# Plan/billing changes are dashboard-only; the CLI never touches the subscription.
if err == "pro_required":
    sub = detail if detail else "This subdomain"
    sys.stderr.write(
        "%s needs the Pro plan to create nested subdomains.\n"
        "  Upgrade your plan on the dashboard: https://k2.dev/dashboard\n"
        % (sub,)
    )
    sys.exit(1)
msgs = {
    "bad_label": "Invalid label (use lowercase letters, digits, and dashes).",
    "bad_target": "Invalid target endpoint (e.g. localhost:3000).",
    "label_taken": "That label is already taken.",
    "label_is_primary": "That label is your primary subdomain — it cannot be reassigned here.",
    "primary_undeletable": "Your primary subdomain cannot be removed.",
    "not_found": "No such subdomain.",
    "invalid_token": "Tunnel token rejected — re-select your subdomain in Settings -> K2 Connect to refresh it.",
}
m = msgs.get(err)
if m is None:
    if code == 401:
        m = "Unauthorized — your tunnel token is missing or expired."
    elif err:
        m = "Control plane error (%s)%s" % (err, (": " + detail) if detail else "")
    else:
        m = "Control plane returned HTTP %d%s" % (code, (": " + body[:200]) if body.strip() else "")
elif detail:
    m = m + " (" + detail + ")"
sys.stderr.write("Error: %s\n" % m); sys.exit(1)
' "$pretty"
}

# 0.45.7 — the daemon makes the control-plane call (POST /cli/connect/proxy)
# with the tunnel token it holds. Since 0.45.4 a desktop keeps that token
# in the K2 vault and tunnel.json's `token` is empty, so the CLI cannot
# read it, and must not: the token never leaves the daemon.
#
# Prints "<http_status>\n<body>" (the control plane's answer) and returns
# 0; returns $_CONNECT_NO_DAEMON when no daemon answered (none configured,
# connection refused, or one too old to have the route) so the caller may
# fall back; otherwise prints why on stderr (_publish_err: JSON under
# --json) and returns the exit code to use (3 = needs the owner).
_CONNECT_NO_DAEMON=10
_connect_curl_daemon() {
    local method="$1" endpoint="$2" body="${3:-}"
    local req
    # MSYS2_ENV_CONV_EXCL: on Windows, Git Bash rewrites an environment
    # value that looks like a POSIX path for a native program, so
    # "/subdomains" reached Python as "C:/Program Files/Git/subdomains".
    # These values are API paths, never files. Unix ignores the variable.
    req="$(MSYS2_ENV_CONV_EXCL=K2CC_ K2CC_M="$method" K2CC_E="$endpoint" K2CC_B="$body" python3 -c '
import json, os
d = {"method": os.environ["K2CC_M"], "endpoint": os.environ["K2CC_E"]}
if os.environ.get("K2CC_B"):
    d["body"] = os.environ["K2CC_B"]
print(json.dumps(d))
')" || return 1
    local _K2_CFG="" out="" rc=1 code="" resp=""
    # A K2 agent terminal / sealed cell: the per-cell socket with the
    # session passport first (a sealed cell may have no TCP route), like
    # cli_send. A daemon that doesn't serve the route there → TCP below.
    local sock; sock="$(_hook_sock)"
    if [ -n "$sock" ] && [ -S "$sock" ] && [ -n "${SCOPED_TOKEN:-}" ] \
        && [ "${TOKEN:-}" = "$SCOPED_TOKEN" ] && ! _cli_skip_uds; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        _k2_cfg_add data-raw "$req"
        rc=0
        out="$(_k2_curl "$_K2_CFG" -sS -X POST --unix-socket "$sock" \
            "http://localhost/cli/connect/proxy" -H "Content-Type: application/json" \
            --connect-timeout 5 --max-time 45 -w $'\n%{http_code}' 2>/dev/null)" || rc=$?
        if [ "$rc" -eq 0 ] && [[ "$out" == *"not served on cell socket"* ]]; then
            rc=1
        fi
    fi
    if [ "$rc" -ne 0 ]; then
        if [ -z "${PORT:-}" ] || [ -z "${TOKEN:-}" ]; then
            return "$_CONNECT_NO_DAEMON"
        fi
        _K2_CFG=""
        _k2_cfg_add url "${BASE_URL}/cli/connect/proxy?token=${TOKEN}"
        _k2_cfg_add data-raw "$req"
        rc=0
        out="$(_k2_curl "$_K2_CFG" -sS -X POST -H "Content-Type: application/json" \
            --connect-timeout 5 --max-time 45 -w $'\n%{http_code}' 2>/dev/null)" || rc=$?
        if [ "$rc" -ne 0 ]; then
            return "$_CONNECT_NO_DAEMON"
        fi
    fi
    code="${out##*$'\n'}"
    resp="${out%$'\n'*}"
    case "$code" in
        200)
            printf '%s' "$resp" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
    status, body = int(d["status"]), d["body"]
except Exception as e:
    sys.stderr.write("Error: unexpected reply from the K2 daemon for a K2 Connect request (%s)\n" % e)
    sys.exit(1)
sys.stdout.write("%d\n%s" % (status, body))
'
            return $?
            ;;
        # A daemon from before 0.45.7 has no such route.
        404|405) return "$_CONNECT_NO_DAEMON" ;;
    esac
    local pair
    pair="$(printf '%s' "$resp" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    d = {}
if not isinstance(d, dict):
    d = {}
err = d.get("error")
if isinstance(err, dict):
    err = err.get("hint") or err.get("code")
print(d.get("code") or "")
print(err or "")
' 2>/dev/null)" || pair=""
    local ecode="${pair%%$'\n'*}" emsg="${pair#*$'\n'}"
    if [ "$code" = "403" ] && [ -z "$ecode" ]; then
        _publish_err "forbidden" "K2 Connect commands need the server owner, an Owner/Admin login, or a K2 agent session's own passport; this credential was refused."
        return 3
    fi
    [ -n "$ecode" ] || ecode="http_${code}"
    [ -n "$emsg" ] || emsg="the K2 daemon refused the K2 Connect request (HTTP ${code})"
    _publish_err "$ecode" "$emsg"
    return 1
}

# Issue a control-plane request, echoing "<http_status>\n<body>" for the
# renderer. Usage: _connect_curl METHOD ENDPOINT [json-body].
# Exits (1, or 3 = needs the owner) after printing why when there is no
# answer. Callers capture it (`raw="$(_connect_curl …)" || exit $?`) so
# that exit stops them.
#
# The daemon makes the call (_connect_curl_daemon). Only when no daemon
# answers AND tunnel.json still holds a token (a headless box before the
# daemon is up) does the CLI call the control plane itself, as before
# 0.45.7.
_connect_curl() {
    _k2_refuse_if_airgap
    local method="$1" endpoint="$2" body="${3:-}"
    local drc=0
    _connect_curl_daemon "$method" "$endpoint" "$body" || drc=$?
    if [ "$drc" -eq 0 ]; then
        return 0
    elif [ "$drc" -ne "$_CONNECT_NO_DAEMON" ]; then
        exit "$drc"
    fi
    local token
    token="$(_connect_bearer_token)"
    if [ -z "$token" ]; then
        _publish_err "no_token" "could not reach the K2 daemon, which holds the K2 Connect tunnel token (${K2_HOME}/tunnel.json has none). Start K2 and try again — or, if this device is not paired, select a subdomain in Settings -> K2 Connect (or run 'k2 tunnel start') first."
        exit 1
    fi
    local args=(-sS -X "$method" "${K2_CONNECT_BASE}${endpoint}"
        --connect-timeout 5 --max-time 30
        -w $'\n%{http_code}')
    local _K2_CFG=""
    _k2_cfg_add header "Authorization: Bearer ${token}"
    if [ -n "$body" ]; then
        args+=(-H "Content-Type: application/json")
        _k2_cfg_add data-raw "$body"
    fi
    local out
    out="$(_k2_curl "$_K2_CFG" "${args[@]}" 2>/dev/null)" || {
        echo "Error: could not reach the K2 Connect control plane at ${K2_CONNECT_BASE}." >&2
        exit 1
    }
    # curl appended "\n<status>" (from -w) after the body; pull the status
    # off the tail and emit it FIRST so the renderer can branch on it.
    local code="${out##*$'\n'}"
    local resp="${out%$'\n'*}"
    printf '%s\n%s' "$code" "$resp"
}

# `k2 publish secret --help` (K2 Cores field report 2026-10-28: the flags
# lived only in `k2 vault grant --help`).
_publish_secret_usage() {
    cat <<'EOF'
k2 publish secret — vault secrets a published service gets when it starts

  k2 publish secret add <service> NAME [--scope <s>] [--as env|file] [--env-name X] [--file-name F]
  k2 publish secret remove <service> NAME [--scope <s>]
  k2 publish secret list <service>

  <service>        the name you gave k2 publish run
  NAME             the secret's name in the vault (k2 vault list)
  --scope <s>      where NAME lives: ws:<workspace> or server.
                   Default: your own workspace if you're an agent; the server
                   scope if the owner runs it from their own terminal.
                   If NAME isn't there, the error names the scope that has it.
  --as env|file    env: an environment variable; file: a 0600 file in
                   $K2_SECRETS_DIR. Default: how the secret was saved to be
                   delivered. You can't pick a delivery the secret doesn't
                   allow (k2 vault show NAME shows both).
  --env-name X     the variable's name (default: NAME in capitals, - and . as _).
                   --env-name alone means --as env.
  --file-name F    the file's name in $K2_SECRETS_DIR (default: NAME).
                   --file-name alone means --as file.

The service gets it the next time it starts: a file in its own
$K2_SECRETS_DIR, or the env variable. Running: k2 publish stop <service>, then
k2 publish start <service>. Exited: k2 publish start <service>.
An App: k2 app restart <app>.
Removing works the same way: it needs no card, and the running process keeps
what it got.

An agent can add the secrets its own workspace created. A secret a person
gave (a Thread secret card, Settings → Vault) only the owner or an Admin
can share with a service. With permission cards on, the refusal ends with
the line to ask them with, filled in: run it, adding only your reason.

  k2 permission request vault.share-service as=file|env secret=NAME service=<service> --why "<what you need it for>"

They press Allow from their Thread, phone or desktop and confirm it's them.
That does the share: don't run the add again. You get a [permission:<id>]
turn saying so; then restart the service. It works only for a secret in your
own workspace and a service or App of that workspace.
Cards off (no Ask-with line): the refusal prints the exact command for them
to run in their own terminal (Terminal or iTerm, not a K2 terminal; it asks
for Touch ID or the vault password). Don't retry it yourself.

Example: a status page that won't start without MPC_PROGRESS_PIN
  k2 publish secret add mpc-progress MPC_PROGRESS_PIN --as env
  k2 publish start mpc-progress
  (A person gave the secret? The owner runs the first line in their own
  terminal with --scope ws:k2-cores added.)
More: k2 vault grant --help, k2 study vault
EOF
}

_publish_usage() {
    cat <<'EOF' >&2
k2 publish — host a process on this machine, optionally on a nested URL

TWO DIFFERENT LISTS
  k2 publish ps                 processes this workspace is hosting
  k2 publish status             your Connect host + plan (not processes)
  k2 publish subdomain list     nested hostnames (DNS), whether or not you host the process

k2 tunnel is the device pipe to your primary *.k2.dev. Nested names ride that
same pipe. This command does not start k2 tunnel.

USAGE
  k2 publish run <name> --cmd <shell> --port <n> [--no-tunnel] [--cwd <path>] [--transfer] [--probe-timeout <dur>]
  k2 publish run <name> --cmd <dev command> --port <n> --dev [--cwd <path>] [--idle 60m] [--mem 4G] [--cpus 2]
  k2 publish run <name> --skin [<dir>] --port <n> ...   (only a site already on --skin; no new ones)
  k2 publish start|stop|logs <name>
  k2 publish rm <name> [--keep-hostname]
  k2 publish ps|status [--long] [--json]       (ps = list)
  k2 publish visibility <name> [private|pin|public] [--reason <why>] [--pin-days N] [--yes] [--dev-is-public] [--json]
  k2 publish open <name> [--path <p>] [--no-browser] [--json]
  k2 publish sessions <name> [--revoke-all] [--json]
  k2 publish env <name> [clean|legacy] · k2 publish secret add|remove|list <name> …
  k2 publish frame <name> (--allow <origin> ... | --none | --reset) [--json]
  k2 publish client-ip [<label> on|off] [--json]
  k2 publish subdomain <create|list|point|target|rm|claim|unclaim> ...

run
  Starts <name> from --cmd, listening on --port.
  K2 0.46.1+, a Linux server with the App runtime set up: a NEW <name> runs as
  an open-path App (a relaxed container with a fresh copy of the folder at each
  start; keep lasting files in $K2_APP_DATA; --port is its port inside; egress
  logged). Run it again to deploy changes; start|stop|rm map onto the App
  (k2 app logs|inspect <name>). An agent's new service waits for the owner's
  in-person approval (exit 2); re-running one it already has needs "Build and
  run Apps".
  Services published before keep running as they are.
  Default (Pro): also creates nested https://<name>.<host>.k2.dev → localhost:<port>.
  <host> is the Connected host from `k2 publish status`.
  Not Pro / no tunnel: run FAILS, nothing starts. Use --no-tunnel or upgrade:
    https://k2.dev/dashboard
  --no-tunnel: process only, no public URL, no Pro required.
  Same <name> already running: error — stop first. Does not overwrite.
  A name another workspace on this server owns is refused
  (label_owned_by_other_workspace). --transfer: owner/admin only, moves it
  here on purpose; recorded in the access audit log.
  --cmd and --skin are mutually exclusive. --cwd is for --cmd only.

  --probe-timeout <dur>  how long the service may take to open --port
                   (default 15s, at most 15m): 90s, 10m, 1m30s, 1500ms.
                   Saved on the service: start, restarts and boot use it.
                   A re-run without it keeps the saved value; `default` resets.
                   While it waits the status is "starting (waiting for port
                   N, 4m12s left)"; with a tunnel the nested URL is published
                   only once the port answers. Never answered in time: the
                   run is rolled back (tunnel) or shows unhealthy (local,
                   still re-checked, so a late bind turns running).

  --dev            Run a dev server from the live folder, with hot reload through
                   the site's address. Private until you share it. It stops after
                   --idle (default 60m, off is for the owner and Admins) and starts
                   again for the next visitor who passes K2's check. Not walled (a
                   normal new --cmd service is), so an agent's first one waits for
                   the owner. How: k2 study publish-dev

  --skin [<dir>]   Keeps a site already on --skin running. A new <name> is refused
                   (skin_retired). A folder of files is an App now:
                   k2 app init --template static (k2 study app-deploys).

visibility
  Who may open a site. A new site is private. Sites from before 0.46.7 stay
  public until someone changes them. No mode: show it. k2 study publish-visibility
  private          only the owner and Admins, after K2 checks who they are
  pin              the PIN first. A person gives it on stdin, or types it when
                   asked (--pin is refused: it would land in shell history)
  public           anyone with the address; needs --reason, and an owner or Admin
  An agent can make its own site private. For pin or public, the same command
  puts a card on the agent's Thread and exits 2 (waiting for a person); the person
  answers it, and types any PIN into K2's own form, so the agent never sees it.
  A dev server can't be asked for public (dev_public_person_only): ask for a PIN.
  (Exit 2 = waiting for a person, or a usage error: the message says which.
  Exit 3 = refused.)

open       a one-time link to a private site, for a browser outside K2. K2's own
           tabs need no link. Not for agents: test on the local port instead.
sessions   who is signed in to a site, private or PIN; --revoke-all ends them all.

start / stop
  start <name>: uses the saved --cmd/--port/--cwd. Do not pass them again.
  stop <name>:  kill the process. Nested URL stays (it will miss until start).

rm
  stop + forget the service. Also removes the nested URL unless --keep-hostname
  (--keep-hostname: process gone, https://<name>.<host>.k2.dev still exists).

  <name> on run is the subdomain label. It cannot differ.

ps     hosted processes for THIS workspace (running/stopped, port, who may open
       it, URL if any); --long adds why a site is public, and a dev server's caps
       [legacy env] = a service from before 0.45.4 that still inherits the
       daemon's whole environment (it keeps it; no end is set). Start it
       clean: k2 publish env <name> clean
logs   that process's log file (--tail follows)
status Connect plan + primary host. Not "is my app up" — use ps.

subdomain  (Pro) hostname for a port that is ALREADY listening.
  Does not start/stop a process. ps/stop/logs will not see it.
  create <label> --target localhost:<port>   new name → that port
  point  <label> --target localhost:<port>   existing name → a different port
  rm     <label>                             delete the name (not the process)
  claim / unclaim <label>                    show or hide that name on THIS
                                             workspace's Published list
  A name another workspace owns is refused before anything changes.
  create/point/claim --transfer: owner/admin only, audited.

  Labels `skin`, `meet`, `chat` and `mail` are reserved (box front doors: Apps, K2 Meet, K2 chat, K2 webmail).
  Off-platform custom domains are not created here.
  k2 app is guests/roles/tokens, and (Apps beta) hosts server Apps with
  deploys and rollback: k2 study app-deploys.
  Leftover: k2 skin.

logs <name> [--tail]   dump the log file; --tail follows

frame  (owner only) who may put a --skin App in a frame on another site.
  Default: the App itself and K2's own windows. Every other site is refused.
  --allow https://your.site   add an origin (repeatable, at most 8)
  --none                      refuse every frame, K2's windows too
  --reset                     back to the default
  ps shows the policy. A --cmd App sets its own header: k2 study publish-framing

If Connect/tunnel is down, default run FAILS (nothing starts). --no-tunnel still works.
EOF
}

# GH#33 / #35 — publish errors as JSON when AS_JSON=true.
_publish_err() {
    local code="$1" hint="$2"
    if [ "${AS_JSON:-false}" = "true" ] || [ "${as_json:-false}" = "true" ]; then
        python3 -c 'import json,sys; print(json.dumps({"error":{"code":sys.argv[1],"hint":sys.argv[2]}}))' \
            "$code" "$hint" 1>&2 2>/dev/null || echo "{\"error\":{\"code\":\"$code\",\"hint\":\"$hint\"}}" >&2
    else
        echo "Error: $hint" >&2
    fi
}

# Render a /cli/publish/* daemon body. Env: RESP, AS_JSON, P_MODE (service|list|logs), P_VERB.
# Errors stderr (human, or _publish_err JSON). Not-Pro copy from the daemon is
# printed as-is — do not invent a second dashboard / --no-tunnel story.
_publish_handle_resp() {
    RESP="${RESP:-}" AS_JSON="${AS_JSON:-false}" P_MODE="${P_MODE:-service}" \
    P_VERB="${P_VERB:-}" python3 - <<'PY'
import json, os, sys

raw = os.environ.get("RESP") or ""
as_json = os.environ.get("AS_JSON", "false") == "true"
mode = os.environ.get("P_MODE") or "service"
verb = os.environ.get("P_VERB") or ""

def fail(code, hint, exit_code=1):
    if verb == "frame":
        # Exit 2 = bad input; exit 3 = stop, this needs the owner.
        if code in ("usage", "bad_frame_origin", "frame_too_many", "bad_frame_policy"):
            exit_code = 2
        elif code in ("owner_only", "role_required"):
            exit_code = 3
    # The disk guard (0.46.3): a refusal is exit 3, like every k2 app verb.
    if code == "disk_low":
        exit_code = 3
    # 0.46.7: K2's refusals of a dev server, and of a new --skin site.
    if code in ("skin_retired", "idle_off_person_only"):
        exit_code = 3
    if code in ("dev_needs_cmd", "dev_needs_tunnel", "usage"):
        exit_code = 2
    hint = hint if isinstance(hint, str) else str(hint)
    text = hint.rstrip("\n")
    if as_json:
        sys.stderr.write(json.dumps(
            {"error": {"code": str(code), "hint": text}},
            separators=(",", ":"),
        ) + "\n")
    else:
        # Daemon Not-Pro bodies already name dashboard + --no-tunnel.
        if text.startswith("Error:"):
            sys.stderr.write(text + "\n")
        else:
            sys.stderr.write("Error: %s\n" % text)
    sys.exit(exit_code)

if not raw.strip():
    fail("unreachable", "no response from the daemon")

d = None
try:
    d = json.loads(raw)
except Exception:
    d = None

if mode == "logs" and not isinstance(d, dict):
    sys.stdout.write(raw)
    if not raw.endswith("\n"):
        sys.stdout.write("\n")
    sys.exit(0)

if d is None:
    # Non-JSON error body (e.g. Not Pro copy): print it; do not wrap a second story.
    fail("error", raw.strip()[:2000] if raw.strip() else "daemon returned an unparseable response")

if not isinstance(d, dict):
    if as_json:
        print(json.dumps(d, separators=(",", ":")))
        sys.exit(0)
    fail("bad_response", "unexpected response from /cli/publish")

err = d.get("error")
if err is not None and err != "" and err is not False:
    if isinstance(err, dict):
        code = err.get("code") or "error"
        hint = err.get("hint") or err.get("message") or err.get("detail") or code
    elif d.get("code"):
        # Structured refusal (e.g. label_owned_by_other_workspace): keep
        # both the message and the hint.
        code = str(d.get("code"))
        hint = str(err)
        if d.get("hint"):
            hint = "%s\n  %s" % (hint, d.get("hint"))
    else:
        code = str(err) if str(err).replace("_", "").isalnum() and len(str(err)) < 48 else "error"
        hint = str(err)
        if d.get("hint"):
            hint = str(d.get("hint"))
        elif d.get("detail"):
            hint = str(d.get("detail"))
        elif d.get("message") and hint in ("error", "pro_required"):
            hint = str(d.get("message"))
    fail(code, hint)

if d.get("success") is False:
    fail("error", d.get("hint") or d.get("message") or "request failed")

if mode == "logs":
    text = d.get("log")
    if text is None:
        text = d.get("text")
    if text is None:
        text = d.get("body")
    if text is None and isinstance(d.get("lines"), list):
        text = "\n".join(str(x) for x in d["lines"])
    if text is None:
        text = raw
    sys.stdout.write(str(text))
    if not str(text).endswith("\n"):
        sys.stdout.write("\n")
    sys.exit(0)

def env_legacy(s):
    # 0.45.4: envMode from the daemon; older daemons only sent the notice.
    if not isinstance(s, dict):
        return False
    m = s.get("envMode")
    if m is not None:
        return m == "legacy"
    return bool(s.get("envDeprecation"))

# The release the inherited environment ends in, from the daemon. Null: no
# end is set (Rosson 2026-10-26: --cmd services keep running), so no text
# names one. An older daemon still sends one.
def env_ends(s):
    v = s.get("envRemovedIn") if isinstance(s, dict) else None
    return str(v) if v else None

# 0.46.4 field report #4: a daemon that IS that release says so (cutoverNow).
def env_now(s):
    return bool(isinstance(s, dict) and s.get("cutoverNow"))

def env_when(s):
    e = env_ends(s)
    if not e:
        return None
    return ("with this release (K2 %s)" if env_now(s) else "in K2 %s") % e

def env_warning(n, s):
    w = env_when(s)
    if not w:
        return ("warning: %s still inherits the daemon environment (it keeps it as a --cmd service). "
                "Start it clean with: k2 publish env %s clean (grant secrets with k2 publish secret add)."
                % (n, n))
    return ("warning: %s still inherits the daemon environment; this ends %s. "
            "Switch with: k2 publish env %s clean (grant secrets with k2 publish secret add)."
            % (n, w, n))

# 0.46.3: held for the owner (a new --cmd service from an agent, or a disk
# override that waits for the owner): print the request and exit 2.
if mode == "service" and verb == "run" and d.get("status") == "held":
    if as_json:
        print(json.dumps(d, separators=(",", ":")))
    else:
        r = d.get("request") if isinstance(d.get("request"), dict) else {}
        print("held for the owner: %s  %s" % (r.get("id") or d.get("requestId") or "?", r.get("summary") or ""))
        for why in d.get("reasons") or []:
            print("  why: %s" % why)
        # 0.46.6 (client papercut 12): where the owner sees it, and the
        # permission card K2 raised for it (as k2 app holds say).
        if d.get("reused"):
            print("  this is your earlier request %s (asked %s ago), still pending; nothing new was filed" % (r.get("id") or "?", d.get("askedAgo") or "a while"))
        c = d.get("card") if isinstance(d.get("card"), dict) else {}
        if c.get("where"):
            print("  owner: %s" % c["where"])
        if d.get("hint"):
            print(d["hint"])
    sys.exit(2)
if mode == "service" and verb == "run":
    for _k, _label in (("diskWarning", "disk"), ("diskOverride", "disk override")):
        _x = d.get(_k)
        if isinstance(_x, dict) and _x.get("hint"):
            sys.stderr.write("%s warning: %s\n" % (_label, _x["hint"]))
        elif isinstance(_x, dict) and _x.get("applied"):
            sys.stderr.write("disk override (%s): %s\n" % (_x.get("via", ""), _x.get("reason", "")))
# run/start of a legacy service: one stderr line, so --json stdout stays clean.
if mode == "service" and verb in ("run", "start"):
    _s = d.get("service") if isinstance(d.get("service"), dict) else d
    # DM-4: a dev server says what it shows, how it runs, and its box's limits.
    if isinstance(_s, dict) and isinstance(_s.get("dev"), dict):
        _dv = _s["dev"]
        if d.get("devWarning"):
            sys.stderr.write("warning: %s\n" % d["devWarning"])
        sys.stderr.write("dev: live folder, not walled · caps: %s\n" % (_dv.get("caps") or "?"))
        if _dv.get("warning"):
            sys.stderr.write("warning: %s\n" % _dv["warning"])
    if env_legacy(_s):
        sys.stderr.write(env_warning(str(_s.get("name") or "?"), _s) + "\n")
    _c = d.get("crossover") if isinstance(d, dict) else None
    if isinstance(_c, dict) and _c.get("wouldHold"):
        # 0.46.0 is warn-only: the run went through; say so first.
        sys.stderr.write("warning: %s\n" % (_c.get("message") or ("it ran (this release only warns); " + str(_c.get("hint", "")))))
    # 0.46.1 (K5): where the run went.
    _w = d.get("wall") if isinstance(d, dict) else None
    if isinstance(_w, dict) and _w.get("path") == "open":
        sys.stderr.write("%s: %s, deployment %s (%s)\n" % (_w.get("app") or "?", _w.get("label") or "open path",
                         _w.get("deployment") or "?", "new App" if _w.get("created") else "redeployed"))
    elif isinstance(_w, dict) and _w.get("path") == "legacy":
        sys.stderr.write("note: it runs unwalled: %s\n" % (_w.get("why") or "the open path isn't ready on this box"))

if as_json:
    print(json.dumps(d, separators=(",", ":")))
    sys.exit(0)

def url_or_target(s):
    if not isinstance(s, dict):
        return "-"
    u = (s.get("url") or "").strip()
    if u:
        return u
    t = (s.get("target") or "").strip()
    if t:
        return t
    port = s.get("port")
    if port is not None and str(port) != "":
        return "localhost:%s" % port
    return "-"

def fmt_left(ms):
    try:
        secs = max(0, int(ms) // 1000)
    except Exception:
        return "?"
    m, s = divmod(secs, 60)
    if m and s:
        return "%dm%02ds" % (m, s)
    if m:
        return "%dm" % m
    return "%ds" % s

def status_text(s):
    # "starting (waiting for port 3047, 4m12s left)" while inside the grace.
    if not isinstance(s, dict):
        return "-"
    st = str(s.get("status") or s.get("desired") or "-")
    if st == "starting" and s.get("startingLeftMs") is not None:
        return "starting (waiting for port %s, %s left)" % (s.get("port"), fmt_left(s.get("startingLeftMs")))
    return st

def vis_label(s):
    # Who may open it. An App's own logins decide for an App; a --no-tunnel
    # service has no address to open.
    if not isinstance(s, dict):
        return "-"
    v = s.get("visibility")
    if not isinstance(v, dict):
        return "own logins" if s.get("cmd") == "(app)" else "-"
    if v.get("enforced") is False:
        return str(v.get("mode") or "?") + " (not enforced: E2E off)"
    return str(v.get("mode") or "?") + (", dev" if v.get("dev") else "")

def frame_label(s):
    # --skin: the policy K2 sends. --cmd: the App's own job.
    if not isinstance(s, dict):
        return "-"
    pol = s.get("framePolicy")
    if pol is None:
        return "own server"
    raw = str(s.get("frameAncestors") or "")
    if raw == "":
        return "default"
    if raw == "none":
        return "none"
    return "default + " + raw.replace(" ", ",")

if mode == "list":
    services = d.get("services")
    if not isinstance(services, list):
        services = []
    rows = []
    warnings = []
    details = []
    long_view = os.environ.get("P_LONG") == "1"
    reasons = []
    legacy = []
    unwalled = []
    hint = "k2 study app-move"
    ends = None
    for s in services:
        if not isinstance(s, dict):
            continue
        port = s.get("port")
        port_s = "-" if port is None or str(port) == "" else str(port)
        u = url_or_target(s)
        if env_legacy(s):
            legacy.append(str(s.get("name") or "?"))
            ends = env_when(s)
            if env_ends(s):
                u += "  [legacy env (ends %s)]" % (("now, " + env_ends(s)) if env_now(s) else env_ends(s))
            else:
                u += "  [legacy env]"
        # 0.46.3: --cmd services no wall runs, and where a moved label went.
        if s.get("legacyLabel"):
            unwalled.append(str(s.get("name") or "?"))
            hint = s.get("legacyHint") or "k2 study app-move"
            u += "  [legacy (--cmd)%s]" % ((": label moved to App " + s["labelMovedTo"]) if s.get("labelMovedTo") else "")
        rows.append((
            str(s.get("name") or "?"),
            status_text(s),
            port_s,
            vis_label(s),
            frame_label(s),
            u,
        ))
        if s.get("bindWarning"):
            warnings.append((str(s.get("name") or "?"), str(s.get("bindWarning"))))
        if long_view:
            _v = s.get("visibility") if isinstance(s.get("visibility"), dict) else {}
            if _v.get("mode") == "public" and _v.get("reason"):
                details.append((str(s.get("name") or "?"), "public because: %s" % _v["reason"]))
            if isinstance(s.get("dev"), dict):
                _dv = s["dev"]
                _idle = _dv.get("idleSecs")
                details.append((str(s.get("name") or "?"), "dev: %s · caps: %s · idle stop: %s" % (
                    _dv.get("lane") or "?", _dv.get("caps") or "?",
                    "off" if _idle is None else "%dm" % (int(_idle) // 60))))
                if _dv.get("warning"):
                    warnings.append((str(s.get("name") or "?"), str(_dv["warning"])))
        if s.get("frameWarning"):
            warnings.append((str(s.get("name") or "?"), str(s.get("frameWarning"))))
        if s.get("reason"):
            reasons.append((str(s.get("name") or "?"), str(s.get("status") or "down"), str(s.get("reason"))))
        if s.get("reservedLabel"):
            warnings.append((str(s.get("name") or "?"),
                "this name is now reserved for a K2 box door; it keeps working, but pick another name for anything new"))
    if not rows:
        print("No published services.")
        sys.exit(0)
    nw = max([len(r[0]) for r in rows] + [len("NAME")])
    sw = max([len(r[1]) for r in rows] + [len("STATUS")])
    pw = max([len(r[2]) for r in rows] + [len("PORT")])
    vw = max([len(r[3]) for r in rows] + [len("VISIBILITY")])
    fw = max([len(r[4]) for r in rows] + [len("FRAME")])
    print("%-*s  %-*s  %-*s  %-*s  %-*s  %s" % (nw, "NAME", sw, "STATUS", pw, "PORT", vw, "VISIBILITY", fw, "FRAME", "URL/TARGET"))
    for n, st, p, v, f, u in rows:
        print("%-*s  %-*s  %-*s  %-*s  %-*s  %s" % (nw, n, sw, st, pw, p, vw, v, fw, f, u))
    for n, w in details:
        print("%s: %s" % (n, w))
    for n, w in warnings:
        print("warning: %s: %s" % (n, w))
    # Why a service isn't serving (its exit code and last log line).
    for n, st, why in reasons:
        print("%s is not serving (%s): %s. More: k2 publish logs %s" % (n, st, why, n))
    if legacy:
        print("warning: %d service%s still inherit%s the daemon environment (%s)%s. "
              "Switch each with: k2 publish env <service> clean (grant secrets with k2 publish secret add <service> NAME)."
              % (len(legacy), "" if len(legacy) == 1 else "s", "s" if len(legacy) == 1 else "",
                 ", ".join(legacy), ("; this ends %s" % ends) if ends else ""))
    if unwalled:
        # No cut-over is set (Rosson 2026-10-26): they keep running.
        print("legacy (--cmd): %s run%s unwalled and keep%s running; move %s to an App when ready (%s)."
              % (", ".join(unwalled), "s" if len(unwalled) == 1 else "", "s" if len(unwalled) == 1 else "",
                 "it" if len(unwalled) == 1 else "them", hint))
    sys.exit(0)

svc = d.get("service") if isinstance(d.get("service"), dict) else d
name = str(svc.get("name") or "?") if isinstance(svc, dict) else "?"
status = status_text(svc) if isinstance(svc, dict) else ""
port = svc.get("port") if isinstance(svc, dict) else None
url = url_or_target(svc) if isinstance(svc, dict) else "-"
if verb == "frame" and isinstance(svc, dict):
    print("Frame policy %s · %s" % (name, frame_label(svc)))
    print("  Content-Security-Policy: frame-ancestors %s" % (svc.get("framePolicy") or "?"))
    if d.get("restarted"):
        print("  App restarted with the new policy.")
    else:
        print("  App is not running; the policy applies at the next start.")
    sys.exit(0)
past = {"run": "Started", "start": "Started", "stop": "Stopped", "rm": "Removed"}.get(verb, "ok")
line = "%s %s" % (past, name)
if status:
    line += " · %s" % status
if port is not None and str(port) != "":
    line += " · port %s" % port
print(line)
if url and url != "-":
    print("  %s" % url)
if isinstance(svc, dict) and svc.get("hostnamePending"):
    print("  public URL is published once port %s answers (k2 publish status %s)" % (port, name))
# 0.46.7: a new site is private, so a bare visit gets K2's private page.
if verb == "run" and isinstance(svc, dict) and isinstance(svc.get("visibility"), dict):
    _vm = svc["visibility"].get("mode")
    if svc["visibility"].get("enforced") is False:
        print("  %s, but K2's check can't see this site's traffic with E2E off: anyone with the address can open it" % _vm)
    elif _vm == "private":
        print("  private: open it in a K2 tab, or k2 publish open %s. Agents: test on http://127.0.0.1:%s (k2 study publish-visibility)" % (name, port))
    elif _vm == "pin":
        print("  behind a PIN (k2 publish visibility %s)" % name)
if isinstance(d, dict) and d.get("visibilityNote"):
    print("  %s" % d["visibilityNote"])
if isinstance(svc, dict) and svc.get("error"):
    print("  error: %s" % svc.get("error"))
if isinstance(svc, dict) and env_legacy(svc) and verb not in ("run", "start"):
    # run/start already warned on stderr; stop/rm still name the end.
    if env_when(svc):
        print("  deprecated: inherits the daemon environment; ends %s (k2 publish env %s clean)" % (env_when(svc), name))
    else:
        print("  inherits the daemon environment (start it clean: k2 publish env %s clean)" % name)
sys.exit(0)
PY
}

# `--probe-timeout` check, same rules as the daemon: 90s, 10m, 1m30s,
# 1500ms, bare seconds, or `default`; > 0 and at most 15m. Prints the
# error and returns 1 when it is not.
_publish_probe_timeout_check() {
    P_PT="$1" python3 -c '
import os, re, sys
raw = os.environ.get("P_PT", "").strip().lower()
bad = "--probe-timeout must be a duration like 90s, 10m or 1500ms (at most 15m), got \x27%s\x27" % os.environ.get("P_PT", "").strip()
if raw == "default":
    sys.exit(0)
if re.fullmatch(r"[0-9]+", raw):
    ms = int(raw) * 1000
elif re.fullmatch(r"([0-9]+(ms|s|m|h))+", raw):
    unit = {"ms": 1, "s": 1000, "m": 60000, "h": 3600000}
    ms = sum(int(n) * unit[u] for n, u in re.findall(r"([0-9]+)(ms|s|m|h)", raw))
else:
    print(bad); sys.exit(1)
if ms <= 0:
    print(bad); sys.exit(1)
if ms > 15 * 60 * 1000:
    print("--probe-timeout is at most 15m (got %s); a service that needs longer should open its port first and build behind it" % os.environ.get("P_PT", "").strip()); sys.exit(1)
'
}

_publish_is_port() {
    case "${1:-}" in
        ''|*[!0-9]*) return 1 ;;
        0) return 1 ;;
    esac
    [ "$1" -le 65535 ]
}

cmd_publish() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        run)
            cmd_publish_run "$@"
            ;;
        start)
            cmd_publish_start "$@"
            ;;
        stop)
            cmd_publish_stop "$@"
            ;;
        rm)
            cmd_publish_rm "$@"
            ;;
        ps|list|ls)
            cmd_publish_ps "$@"
            ;;
        logs)
            cmd_publish_logs "$@"
            ;;
        status)
            cmd_publish_status "$@"
            ;;
        subdomain)
            cmd_publish_subdomain "$@"
            ;;
        frame)
            cmd_publish_frame "$@"
            ;;
        client-ip)
            cmd_publish_client_ip "$@"
            ;;
        visibility)
            cmd_publish_visibility "$@"
            ;;
        open)
            cmd_publish_open "$@"
            ;;
        sessions)
            cmd_publish_sessions "$@"
            ;;
        # K2 vault (§10.5): secrets a published service gets at start.
        secret|secrets)
            local act="${1:-}" label="${2:-}" sname="${3:-}"
            if [ -z "$act" ]; then
                _publish_secret_usage >&2
                exit 2
            fi
            if [ "$act" = "help" ] || _wants_help "$@"; then
                _publish_secret_usage
                exit 0
            fi
            case "$act" in
                add)
                    [ -n "$label" ] && [ -n "$sname" ] || { echo "usage: k2 publish secret add <service> NAME [--as file|env] [--env-name X] [--scope …]" >&2; exit 2; }
                    shift 3
                    cmd_vault grant "$sname" --to "service:$label" "$@" ;;
                remove|rm)
                    [ -n "$label" ] && [ -n "$sname" ] || { echo "usage: k2 publish secret remove <service> NAME [--scope …]" >&2; exit 2; }
                    shift 3
                    cmd_vault revoke "$sname" --from "service:$label" "$@" ;;
                list|ls)
                    [ -n "$label" ] || { echo "usage: k2 publish secret list <service>" >&2; exit 2; }
                    cmd_vault service "$label" ;;
                *) echo "usage: k2 publish secret add|remove|list <service> …" >&2; exit 2 ;;
            esac
            ;;
        env)
            [ -n "${1:-}" ] || { echo "usage: k2 publish env <service> [clean|legacy] [--env-file .env.k2]" >&2; exit 2; }
            cmd_vault service "$@"
            ;;
        ""|help|--help|-h)
            _publish_usage
            exit 0
            ;;
        *)
            echo "Unknown 'k2 publish' subcommand: $sub" >&2
            _publish_usage
            exit 1
            ;;
    esac
}

# `k2 publish run <name> --cmd <shell> --port <n> [--no-tunnel] [--cwd <path>]`
# `k2 publish run <name> --skin [<dir>] --port <n> [--no-tunnel]`
cmd_publish_run() {
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local name="" cmd="" port="" cwd="" no_tunnel=0 as_json=false skin=0 skin_root="" transfer=0 probe_timeout="" dov=""
    local dev=0 dev_idle="" dev_mem="" dev_cpus=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --dev) dev=1; shift ;;
            --idle)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--idle needs a duration from 5m to 24h, or off"; exit 2; }
                dev_idle="$2"; shift 2 ;;
            --idle=*) dev_idle="${1#--idle=}"; shift ;;
            --mem)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--mem needs an amount, like 4G"; exit 2; }
                dev_mem="$2"; shift 2 ;;
            --mem=*) dev_mem="${1#--mem=}"; shift ;;
            --cpus)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--cpus needs a number of CPUs, like 2"; exit 2; }
                dev_cpus="$2"; shift 2 ;;
            --cpus=*) dev_cpus="${1#--cpus=}"; shift ;;
            --disk-override)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--disk-override needs a reason"; exit 2; }
                dov="$2"; shift 2 ;;
            --disk-override=*)
                dov="${1#--disk-override=}"; shift ;;
            --probe-timeout)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--probe-timeout needs a duration (90s, 10m, at most 15m) or 'default'"; exit 2; }
                probe_timeout="$2"; shift 2 ;;
            --probe-timeout=*)
                probe_timeout="${1#--probe-timeout=}"; shift ;;
            --cmd)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--cmd needs a shell string"; exit 2; }
                cmd="$2"; shift 2 ;;
            --cmd=*)
                cmd="${1#--cmd=}"; shift ;;
            --port)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--port needs a number"; exit 2; }
                port="$2"; shift 2 ;;
            --port=*)
                port="${1#--port=}"; shift ;;
            --cwd)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--cwd needs a path"; exit 2; }
                cwd="$2"; shift 2 ;;
            --cwd=*)
                cwd="${1#--cwd=}"; shift ;;
            --skin)
                skin=1; shift
                # Optional operand iff the next argv does not start with `-`.
                # Do not stat to decide (flags vs dir).
                if [ $# -gt 0 ]; then
                    case "$1" in
                        -*) ;;
                        *) skin_root="$1"; shift ;;
                    esac
                fi
                ;;
            --skin=*)
                skin=1
                skin_root="${1#--skin=}"
                shift ;;
            --no-tunnel) no_tunnel=1; shift ;;
            --transfer) transfer=1; shift ;;
            --json) as_json=true; shift ;;
            --help|-h) _publish_usage; exit 0 ;;
            -*)
                as_json="$as_json" _publish_err "usage" \
                    "unknown flag '$1' — usage: k2 publish run <name> (--cmd <shell>|--skin [<dir>]) --port <n> [--no-tunnel] [--cwd <path>] [--transfer] [--probe-timeout <dur>] [--dev [--idle <dur>] [--mem <n>] [--cpus <n>]]"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then
                    name="$1"; shift
                else
                    as_json="$as_json" _publish_err "usage" "unexpected argument '$1'"
                    exit 2
                fi ;;
        esac
    done
    if [ "$skin" = 1 ] && [ -n "$cmd" ]; then
        as_json="$as_json" _publish_err "usage" "--cmd and --skin are mutually exclusive"
        exit 2
    fi
    if [ "$dev" = 1 ] && [ "$skin" = 1 ]; then
        as_json="$as_json" _publish_err "dev_needs_cmd" "--dev runs a dev server, so it needs --cmd (--skin hosts static files)"
        exit 2
    fi
    if [ "$dev" = 1 ] && [ "$no_tunnel" = 1 ]; then
        as_json="$as_json" _publish_err "dev_needs_tunnel" "--dev puts a dev server behind K2's check at its address, so it can't use --no-tunnel. To run one for yourself only, run the command in a terminal"
        exit 2
    fi
    if [ "$dev" != 1 ] && { [ -n "$dev_idle" ] || [ -n "$dev_mem" ] || [ -n "$dev_cpus" ]; }; then
        as_json="$as_json" _publish_err "usage" "--idle, --mem and --cpus go with --dev"
        exit 2
    fi
    if [ "$skin" = 1 ] && [ -n "$cwd" ]; then
        as_json="$as_json" _publish_err "usage" "--cwd and --skin are mutually exclusive"
        exit 2
    fi
    if [ -z "$name" ] || [ -z "$port" ]; then
        as_json="$as_json" _publish_err "usage" \
            "usage: k2 publish run <name> (--cmd <shell>|--skin [<dir>]) --port <n> [--no-tunnel] [--cwd <path>]"
        exit 2
    fi
    if [ "$skin" != 1 ] && [ -z "$cmd" ]; then
        as_json="$as_json" _publish_err "usage" \
            "usage: k2 publish run <name> (--cmd <shell>|--skin [<dir>]) --port <n> [--no-tunnel] [--cwd <path>]"
        exit 2
    fi
    if ! _publish_is_port "$port"; then
        as_json="$as_json" _publish_err "usage" "--port must be an integer 1-65535"
        exit 2
    fi
    if [ -n "$probe_timeout" ]; then
        local pt_err
        if ! pt_err=$(_publish_probe_timeout_check "$probe_timeout"); then
            as_json="$as_json" _publish_err "usage" "$pt_err"
            exit 2
        fi
    fi
    if [ "$skin" = 1 ] && [ -n "$skin_root" ]; then
        case "$skin_root" in
            /*|~*|[A-Za-z]:*)
                as_json="$as_json" _publish_err "usage" "skinRoot must be a workspace-relative directory"
                exit 2 ;;
        esac
        case "$skin_root" in
            *..*)
                as_json="$as_json" _publish_err "usage" "skinRoot must not contain .."
                exit 2 ;;
        esac
        if [ ! -d "$PROJECT/$skin_root" ]; then
            as_json="$as_json" _publish_err "usage" "skinRoot is not a directory: $skin_root"
            exit 2
        fi
    fi
    [ -z "$cwd" ] && [ "$skin" != 1 ] && cwd="$PROJECT"
    # A relative --cwd is the caller's: resolve it against where k2 runs
    # (the daemon would read it against its own folder). K2 Cores field
    # report 2026-10-28.
    case "$cwd" in
        ''|/*|[A-Za-z]:[\\/]*) ;;
        '~'|'~/'*) cwd="${HOME}${cwd#\~}" ;;
        *) cwd="$(P_REL="$cwd" python3 -c 'import os; print(os.path.abspath(os.environ["P_REL"]))')" ;;
    esac
    local body RESP
    body=$(P_NAME="$name" P_CMD="$cmd" P_PORT="$port" P_CWD="$cwd" \
           P_PROJECT="$PROJECT" P_NO_TUNNEL="$no_tunnel" P_TRANSFER="$transfer" \
           P_SKIN="$skin" P_SKIN_ROOT="$skin_root" P_PROBE_TIMEOUT="$probe_timeout" P_DISK_OVERRIDE="$dov" \
           P_DEV="$dev" P_DEV_IDLE="$dev_idle" P_DEV_MEM="$dev_mem" P_DEV_CPUS="$dev_cpus" python3 -c '
import json, os
b = {
    "name": os.environ["P_NAME"],
    "port": int(os.environ["P_PORT"]),
    "project": os.environ["P_PROJECT"],
}
if os.environ.get("P_DEV") == "1":
    b["dev"] = True
    for k, e in (("idle", "P_DEV_IDLE"), ("mem", "P_DEV_MEM"), ("cpus", "P_DEV_CPUS")):
        if os.environ.get(e):
            b[k] = os.environ[e]
cmd = os.environ.get("P_CMD") or ""
if cmd:
    b["cmd"] = cmd
cwd = os.environ.get("P_CWD") or ""
if cwd:
    b["cwd"] = cwd
if os.environ.get("P_NO_TUNNEL") == "1":
    b["noTunnel"] = True
if os.environ.get("P_TRANSFER") == "1":
    b["transfer"] = True
pt = (os.environ.get("P_PROBE_TIMEOUT") or "").strip()
if pt:
    b["probeTimeout"] = pt
dov = (os.environ.get("P_DISK_OVERRIDE") or "").strip()
if dov:
    b["diskOverride"] = dov
if os.environ.get("P_SKIN") == "1":
    b["skin"] = True
    root = os.environ.get("P_SKIN_ROOT") or ""
    if root:
        b["skinRoot"] = root
print(json.dumps(b))
')
    RESP=$(cli_send cli_post_json "/cli/publish/run" "$body") || RESP=""
    local AS_JSON="$as_json" P_MODE="service" P_VERB="run"
    _publish_handle_resp
}

# `k2 publish start <name>` / `stop <name>`
_publish_named_post() {
    local verb="$1" endpoint="$2"
    shift 2
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local name="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --help|-h) _publish_usage; exit 0 ;;
            -*)
                as_json="$as_json" _publish_err "usage" \
                    "unknown flag '$1' — usage: k2 publish $verb <name>"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then
                    name="$1"; shift
                else
                    as_json="$as_json" _publish_err "usage" "unexpected argument '$1'"
                    exit 2
                fi ;;
        esac
    done
    if [ -z "$name" ]; then
        as_json="$as_json" _publish_err "usage" "usage: k2 publish $verb <name>"
        exit 2
    fi
    local body RESP
    body=$(P_NAME="$name" P_PROJECT="$PROJECT" python3 -c '
import json, os
print(json.dumps({"name": os.environ["P_NAME"], "project": os.environ["P_PROJECT"]}))
')
    RESP=$(cli_send cli_post_json "$endpoint" "$body") || RESP=""
    local AS_JSON="$as_json" P_MODE="service" P_VERB="$verb"
    _publish_handle_resp
}

# `k2 publish frame <name> (--allow <origin> ... | --none | --reset)` — owner
# only (prd-app-frame-ancestors-v1). Which other sites may put a --skin App in
# a frame. The daemon validates; bad origins exit 2, agents get exit 3.
cmd_publish_frame() {
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local name="" as_json=false mode="" allow=""
    local usage="usage: k2 publish frame <name> (--allow <origin> ... | --none | --reset) [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --allow)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--allow needs an origin, e.g. https://partner.example"; exit 2; }
                case "$mode" in ""|allow) ;; *) as_json="$as_json" _publish_err "usage" "$usage"; exit 2 ;; esac
                mode=allow; allow="${allow:+$allow }$2"; shift 2 ;;
            --allow=*)
                case "$mode" in ""|allow) ;; *) as_json="$as_json" _publish_err "usage" "$usage"; exit 2 ;; esac
                mode=allow; allow="${allow:+$allow }${1#--allow=}"; shift ;;
            --none)
                [ -z "$mode" ] || { as_json="$as_json" _publish_err "usage" "$usage"; exit 2; }
                mode=none; shift ;;
            --reset)
                [ -z "$mode" ] || { as_json="$as_json" _publish_err "usage" "$usage"; exit 2; }
                mode=reset; shift ;;
            --json) as_json=true; shift ;;
            --help|-h) _publish_usage; exit 0 ;;
            -*)
                as_json="$as_json" _publish_err "usage" "unknown flag '$1' — $usage"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then
                    name="$1"; shift
                else
                    as_json="$as_json" _publish_err "usage" "unexpected argument '$1' — $usage"
                    exit 2
                fi ;;
        esac
    done
    if [ -z "$name" ] || [ -z "$mode" ]; then
        as_json="$as_json" _publish_err "usage" "$usage"
        exit 2
    fi
    local body RESP
    body=$(P_NAME="$name" P_PROJECT="$PROJECT" P_MODE="$mode" P_ALLOW="$allow" python3 -c '
import json, os
b = {"name": os.environ["P_NAME"], "project": os.environ["P_PROJECT"]}
m = os.environ["P_MODE"]
if m == "allow":
    b["allow"] = os.environ["P_ALLOW"]
else:
    b[m] = True
print(json.dumps(b))
')
    RESP=$(cli_send cli_post_json "/cli/publish/frame" "$body") || RESP=""
    local AS_JSON="$as_json" P_MODE="service" P_VERB="frame"
    _publish_handle_resp
}

# `k2 publish client-ip [<label> on|off] [--json]` (0.46.3). A nested service
# (https://<label>.<host>.k2.dev) is a raw byte splice by default, whatever its
# protocol. `on`: K2 rewrites each HTTP/1.1 request instead. It drops client
# copies of X-Forwarded-For / X-Real-IP / Forwarded / X-K2-Client-IP and sets
# X-K2-Client-IP (+ X-Forwarded-For, X-Real-IP) to the client K2 saw. A
# non-HTTP/1.x request gets a 400. Apps get the rewrite by default; their
# manifest's "host": {"clientIp": false} turns it off. No args: list the
# labels that are on.
cmd_publish_client_ip() {
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local label="" state="" as_json=false
    local usage="usage: k2 publish client-ip [<label> on|off] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --help|-h) _publish_usage; exit 0 ;;
            -*) echo "unknown flag '$1' — $usage" >&2; exit 2 ;;
            *)
                if [ -z "$label" ]; then label="$1"
                elif [ -z "$state" ]; then state="$1"
                else echo "unexpected argument '$1' — $usage" >&2; exit 2
                fi
                shift ;;
        esac
    done
    local RESP
    if [ -z "$label" ]; then
        RESP=$(cli_send cli_request "/cli/publish/client-ip/list") || RESP=""
    else
        case "$state" in
            on|true) state=true ;;
            off|false) state=false ;;
            *) echo "$usage" >&2; exit 2 ;;
        esac
        local body
        body=$(P_LABEL="$label" P_ON="$state" python3 -c '
import json, os
print(json.dumps({"label": os.environ["P_LABEL"], "on": os.environ["P_ON"]}))
')
        RESP=$(cli_send cli_post_json "/cli/publish/client-ip" "$body") || RESP=""
    fi
    RESP="$RESP" AS_JSON="$as_json" python3 -c '
import json, os, sys
raw = os.environ.get("RESP", "")
try:
    d = json.loads(raw)
except Exception:
    print("k2 publish client-ip: no answer from the daemon: %s" % (raw.strip() or "(empty)"), file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict) or d.get("error"):
    print("k2 publish client-ip: %s" % (d.get("error") if isinstance(d, dict) else raw), file=sys.stderr)
    sys.exit(2)
if os.environ["AS_JSON"] == "true":
    print(json.dumps(d))
    sys.exit(0)
if "label" in d:
    print("%s: client IP %s" % (d["label"], "on (HTTP/1.1 rewrite, X-K2-Client-IP)" if d["clientIp"] else "off (raw splice)"))
labels = d.get("labels") or []
print("Nested services with the client-IP rewrite: %s" % (", ".join(labels) if labels else "none (all raw)"))
print("Apps: on unless the manifest says host.clientIp: false (not listed here). The App gets X-K2-Client-IP only when K2 knows the visitor: k2 tunnel status (Visitor IPs).")
'
}

# The code of a /cli/publish/* refusal in $RESP ("" when it isn't one).
_publish_resp_code() {
    RESP="$RESP" python3 -c '
import json, os
try:
    d = json.loads(os.environ.get("RESP") or "")
except Exception:
    d = None
print((d.get("code") or "") if isinstance(d, dict) else "")
'
}

# Print a visibility / open / sessions answer. Env: RESP, AS_JSON, P_VERB.
# Exit 0 done; 2 usage or held for a person; 3 refused.
_publish_site_render() {
    RESP="${RESP:-}" AS_JSON="${AS_JSON:-false}" P_VERB="${P_VERB:-}" python3 - <<'PY'
import json, os, sys

raw = os.environ.get("RESP") or ""
as_json = os.environ.get("AS_JSON") == "true"
verb = os.environ.get("P_VERB") or ""
USAGE = ("usage", "pin_required", "pin_length", "pin_characters", "pin_days", "pin_in_argv", "reason_required",
         "needs_yes", "dev_public_needs_flag", "bad_label", "bad_path")

def bail(code, text, status):
    if as_json:
        sys.stderr.write(json.dumps({"error": {"code": code, "hint": text}}, separators=(",", ":")) + "\n")
    else:
        sys.stderr.write("Error: %s\n" % text)
    sys.exit(status)

try:
    d = json.loads(raw)
except Exception:
    d = None
if not isinstance(d, dict):
    bail("unreachable", "no answer from the daemon: %s" % (raw.strip()[:200] or "(empty)"), 1)

if d.get("status") == "held":
    if as_json:
        print(json.dumps(d, separators=(",", ":")))
    else:
        print(d.get("message") or "This needs a person: K2 put a card on your Thread.")
        if d.get("request"):
            print("  request: %s" % d["request"])
    sys.exit(2)

err = d.get("error")
if err:
    if isinstance(err, dict):
        code, text = err.get("code") or "error", err.get("hint") or err.get("message") or "refused"
    else:
        code, text = d.get("code") or "error", str(err)
    if d.get("hint"):
        text = "%s\n  %s" % (text, d["hint"])
    bail(code, text, 2 if code in USAGE else 3)

if as_json:
    print(json.dumps(d, separators=(",", ":")))
    sys.exit(0)

def when(ts):
    import datetime
    try:
        return datetime.datetime.fromtimestamp(int(ts)).strftime("%Y-%m-%d %H:%M")
    except Exception:
        return "-"

if verb == "visibility":
    mode = d.get("mode") or "?"
    print("%s: %s%s" % (d.get("label") or "?", mode, ", dev" if d.get("dev") else ""))
    if d.get("url"):
        print("  %s" % d["url"])
    words = {
        "private": "Only the owner and Admins can open it (k2 publish open %s)." % (d.get("label") or "<name>"),
        "pin": "K2 asks for the PIN first. Send the address and the PIN separately.",
        "public": "Anyone with the address can open it.",
    }
    if mode in words:
        print("  %s" % words[mode])
    if mode == "public" and d.get("reason"):
        print("  Why: %s" % d["reason"])
    for line in d.get("warnings") or []:
        print("warning: %s" % line)
elif verb == "open":
    if d.get("opened"):
        print("Opened %s in your browser." % (d.get("label") or "the site"))
    else:
        print(d.get("url") or "")
        if d.get("oneTime"):
            print("  This link works once, within %s seconds." % (d.get("expiresInSecs") or 60), file=sys.stderr)
elif verb == "sessions":
    if d.get("revoked") is not None:
        print("Ended %s session%s on %s." % (d["revoked"], "" if d["revoked"] == 1 else "s", d.get("label") or "?"))
    else:
        rows = d.get("sessions") or []
        if not rows:
            print("Nobody is signed in to %s." % (d.get("label") or "?"))
        else:
            print("%-8s  %-24s  %-16s  %s" % ("KIND", "WHO", "STARTED", "ENDS"))
            for r in rows:
                print("%-8s  %-24s  %-16s  %s" % (r.get("kind") or "?", r.get("user") or "-", when(r.get("createdAt")), when(r.get("expiresAt"))))
PY
}

# Read a PIN into _K2_PIN: asked twice with no echo in a terminal, else one line
# from stdin. Never on argv. Non-zero when the two don't match.
_publish_read_pin() {
    _K2_PIN=""
    if [ -t 0 ]; then
        local again=""
        printf 'PIN for %s (6 to 64 characters): ' "$1" >&2
        IFS= read -rs _K2_PIN || _K2_PIN=""
        echo >&2
        printf 'The same PIN again: ' >&2
        IFS= read -rs again || again=""
        echo >&2
        [ "$_K2_PIN" = "$again" ] || return 1
    else
        IFS= read -r _K2_PIN || true
    fi
    return 0
}

# `k2 publish visibility <label> [private|pin|public] ...` (prd-publish-
# visibility-and-dev-mode-v1 PV-7, PV-10, PV-12, DM-3). The daemon decides who
# may; this reads the PIN and asks the person to confirm.
cmd_publish_visibility() {
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local label="" mode="" reason="" pin_days="" yes=0 dev_public=0 as_json=false
    local usage="usage: k2 publish visibility <name> [private|pin|public] [--reason <why>] [--pin-days N] [--yes] [--dev-is-public] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --pin|--pin=*)
                as_json="$as_json" _publish_err "pin_in_argv" "a PIN on the command line lands in your shell history and process lists. Pipe it, or leave it out and type it when asked: k2 publish visibility ${label:-<name>} pin < pin.txt"
                exit 2 ;;
            --reason)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--reason needs a few words on why"; exit 2; }
                reason="$2"; shift 2 ;;
            --reason=*) reason="${1#--reason=}"; shift ;;
            --pin-days)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--pin-days needs a number from 1 to 30"; exit 2; }
                pin_days="$2"; shift 2 ;;
            --pin-days=*) pin_days="${1#--pin-days=}"; shift ;;
            --yes|-y) yes=1; shift ;;
            --dev-is-public) dev_public=1; shift ;;
            --json) as_json=true; shift ;;
            -*) as_json="$as_json" _publish_err "usage" "unknown flag '$1' — $usage"; exit 2 ;;
            *)
                if [ -z "$label" ]; then label="$1"
                elif [ -z "$mode" ]; then mode="$1"
                else as_json="$as_json" _publish_err "usage" "unexpected argument '$1' — $usage"; exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$label" ] || { as_json="$as_json" _publish_err "usage" "$usage"; exit 2; }
    local RESP
    if [ -z "$mode" ]; then
        RESP=$(cli_send cli_request "/cli/publish/visibility" "label=$(urlencode "$label")") || RESP=""
        local AS_JSON="$as_json" P_VERB="visibility"
        _publish_site_render
        return $?
    fi
    case "$mode" in
        private|pin|public) ;;
        *) as_json="$as_json" _publish_err "usage" "the mode is private, pin or public — $usage"; exit 2 ;;
    esac
    case "$pin_days" in
        ''|[0-9]|[0-9][0-9]) ;;
        *) as_json="$as_json" _publish_err "usage" "--pin-days needs a number from 1 to 30"; exit 2 ;;
    esac

    # DM-3: public dev is loud. In a terminal the person types the name; --yes
    # is only for a call with no terminal.
    if [ "$mode" = public ] && [ "$dev_public" = 1 ]; then
        if [ -t 0 ] && [ -t 2 ]; then
            {
                echo "$label is a dev server. Made public, anyone with the address can see its source maps,"
                echo "error pages with file paths, and anything the dev server evaluates."
                printf 'Type %s to make it public: ' "$label"
            } >&2
            local typed=""
            IFS= read -r typed || typed=""
            [ "$typed" = "$label" ] || { echo "Nothing changed." >&2; exit 2; }
        elif [ "$yes" != 1 ]; then
            as_json="$as_json" _publish_err "needs_yes" "public dev needs a person at a terminal, or --yes with no terminal"
            exit 2
        fi
    fi

    # Ask first, with no PIN: an agent gets its card and a person who can't set
    # it gets refused, before anyone types one. The PIN then goes over TCP in
    # a body (curl config on stdin), never on argv.
    _publish_vis_body() {
        P_LABEL="$label" P_MODE_V="$mode" P_REASON="$reason" P_PIN_DAYS="$pin_days" P_YES="$yes" \
        P_DEV_PUBLIC="$dev_public" P_PROJECT="$PROJECT" P_CHECK="${1:-0}" python3 -c '
import json, os, sys
b = {"label": os.environ["P_LABEL"], "mode": os.environ["P_MODE_V"], "project": os.environ["P_PROJECT"]}
if os.environ.get("P_REASON"):
    b["reason"] = os.environ["P_REASON"]
if os.environ.get("P_PIN_DAYS"):
    b["pinDays"] = os.environ["P_PIN_DAYS"]
if os.environ.get("P_YES") == "1":
    b["yes"] = True
if os.environ.get("P_DEV_PUBLIC") == "1":
    b["devIsPublic"] = True
if os.environ.get("P_CHECK") == "1":
    b["check"] = True
pin = sys.stdin.read()
if pin:
    b["pin"] = pin
print(json.dumps(b))
'
    }
    local check=0 code=""
    [ "$mode" = pin ] && check=1
    while :; do
        RESP=$(printf '' | _publish_vis_body "$check" | { IFS= read -r b; cli_send cli_post_json "/cli/publish/visibility" "$b"; }) || RESP=""
        code="$(_publish_resp_code)"
        [ "$code" = needs_yes ] || break
        # PV-3: moving a public site behind the gate changes how K2 carries it.
        if [ -t 0 ] && [ -t 2 ]; then
            RESP="$RESP" python3 -c 'import json,os; print(json.loads(os.environ["RESP"]).get("error",""))' >&2
            printf 'Go ahead? [y/N] ' >&2
            local answer=""
            IFS= read -r answer || answer=""
            case "$answer" in y|Y|yes|YES) yes=1 ;; *) echo "Nothing changed." >&2; exit 2 ;; esac
        else
            break
        fi
    done
    if [ "$mode" = pin ] && [ -z "$code" ] && [ "$check" = 1 ] && RESP="$RESP" python3 -c 'import json,os,sys; sys.exit(0 if json.loads(os.environ["RESP"]).get("check") else 1)' 2>/dev/null; then
        local _K2_PIN=""
        if ! _publish_read_pin "$label"; then
            as_json="$as_json" _publish_err "usage" "the two PINs don't match; nothing changed"
            exit 2
        fi
        [ -n "$_K2_PIN" ] || { as_json="$as_json" _publish_err "pin_required" "no PIN on stdin; nothing changed"; exit 2; }
        _K2_FORCE_TCP=1
        RESP=$(printf '%s' "$_K2_PIN" | _publish_vis_body 0 | { IFS= read -r b; cli_post_json "/cli/publish/visibility" "$b"; }) || RESP=""
        _K2_PIN=""
    fi
    local AS_JSON="$as_json" P_VERB="visibility"
    _publish_site_render
}

# `k2 publish open <label> [--path p] [--no-browser] [--json]` (PV-5): a
# one-time link from a signed-in K2 client. Agents are refused (person_only).
cmd_publish_open() {
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local label="" path="" as_json=false no_browser=0
    local usage="usage: k2 publish open <name> [--path <p>] [--no-browser] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --path)
                [ $# -ge 2 ] || { as_json="$as_json" _publish_err "usage" "--path needs a path, like /docs"; exit 2; }
                path="$2"; shift 2 ;;
            --path=*) path="${1#--path=}"; shift ;;
            --no-browser) no_browser=1; shift ;;
            --json) as_json=true; shift ;;
            -*) as_json="$as_json" _publish_err "usage" "unknown flag '$1' — $usage"; exit 2 ;;
            *)
                if [ -z "$label" ]; then label="$1"
                else as_json="$as_json" _publish_err "usage" "unexpected argument '$1' — $usage"; exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$label" ] || { as_json="$as_json" _publish_err "usage" "$usage"; exit 2; }
    local body RESP
    body=$(P_LABEL="$label" P_PATH="$path" P_PROJECT="$PROJECT" python3 -c '
import json, os
b = {"label": os.environ["P_LABEL"], "project": os.environ["P_PROJECT"]}
if os.environ.get("P_PATH"):
    b["path"] = os.environ["P_PATH"]
print(json.dumps(b))
')
    RESP=$(cli_send cli_post_json "/cli/publish/open" "$body") || RESP=""
    # Open a browser only for a person at a terminal with one. A one-time link
    # is spent once it opens, so it is printed only when it wasn't.
    if [ "$as_json" = false ] && [ "$no_browser" = 0 ] && [ -t 1 ]; then
        local url
        url="$(RESP="$RESP" python3 -c 'import json,os
try:
    print(json.loads(os.environ["RESP"]).get("url") or "")
except Exception:
    print("")')"
        if [ -n "$url" ]; then
            local opener=""
            case "$(uname -s)" in
                Darwin) opener=open ;;
                *) if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ] && command -v xdg-open >/dev/null 2>&1; then opener=xdg-open; fi ;;
            esac
            if [ -n "$opener" ] && "$opener" "$url" >/dev/null 2>&1; then
                RESP="$(RESP="$RESP" python3 -c 'import json,os; d=json.loads(os.environ["RESP"]); d["opened"]=True; print(json.dumps(d))')"
            fi
        fi
    fi
    local AS_JSON="$as_json" P_VERB="open"
    _publish_site_render
}

# `k2 publish sessions <label> [--revoke-all] [--json]` (PV-5, PV-12).
cmd_publish_sessions() {
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local label="" revoke=0 as_json=false
    local usage="usage: k2 publish sessions <name> [--revoke-all] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --revoke-all) revoke=1; shift ;;
            --json) as_json=true; shift ;;
            -*) as_json="$as_json" _publish_err "usage" "unknown flag '$1' — $usage"; exit 2 ;;
            *)
                if [ -z "$label" ]; then label="$1"
                else as_json="$as_json" _publish_err "usage" "unexpected argument '$1' — $usage"; exit 2
                fi
                shift ;;
        esac
    done
    [ -n "$label" ] || { as_json="$as_json" _publish_err "usage" "$usage"; exit 2; }
    local RESP
    if [ "$revoke" = 1 ]; then
        local body
        body=$(P_LABEL="$label" P_PROJECT="$PROJECT" python3 -c '
import json, os
print(json.dumps({"label": os.environ["P_LABEL"], "project": os.environ["P_PROJECT"]}))
')
        RESP=$(cli_send cli_post_json "/cli/publish/sessions/revoke" "$body") || RESP=""
    else
        RESP=$(cli_send cli_request "/cli/publish/sessions" "label=$(urlencode "$label")") || RESP=""
    fi
    local AS_JSON="$as_json" P_VERB="sessions"
    _publish_site_render
}

cmd_publish_start() { _publish_named_post start "/cli/publish/start" "$@"; }
cmd_publish_stop()  { _publish_named_post stop  "/cli/publish/stop"  "$@"; }

# `k2 publish rm <name> [--keep-hostname]`  — stop + drop the service row.
# Distinct from `k2 publish subdomain rm <label>` (hostname only).
cmd_publish_rm() {
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local name="" keep=0 as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --keep-hostname) keep=1; shift ;;
            --json) as_json=true; shift ;;
            --help|-h) _publish_usage; exit 0 ;;
            -*)
                as_json="$as_json" _publish_err "usage" \
                    "unknown flag '$1' — usage: k2 publish rm <name> [--keep-hostname]  (not: k2 publish subdomain rm)"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then
                    name="$1"; shift
                else
                    as_json="$as_json" _publish_err "usage" "unexpected argument '$1'"
                    exit 2
                fi ;;
        esac
    done
    if [ -z "$name" ]; then
        as_json="$as_json" _publish_err "usage" \
            "usage: k2 publish rm <name> [--keep-hostname]  (service; for a hostname use: k2 publish subdomain rm <label>)"
        exit 2
    fi
    local body RESP
    body=$(P_NAME="$name" P_PROJECT="$PROJECT" P_KEEP="$keep" python3 -c '
import json, os
b = {"name": os.environ["P_NAME"], "project": os.environ["P_PROJECT"]}
if os.environ.get("P_KEEP") == "1":
    b["keepHostname"] = True
print(json.dumps(b))
')
    RESP=$(cli_send cli_post_json "/cli/publish/rm" "$body") || RESP=""
    local AS_JSON="$as_json" P_MODE="service" P_VERB="rm"
    _publish_handle_resp
}

# `k2 publish ps [--json]` — this workspace's hosted processes (not Connect plan).
cmd_publish_ps() {
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local as_json=false long=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --long|-l) long=1; shift ;;
            --help|-h) _publish_usage; exit 0 ;;
            -*)
                as_json="$as_json" _publish_err "usage" "unknown flag '$1' — usage: k2 publish ps [--long] [--json]"
                exit 2 ;;
            *)
                as_json="$as_json" _publish_err "usage" "unexpected argument '$1' — usage: k2 publish ps [--long] [--json]"
                exit 2 ;;
        esac
    done
    local RESP
    RESP=$(cli_send cli_request "/cli/publish/list") || RESP=""
    local AS_JSON="$as_json" P_MODE="list" P_VERB="ps"
    P_LONG="$long" _publish_handle_resp
}

# `k2 publish logs <name> [--tail]` — last N lines on GET (P17: not a live WS).
cmd_publish_logs() {
    if _wants_help "$@"; then _publish_usage; exit 0; fi
    local name="" follow=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --tail) follow=1; shift ;;
            --help|-h) _publish_usage; exit 0 ;;
            -*)
                _publish_err "usage" "unknown flag '$1' — usage: k2 publish logs <name> [--tail]"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then
                    name="$1"; shift
                else
                    _publish_err "usage" "unexpected argument '$1'"
                    exit 2
                fi ;;
        esac
    done
    if [ -z "$name" ]; then
        _publish_err "usage" "usage: k2 publish logs <name> [--tail]"
        exit 2
    fi
    local RESP
    if [ "$follow" = 1 ]; then
        RESP=$(cli_send cli_request "/cli/publish/logs" \
            "name=$(urlencode "$name")" "tail=1") || RESP=""
    else
        RESP=$(cli_send cli_request "/cli/publish/logs" \
            "name=$(urlencode "$name")") || RESP=""
    fi
    local AS_JSON=false P_MODE="logs" P_VERB="logs"
    _publish_handle_resp
}

# `k2 publish status` — show the CONNECTED subdomain and its plan.
#
# Resolves the local tunnel (token + subdomain label from ~/.k2/tunnel.json),
# calls GET /subdomains (authenticated by the token), and picks the row for
# THIS tunnel: preferring a server-set `connected`/`current` flag (the row
# whose tunnel_token matched), else falling back to the local subdomain
# label. Displays the per-row `tier` (free|single|pro) as a friendly plan.
# Display is UX only — the create/point gate is enforced server-side.
# GH#33: --json → single JSON object (mail/dns contract).
# `k2 publish status <name>`: one App's state with lakebed-style loopable
# exit codes (decision-k2-runtime-v1 stage 0 (e)): 0 running, 2 starting,
# 1 failed, each with a suggestion line. Reads the daemon's service list.
_publish_app_status() {
    local name="$1" as_json="$2" raw
    raw=$(cli_send cli_request "/cli/publish/list") || raw=""
    PS_RAW="$raw" PS_NAME="$name" PS_JSON="$as_json" python3 -c '
import json, os, sys
raw = os.environ.get("PS_RAW") or ""
name = os.environ["PS_NAME"].strip().lower()
as_json = os.environ.get("PS_JSON") == "true"
try:
    d = json.loads(raw)
except Exception:
    print("Error: no usable answer from the K2 daemon on this computer", file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict) or "error" in d:
    print("Error: %s" % ((d.get("error") if isinstance(d, dict) else None) or raw[:200]), file=sys.stderr)
    sys.exit(1)
svc = next((s for s in d.get("services") or [] if isinstance(s, dict) and str(s.get("name", "")).lower() == name), None)
if svc is None:
    out = {"name": name, "status": "missing", "exit": 1,
           "suggestion": "no published App named %s in this workspace: k2 publish ps lists them" % name}
else:
    st = str(svc.get("status") or "")
    st_label = st
    err = svc.get("error")
    code = svc.get("lastExitCode")
    if st == "running":
        exit_code, sug = 0, "it is up" + (" at %s" % svc["url"] if svc.get("url") else "")
    elif st == "starting":
        left = svc.get("startingLeftMs")
        if left is not None:
            secs = max(0, int(left) // 1000)
            m, s = divmod(secs, 60)
            left_s = ("%dm%02ds" % (m, s)) if m and s else (("%dm" % m) if m else ("%ds" % s))
            st_label = "starting (waiting for port %s, %s left)" % (svc.get("port"), left_s)
        exit_code, sug = 2, "still starting: run k2 publish status %s again in a few seconds" % name
        if svc.get("hostnamePending"):
            sug += " (the public URL is published once the port answers)"
    elif st == "stopped":
        exit_code, sug = 1, "it is stopped: k2 publish start %s" % name
    elif st == "unhealthy":
        exit_code, sug = 1, "the process runs but nothing answers on its port: check it listens on %s (k2 publish logs %s)" % (svc.get("target") or "its port", name)
    else:
        exit_code, sug = 1, "it exited%s: read k2 publish logs %s, fix it, check the code with k2 app validate %s, then k2 publish start %s" % (
            " with code %s" % code if code is not None else "", name, name, name)
    out = {"name": name, "status": st, "desired": svc.get("desired"), "error": err,
           "lastExitCode": code, "url": svc.get("url"), "suggestion": sug, "exit": exit_code,
           "probeTimeoutMs": svc.get("probeTimeoutMs"),
           "probeTimeoutEffectiveMs": svc.get("probeTimeoutEffectiveMs"),
           "startingLeftMs": svc.get("startingLeftMs"),
           "hostnamePending": bool(svc.get("hostnamePending")),
           "statusText": st_label}
if as_json:
    print(json.dumps(out, indent=2))
else:
    line = "%s: %s" % (name, out.get("statusText") or out["status"])
    if out.get("error"):
        line += " (%s)" % out["error"]
    print(line)
    print("  suggestion: %s" % out["suggestion"])
sys.exit(out["exit"])
'
}

cmd_publish_status() {
    local as_json=false app=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --help|-h)
                cat <<'EOF'
Show the connected K2 Connect subdomain and plan for this device, or one
published App's state (loopable: an agent can wait on it).

USAGE
  k2 publish status [--json]
  k2 publish status <name> [--json]

FLAGS
  --json                 JSON: {connected, host, plan, tier, nested_subdomains}
                         <name>: {name, status, statusText, desired, error, lastExitCode,
                                  probeTimeoutMs, probeTimeoutEffectiveMs, startingLeftMs,
                                  hostnamePending, suggestion, exit}

EXIT CODES
  without <name>: 0 ok, 1 error (no tunnel token / control plane)
  with <name>:    0 running, 2 starting (loop: run it again), 1 exited,
                  unhealthy, stopped or not found (the suggestion says what next)
EOF
                exit 0 ;;
            -*)
                _publish_err "usage" "unknown flag '$1' — usage: k2 publish status [<name>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$app" ]; then app="$1"; shift; continue; fi
                _publish_err "usage" "unexpected argument '$1' — usage: k2 publish status [<name>] [--json]"
                exit 2 ;;
        esac
    done
    if [ -n "$app" ]; then
        _publish_app_status "$app" "$as_json"
        return
    fi
    # 0.45.7: no tunnel.json token check here — a desktop keeps the token
    # in the K2 vault; the daemon reports a missing one (_connect_curl).
    local self cp_raw
    self="$(_connect_self_subdomain)"
    cp_raw="$(_connect_curl GET "/subdomains")" || exit $?
    if [ "$as_json" = "true" ]; then
        printf '%s' "$cp_raw" | SELF="$self" python3 -c '
import json, os, sys
raw = sys.stdin.read()
nl = raw.find("\n")
if nl < 0:
    print(json.dumps({"error":{"code":"unreachable","hint":"no response from control plane"}}), file=sys.stderr)
    sys.exit(1)
status, body = raw[:nl].strip(), raw[nl+1:]
try:
    code = int(status)
except ValueError:
    print(json.dumps({"error":{"code":"bad_status","hint":status}}), file=sys.stderr)
    sys.exit(1)
try:
    d = json.loads(body) if body.strip() else {}
except Exception:
    d = {}
if not (200 <= code < 300):
    err = (d.get("error") if isinstance(d, dict) else None) or "http_%s" % code
    print(json.dumps({"error":{"code":str(err),"hint":(d.get("detail") if isinstance(d, dict) else None) or body[:200]}}), file=sys.stderr)
    sys.exit(1)
self_label = (os.environ.get("SELF") or "").strip().lower()
subs = d.get("subdomains", []) if isinstance(d, dict) else []

def host_of(s):
    return s.get("host") or (str(s.get("label","?")) + ".k2.dev")

def label_of(s):
    lbl = s.get("label")
    if lbl:
        return str(lbl).strip().lower()
    return host_of(s).split(".")[0].strip().lower()

row = None
for s in subs:
    if s.get("connected") or s.get("current") or s.get("is_current"):
        row = s; break
if row is None and self_label:
    for s in subs:
        if label_of(s) == self_label:
            row = s; break
if row is None:
    host = ("%s.k2.dev" % self_label) if self_label else None
    print(json.dumps({
        "connected": bool(self_label),
        "host": host,
        "plan": None,
        "tier": None,
        "nested_subdomains": False,
    }, separators=(",", ":")))
    sys.exit(0)
tier = (row.get("tier") or "").strip().lower()
print(json.dumps({
    "connected": True,
    "host": host_of(row),
    "plan": {"pro": "Pro", "single": "Single", "free": "Free"}.get(tier, tier or None),
    "tier": tier or None,
    "nested_subdomains": tier == "pro",
}, separators=(",", ":")))
'
        return 0
    fi
    printf '%s' "$cp_raw" | SELF="$self" _connect_subdomain_render '
import os
self_label = (os.environ.get("SELF") or "").strip().lower()
subs = d.get("subdomains", []) if isinstance(d, dict) else []

def host_of(s):
    return s.get("host") or (str(s.get("label","?")) + ".k2.dev")

def label_of(s):
    lbl = s.get("label")
    if lbl:
        return str(lbl).strip().lower()
    return host_of(s).split(".")[0].strip().lower()

# Prefer the server-marked connected row (token match); else match the
# local tunnel subdomain label.
row = None
for s in subs:
    if s.get("connected") or s.get("current") or s.get("is_current"):
        row = s; break
if row is None and self_label:
    for s in subs:
        if label_of(s) == self_label:
            row = s; break

if row is None:
    if self_label:
        print("Connected: %s.k2.dev" % self_label)
        print("  Plan: unknown (subdomain not found in your account list)")
    else:
        print("Not connected to a K2 Connect subdomain.")
        print("  Select a subdomain in Settings -> K2 Connect (or run k2 tunnel start).")
    sys.exit(0)

host = host_of(row)
tier = (row.get("tier") or "").strip().lower()
if tier == "pro":
    plan = "Pro ✓ (nested subdomains enabled)"
elif tier == "single":
    plan = "Single — upgrade to Pro to create nested subdomains"
elif tier == "free":
    plan = "Free"
elif tier:
    plan = tier.capitalize()
else:
    plan = "unknown"
print("Connected: %s · Plan: %s" % (host, plan))
if tier in ("single", "free"):
    print("  Manage your plan on the dashboard: https://k2.dev/dashboard")
'
}

# NOTE: subscription/billing is intentionally NOT in the CLI. Upgrading,
# downgrading, cancelling, and any tier change happen ONLY on the website
# dashboard (https://k2.dev/dashboard). The CLI exists to MAKE/MANAGE
# subdomains, not to manage the subscription.

# 0074 — record (or drop) a nested label's WORKSPACE attribution on the
# LOCAL daemon: POST /cli/publish/subdomain/{claim,unclaim} with the
# label (optional target). cli_post carries this shell's $PROJECT.
# Cell UDS + stamp_principal via cli_send. Echoes the response body;
# non-zero when the daemon connection is missing/unreachable.
_subdomain_attrib_post() {
    local verb="$1" label="$2" target="${3:-}"
    if [ $# -ge 3 ]; then shift 3; else shift $#; fi
    if [ -z "$PORT" ] || [ -z "$TOKEN" ]; then return 1; fi
    local fields=("label=$(urlencode "$label")")
    if [ -n "$target" ]; then
        fields+=("target=$(urlencode "$target")")
    fi
    # Extra form fields (check=1 / transfer=1) ride after the target.
    cli_send cli_post "/cli/publish/subdomain/${verb}" "${fields[@]}" "$@"
}

# One owner per label: ask the LOCAL daemon (read only, check=1) whether
# this workspace may take <label> BEFORE any control-plane create/point/rm,
# so a name another workspace on this server owns is never touched.
# Returns non-zero (message on stderr) on refusal or when the daemon is
# unreachable. Usage: _subdomain_label_preflight <label> [transfer 0|1]
# A third arg `release` = the check before a REMOVE: an old `meet`/`chat`
# label (reserved after it was made) can still be deleted.
_subdomain_label_preflight() {
    local label="$1" transfer="${2:-0}" mode="${3:-}" resp
    if [ -z "$PORT" ] || [ -z "$TOKEN" ]; then
        echo "Error: the local K2 daemon isn't reachable, so ownership of '$label' can't be checked." >&2
        return 1
    fi
    if [ "$mode" = release ]; then
        resp=$(_subdomain_attrib_post claim "$label" "" "check=1" "release=1") || resp=""
    elif [ "$transfer" = 1 ]; then
        resp=$(_subdomain_attrib_post claim "$label" "" "check=1" "transfer=1") || resp=""
    else
        resp=$(_subdomain_attrib_post claim "$label" "" "check=1") || resp=""
    fi
    printf '%s' "$resp" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read() or "{}")
except Exception:
    d = {}
if isinstance(d, dict) and d.get("success"):
    sys.exit(0)
err = (d.get("error") if isinstance(d, dict) else None) or "no response from the daemon"
print("Error: %s" % err, file=sys.stderr)
if isinstance(d, dict) and d.get("hint"):
    print("  %s" % d.get("hint"), file=sys.stderr)
sys.exit(1)
'
}

# Load-bearing attribution stamp AFTER a successful control-plane
# create/point/rm. Stamp fail → non-zero even if the hostname exists
# on the account. Thin wrapper — does not swallow.
# Usage: _stamp_subdomain_attrib <claim|unclaim> <label> [target] [transfer 0|1]
_stamp_subdomain_attrib() {
    local verb="$1" label="$2" target="${3:-}" transfer="${4:-0}" resp err
    if [ -z "$PORT" ] || [ -z "$TOKEN" ]; then
        echo "Error: workspace attribution was not recorded on the daemon (unreachable)." >&2
        return 1
    fi
    if [ "$transfer" = 1 ]; then
        resp=$(_subdomain_attrib_post "$verb" "$label" "$target" "transfer=1") || resp=""
    else
        resp=$(_subdomain_attrib_post "$verb" "$label" "$target") || resp=""
    fi
    if printf '%s' "$resp" | python3 -c 'import json,sys; sys.exit(0 if (json.loads(sys.stdin.read() or "{}") or {}).get("success") else 1)' 2>/dev/null; then
        return 0
    fi
    err=$(printf '%s' "$resp" | python3 -c 'import json,sys
try:
    d=json.loads(sys.stdin.read() or "{}")
    print((d.get("error") if isinstance(d, dict) else None) or "no response from the daemon")
except Exception:
    print("no response from the daemon")
' 2>/dev/null) || err="no response from the daemon"
    if [ "$verb" = "claim" ]; then
        echo "Error: workspace attribution was not recorded on the daemon: $err" >&2
        echo "  Adopt it later from the owning workspace: k2 publish subdomain claim $label" >&2
    else
        echo "Error: workspace attribution was not removed on the daemon: $err" >&2
        echo "  Remove it later: k2 publish subdomain unclaim $label" >&2
    fi
    return 1
}

# Best-effort realtime nudge AFTER a successful control-plane
# create/point/rm (and its attribution stamp): ask the LOCAL daemon to
# re-pull the subdomain map from the control plane NOW, so the
# new/changed URL appears in the UIs immediately instead of on the next
# periodic poll. The daemon runs the same fetch its poll loop uses and
# broadcasts tunnel_subdomains_changed only when the map actually
# changed. A miss (daemon unreachable / tunnel unconfigured) must not
# fail the command — the control-plane op already succeeded and the map
# converges on the next poll. Silent + always returns 0.
_nudge_subdomain_refresh() {
    if [ -z "$PORT" ] || [ -z "$TOKEN" ]; then return 0; fi
    cli_post_form "/cli/tunnel/subdomains/refresh" >/dev/null 2>&1 || true
    return 0
}

# Hint only (never blocks, always returns 0): when a nested target is
# local (`localhost`, `127.0.0.1`, `[::1]`) and nothing is listening on
# it, say so. For `localhost` the daemon dials 127.0.0.1 then ::1, so we
# probe both and warn only when neither answers.
_warn_if_local_target_down() {
    local target="${1:-}"
    [ -n "$target" ] || return 0
    command -v python3 >/dev/null 2>&1 || return 0
    python3 - "$target" <<'PY' >&2 || true
import socket, sys
t = sys.argv[1].strip()
for p in ("http://", "https://"):
    if t.startswith(p):
        t = t[len(p):]
t = t.split("/", 1)[0]
if t.startswith("["):
    host, _, rest = t[1:].partition("]")
    port = rest[1:] if rest.startswith(":") else ""
else:
    host, _, port = t.rpartition(":")
try:
    port = int(port)
except ValueError:
    sys.exit(0)
h = host.lower().rstrip(".")
if h == "localhost":
    addrs = [("127.0.0.1", socket.AF_INET), ("::1", socket.AF_INET6)]
elif h == "127.0.0.1":
    addrs = [("127.0.0.1", socket.AF_INET)]
elif h == "::1":
    addrs = [("::1", socket.AF_INET6)]
else:
    sys.exit(0)
for ip, fam in addrs:
    try:
        s = socket.socket(fam, socket.SOCK_STREAM)
        s.settimeout(0.5)
        s.connect((ip, port))
        s.close()
        sys.exit(0)
    except OSError:
        pass
tried = " or ".join("[%s]:%d" % (ip, port) if ":" in ip else "%s:%d" % (ip, port) for ip, _ in addrs)
print("Warning: nothing is listening on %s yet. Visitors will see 502 (service not reachable) until your app is running on that port." % tried)
PY
    return 0
}

# Reserved nested labels: box-level front doors, never a publish target.
#   skin = the App front door; meet = K2 Meet; chat = K2 chat (Sew);
#   mail = K2 webmail (web only; never a mail-server name).
# Same set and messages as k2_core::skin::RESERVED_NESTED_LABELS.
_refuse_reserved_nested_label() {
    local label
    label=$(printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')
    case "$label" in
        skin)
            echo "Error: reserved_label: 'skin' is reserved. Pick another nested label for your UI (k2 study apps; leftover: k2 study skins)." >&2
            exit 1 ;;
        meet)
            echo "Error: reserved_label: 'meet' is reserved for K2 Meet (meet.<box>.k2.dev). Pick another nested label." >&2
            exit 1 ;;
        chat)
            echo "Error: reserved_label: 'chat' is reserved for K2 chat (chat.<box>.k2.dev). Pick another nested label." >&2
            exit 1 ;;
        mail)
            echo "Error: reserved_label: 'mail' is reserved for K2 webmail (mail.<box>.k2.dev). Pick another nested label." >&2
            exit 1 ;;
    esac
}

cmd_publish_subdomain() {
    local action="${1:-list}"
    shift 2>/dev/null || true
    # 0074 — claim/unclaim adopt/release a label's workspace attribution
    # on the LOCAL daemon only (POST /cli/publish/subdomain/{claim,unclaim},
    # no control-plane call): claim adopts a pre-existing / remote-created
    # label for the CURRENT workspace; unclaim drops the attribution.
    # Self-only (foreign 0074 → 403). Fail loud — this verb's whole job
    # is the daemon write.
    case "$action" in
        claim|unclaim)
            local label="" transfer=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --transfer) transfer=1; shift ;;
                    --*) echo "Error: unknown flag '$1'" >&2; exit 2 ;;
                    *) [ -z "$label" ] && label="$1"; shift ;;
                esac
            done
            if [ -z "$label" ]; then
                echo "Usage: k2 publish subdomain $action <label>" >&2
                if [ "$action" = claim ]; then
                    echo "       k2 publish subdomain claim <label> --transfer   (owner/admin; audited)" >&2
                fi
                exit 1
            fi
            if [ "$transfer" = 1 ] && [ "$action" != claim ]; then
                echo "Error: --transfer only applies to claim/create/point/run" >&2
                exit 2
            fi
            if [ -z "$PORT" ] || [ -z "$TOKEN" ]; then
                echo "Error: '$action' records attribution on the local K2 daemon, which isn't reachable." >&2
                echo "(Connection details: ~/.k2/heartbeat.port and ~/.k2/heartbeat.token)" >&2
                exit 1
            fi
            local resp
            if [ "$transfer" = 1 ]; then
                resp=$(_subdomain_attrib_post "$action" "$label" "" "transfer=1") || resp=""
            else
                resp=$(_subdomain_attrib_post "$action" "$label") || resp=""
            fi
            printf '%s' "$resp" | ACTION="$action" LABEL="$label" python3 -c '
import json, os, sys
action, label = os.environ["ACTION"], os.environ["LABEL"]
try:
    d = json.loads(sys.stdin.read() or "{}")
except Exception:
    d = {}
if not isinstance(d, dict) or not d.get("success"):
    err = (d.get("error") if isinstance(d, dict) else None) or "no response from the daemon"
    print(f"Error: {err}", file=sys.stderr)
    if isinstance(d, dict) and d.get("hint"):
        print("  %s" % d.get("hint"), file=sys.stderr)
    sys.exit(1)
if action == "claim" and d.get("transferred"):
    print("Transferred %s to this workspace (project %s). Recorded in the access audit log." % (label, d.get("projectId") or "?"))
elif action == "claim":
    # %-format, not a nested-quote f-string — this program lives inside a
    # single-quoted bash string, so a bare apostrophe would cut it.
    print("Claimed %s for this workspace (project %s)." % (label, d.get("projectId") or "?"))
elif d.get("removed"):
    print(f"Unclaimed {label} — no workspace attribution remains.")
else:
    print(f"{label} had no workspace attribution (nothing to remove).")
'
            return 0
            ;;
    esac
    # 0.45.7: no tunnel.json token gate — a desktop keeps the token in the
    # K2 vault. Each control-plane call is captured first
    # (`cp_raw="$(_connect_curl …)" || exit $?`): a subshell `exit` inside a
    # pipeline would not stop the renderer half of the pipe.
    local cp_raw
    case "$action" in
        create)
            local label="" target="" transfer=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --target) target="${2:-}"; shift 2 ;;
                    --transfer) transfer=1; shift ;;
                    --*) shift ;;
                    *) [ -z "$label" ] && label="$1"; shift ;;
                esac
            done
            if [ -z "$label" ] || [ -z "$target" ]; then
                echo "Usage: k2 publish subdomain create <label> --target <endpoint> [--transfer]" >&2
                echo "  e.g. k2 publish subdomain create staging --target localhost:3000" >&2
                exit 1
            fi
            _refuse_reserved_nested_label "$label"
            _subdomain_label_preflight "$label" "$transfer" || exit 1
            local body
            body="$(python3 -c 'import json,sys; print(json.dumps({"label": sys.argv[1], "target": sys.argv[2]}))' "$label" "$target")"
            cp_raw="$(_connect_curl POST "/subdomains" "$body")" || exit $?
            printf '%s' "$cp_raw" | _connect_subdomain_render '
host = d.get("host") or (str(d.get("label","?")) + ".k2.dev")
print("Created %s" % host)
print("  target:    %s" % d.get("target","?"))
print("  status:    %s" % d.get("status","?"))
print("  cert:      %s" % d.get("cert_status","?"))
print("  effective: %s" % d.get("effective","?"))
'
            # 0074 — the create succeeded (set -e would have exited
            # otherwise): attribute the label to the acting workspace,
            # then nudge the daemon's map refresh so the URL is live in
            # the UIs immediately.
            _stamp_subdomain_attrib claim "$label" "$target" "$transfer"
            _nudge_subdomain_refresh
            _warn_if_local_target_down "$target"
            ;;
        list)
            # GH#33: --json for agent scripting (parity with tunnel/dns/mail).
            local as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --help|-h)
                        cat <<'EOF'
List your K2 Connect subdomains (primary + nested).

USAGE
  k2 publish subdomain list [--json]

FLAGS
  --json                 JSON: {subdomains:[{host,target,plan,tier,status,flags}]}
EOF
                        exit 0 ;;
                    -*)
                        echo "Error: unknown flag '$1'" >&2
                        exit 2 ;;
                    *)
                        echo "Error: unexpected argument '$1'" >&2
                        exit 2 ;;
                esac
            done
            local self
            self="$(_connect_self_subdomain)"
            cp_raw="$(_connect_curl GET "/subdomains")" || exit $?
            if [ "$as_json" = "true" ]; then
                printf '%s' "$cp_raw" | SELF="$self" python3 -c '
import json, os, sys
raw = sys.stdin.read()
nl = raw.find("\n")
if nl < 0:
    print(json.dumps({"error":{"code":"unreachable","hint":"no response from control plane"}}), file=sys.stderr)
    sys.exit(1)
status, body = raw[:nl].strip(), raw[nl+1:]
try:
    code = int(status)
except ValueError:
    print(json.dumps({"error":{"code":"bad_status","hint":status}}), file=sys.stderr)
    sys.exit(1)
try:
    d = json.loads(body) if body.strip() else {}
except Exception:
    d = {}
if not (200 <= code < 300):
    err = (d.get("error") if isinstance(d, dict) else None) or "http_%s" % code
    print(json.dumps({"error":{"code":str(err),"hint":(d.get("detail") if isinstance(d, dict) else None) or body[:200]}}), file=sys.stderr)
    sys.exit(1)
self_label = (os.environ.get("SELF") or "").strip().lower()
subs = d.get("subdomains", []) if isinstance(d, dict) else []

def label_of(s):
    lbl = s.get("label")
    if lbl:
        return str(lbl).strip().lower()
    return (s.get("host") or "").split(".")[0].strip().lower()

def plan_of(s):
    t = (s.get("tier") or "").strip().lower()
    return {"pro": "Pro", "single": "Single", "free": "Free"}.get(t, t or None)

out = []
for s in subs:
    flags = []
    if s.get("primary"):
        flags.append("primary")
    if s.get("connected") or s.get("current") or s.get("is_current") \
       or (self_label and label_of(s) == self_label):
        flags.append("connected")
    out.append({
        "host": s.get("host") or (str(s.get("label","?")) + ".k2.dev"),
        "target": s.get("target") or "",
        "plan": plan_of(s),
        "tier": (s.get("tier") or "").strip().lower() or None,
        "status": s.get("effective") or s.get("status") or "",
        "flags": flags,
        "label": s.get("label"),
    })
print(json.dumps({"subdomains": out}, separators=(",", ":")))
'
                return 0
            fi
            printf '%s' "$cp_raw" | SELF="$self" _connect_subdomain_render '
import os
self_label = (os.environ.get("SELF") or "").strip().lower()
subs = d.get("subdomains", []) if isinstance(d, dict) else []
def label_of(s):
    lbl = s.get("label")
    if lbl:
        return str(lbl).strip().lower()
    return (s.get("host") or "").split(".")[0].strip().lower()
def plan_of(s):
    t = (s.get("tier") or "").strip().lower()
    return {"pro": "Pro", "single": "Single", "free": "Free"}.get(t, t or "-")
if not subs:
    print("No subdomains.")
else:
    rows = []
    for s in subs:
        flags = []
        if s.get("primary"):
            flags.append("primary")
        if s.get("connected") or s.get("current") or s.get("is_current") \
           or (self_label and label_of(s) == self_label):
            flags.append("connected")
        rows.append((
            s.get("host") or (str(s.get("label","?")) + ".k2.dev"),
            str(s.get("target","")),
            plan_of(s),
            str(s.get("effective") or s.get("status","")),
            " ".join(flags),
        ))
    hw = max([len(r[0]) for r in rows] + [len("HOST")])
    tw = max([len(r[1]) for r in rows] + [len("TARGET")])
    pw = max([len(r[2]) for r in rows] + [len("PLAN")])
    sw = max([len(r[3]) for r in rows] + [len("STATUS")])
    print("  %-*s  %-*s  %-*s  %-*s  %s" % (hw,"HOST",tw,"TARGET",pw,"PLAN",sw,"STATUS","FLAGS"))
    for h,t,pl,st,fl in rows:
        print("  %-*s  %-*s  %-*s  %-*s  %s" % (hw,h,tw,t,pw,pl,sw,st,fl))
'
            ;;
        point|target)
            # `target <label> <endpoint>` (0.45.7) = `point <label> --target <endpoint>`.
            local label="" target="" transfer=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --target) target="${2:-}"; shift 2 ;;
                    --transfer) transfer=1; shift ;;
                    --*) shift ;;
                    *)
                        if [ -z "$label" ]; then label="$1"
                        elif [ -z "$target" ]; then target="$1"
                        fi
                        shift ;;
                esac
            done
            if [ -z "$label" ] || [ -z "$target" ]; then
                echo "Usage: k2 publish subdomain point <label> --target <endpoint> [--transfer]" >&2
                echo "       k2 publish subdomain target <label> <endpoint>" >&2
                exit 1
            fi
            _refuse_reserved_nested_label "$label"
            _subdomain_label_preflight "$label" "$transfer" || exit 1
            local body
            body="$(python3 -c 'import json,sys; print(json.dumps({"target": sys.argv[1]}))' "$target")"
            cp_raw="$(_connect_curl PUT "/subdomains/$(urlencode "$label")" "$body")" || exit $?
            printf '%s' "$cp_raw" | _connect_subdomain_render '
host = d.get("host") or (str(d.get("label","?")) + ".k2.dev")
print("Repointed %s -> %s" % (host, d.get("target","?")))
print("  status:    %s" % d.get("status","?"))
print("  effective: %s" % d.get("effective","?"))
'
            # 0074 — a repoint through the K2 surface records the acting
            # workspace (self-only; a foreign label was refused by the
            # preflight). Then nudge the map refresh so the new target
            # shows immediately.
            _stamp_subdomain_attrib claim "$label" "$target" "$transfer"
            _nudge_subdomain_refresh
            _warn_if_local_target_down "$target"
            ;;
        rm|remove|delete)
            local label="${1:-}"
            shift 2>/dev/null || true
            if [ -z "$label" ]; then
                echo "Usage: k2 publish subdomain rm <label>" >&2
                exit 1
            fi
            # Never delete a name another workspace on this server owns.
            # (The reserved front-door label is box-level, not a workspace's.)
            if [ "$(printf '%s' "$label" | tr '[:upper:]' '[:lower:]')" != "skin" ]; then
                _subdomain_label_preflight "$label" 0 release || exit 1
            fi
            cp_raw="$(_connect_curl DELETE "/subdomains/$(urlencode "$label")")" || exit $?
            printf '%s' "$cp_raw" | _connect_subdomain_render '
print("Removed %s" % (d.get("removed") or "(subdomain)"))
'
            # 0074 — the label is gone from the control plane; drop its
            # workspace attribution row too, then nudge the map refresh
            # so the removed URL disappears from the UIs immediately.
            _stamp_subdomain_attrib unclaim "$label"
            _nudge_subdomain_refresh
            ;;
        *)
            echo "Usage: k2 publish subdomain <create|list|point|target|rm|claim|unclaim> ..." >&2
            echo "  create  <label> --target <endpoint>  Create a nested subdomain (attributed to this workspace)" >&2
            echo "  list                                 List your subdomains" >&2
            echo "  point   <label> --target <endpoint>  Repoint to a new endpoint" >&2
            echo "  target  <label> <endpoint>           Same as point (change where the label points)" >&2
            echo "  rm      <label>                      Remove a subdomain" >&2
            echo "  claim   <label>                      Adopt an existing label for THIS workspace (daemon-local)" >&2
            echo "  unclaim <label>                      Drop a label's workspace attribution (daemon-local)" >&2
            exit 1
            ;;
    esac
}

