# ── Headless Connect account pairing — k2 connect login|status|logout ─
# PRD: prd-headless-connect-onboarding-v1.md R1.
# Mirrors desktop src/renderer/components/Settings/lib/k2-account.ts:
# Supabase email/password (or --token JWT) → list purchased subdomains →
# bind tunnel token via POST /cli/tunnel/config → start tunnel.
# Account session lives in ~/.k2/connect-account.json (0600); password
# is never stored. Nested-subdomain Pro verbs stay under `k2 publish`.

# Public Supabase project values (same as the desktop client — anon role
# only; RLS scopes every row to the signed-in caller).
_K2_SUPABASE_URL="https://ttgcalfrzzgkxnfepkiu.supabase.co"
_K2_SUPABASE_ANON_KEY="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6InR0Z2NhbGZyenpna3huZmVwa2l1Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODA1MDIyMzksImV4cCI6MjA5NjA3ODIzOX0.L28xgtYkPEj5eCNDGO5Zf5xxhdKQLxKD8c1CJRHNqI8"

# 0.46.6: the account server address can be set; the default is unchanged.
# Same order as the daemon (k2_core::connect_db): K2_CONNECT_DB_URL, then
# the `connectDbUrl` setting in ~/.k2/settings.json, then the Supabase
# project above. Read here, not asked of the daemon, because `k2 connect
# login` signs in before a daemon has to be running. No automatic fallback
# between hosts. An invalid value is skipped with a warning (the daemon does
# the same). Prints the base URL; says which one on stderr when it is not
# the default. Never prints the anon key.
_connect_db_resolve() {
    K2_SETTINGS_FILE="${K2_HOME}/settings.json" DEFAULT_URL="$_K2_SUPABASE_URL" python3 -c '
import json, os, sys

def normalize(raw):
    t = raw.strip()
    if any(c.isspace() or ord(c) < 32 for c in t):
        return None, "must not contain spaces"
    if t.startswith("https://"):
        rest = t[len("https://"):]
    elif t.startswith("http://"):
        rest = t[len("http://"):]
        auth = rest.split("/")[0].split("?")[0].split("#")[0]
        host = auth[1:].split("]")[0] if auth.startswith("[") else auth.split(":")[0]
        if host not in ("127.0.0.1", "localhost", "::1"):
            return None, "must be an https:// URL (plain http:// only to localhost)"
    else:
        return None, "must be an https:// URL"
    auth = rest.split("/")[0].split("?")[0].split("#")[0]
    if not auth:
        return None, "has no host"
    if "@" in auth:
        return None, "must not carry a user name or password"
    if "?" in rest or "#" in rest:
        return None, "must not carry a query or fragment"
    return t.rstrip("/"), None

env = (os.environ.get("K2_CONNECT_DB_URL") or "").strip()
if env:
    url, err = normalize(env)
    if url:
        sys.stderr.write("k2 connect: account server %s (from K2_CONNECT_DB_URL)\n" % url)
        print(url); sys.exit(0)
    sys.stderr.write("k2 connect: K2_CONNECT_DB_URL ignored: %s\n" % err)
try:
    with open(os.environ["K2_SETTINGS_FILE"]) as f:
        setting = json.load(f).get("connectDbUrl") or ""
except Exception:
    setting = ""
if isinstance(setting, str) and setting.strip():
    url, err = normalize(setting)
    if url:
        sys.stderr.write("k2 connect: account server %s (from the connectDbUrl setting)\n" % url)
        print(url); sys.exit(0)
    sys.stderr.write("k2 connect: setting connectDbUrl ignored: %s\n" % err)
print(os.environ["DEFAULT_URL"])
'
}

# The resolved base for this `k2 connect` run (set once by cmd_connect).
_K2_CONNECT_DB_URL_RESOLVED=""
_K2_CONNECT_UA="k2-cli/${K2_CLI_VERSION:-unknown}"

_connect_account_file() { echo "${K2_HOME}/connect-account.json"; }

_connect_usage() {
    cat <<'EOF' >&2
%include cli/text/help/connect.txt
EOF
}

# Require a live local daemon for tunnel bind/start. Fail loud with a
# start-daemon hint — account login may already have succeeded.
_connect_require_daemon() {
    if [ -z "${PORT:-}" ] || [ -z "${TOKEN:-}" ]; then
        # Re-discover in case the gate was skipped and files appeared late.
        if [ -f "${K2_HOME}/heartbeat.port" ]; then
            PORT="${PORT:-$(cat "${K2_HOME}/heartbeat.port" 2>/dev/null)}"
        fi
        if [ -z "${PORT:-}" ] && [ -f "${K2_HOME}/daemon.port" ]; then
            PORT="$(cat "${K2_HOME}/daemon.port" 2>/dev/null)"
        fi
        if [ -z "${TOKEN:-}" ]; then
            # K2_NO_OWNER_FALLBACK with no session passport: refuse as the
            # connection guard does, never read the token files.
            if [ -n "${_k2_no_owner_fallback:-}" ]; then
                _k2_refuse_owner_fallback
            fi
            if [ -n "${DISK_OWNER_TOKEN:-}" ]; then
                TOKEN="$DISK_OWNER_TOKEN"
            elif [ -r "${K2_HOME}/heartbeat.token" ]; then
                DISK_OWNER_TOKEN="$(cat "${K2_HOME}/heartbeat.token" 2>/dev/null)"
                TOKEN="$DISK_OWNER_TOKEN"
            elif [ -r "${K2_HOME}/daemon.token" ]; then
                DISK_OWNER_TOKEN="$(cat "${K2_HOME}/daemon.token" 2>/dev/null)"
                TOKEN="$DISK_OWNER_TOKEN"
            fi
        fi
        _k2_daemon_base BASE_URL
    fi
    if [ -z "${PORT:-}" ] || [ -z "${TOKEN:-}" ]; then
        echo "Error: local K2 daemon is not reachable (need ~/.k2/heartbeat.port + token)." >&2
        echo "Start it first, then re-run: k2 connect login" >&2
        echo "  k2 daemon start     # or: k2 daemon install  (fresh headless box)" >&2
        return 1
    fi
    return 0
}

# Persist account session as 0600 JSON. Password is never written.
_connect_account_save() {
    local access="$1" refresh="$2" email="$3"
    local path
    path="$(_connect_account_file)"
    # The two tokens reach python on stdin, one per line, never argv or the
    # environment.
    printf '%s\n%s' "$access" "$refresh" | EMAIL="$email" PATH_OUT="$path" python3 -c '
import json, os, stat, sys
path = os.environ["PATH_OUT"]
access, _, refresh = sys.stdin.read().partition("\n")
data = {
    "access_token": access,
    "refresh_token": refresh,
    "email": os.environ.get("EMAIL") or "",
}
tmp = path + ".tmp"
with open(tmp, "w") as f:
    json.dump(data, f, separators=(",", ":"))
os.chmod(tmp, stat.S_IRUSR | stat.S_IWUSR)  # 0600
os.replace(tmp, path)
'
}

# Read the saved session into _CONNECT_ACC_ACCESS, _CONNECT_ACC_REFRESH
# and _CONNECT_ACC_EMAIL (all empty on a miss). Python prints the three
# fields in that order, each ended by a NUL byte, and the shell reads each
# by its place, so a newline inside one never moves another.
_connect_account_read() {
    _CONNECT_ACC_ACCESS="" _CONNECT_ACC_REFRESH="" _CONNECT_ACC_EMAIL=""
    { IFS= read -r -d '' _CONNECT_ACC_ACCESS &&
        IFS= read -r -d '' _CONNECT_ACC_REFRESH &&
        IFS= read -r -d '' _CONNECT_ACC_EMAIL; } < <(python3 -c '
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(0)
for key in ("access_token", "refresh_token", "email"):
    value = (d.get(key) or "").strip().replace("\0", "")
    sys.stdout.buffer.write(value.encode("utf-8", "replace") + b"\0")
' "$(_connect_account_file)" 2>/dev/null) || true
}

_connect_account_clear() {
    local path
    path="$(_connect_account_file)"
    [ -f "$path" ] && rm -f "$path"
    return 0
}

# Stable per-install device id for subdomain claim/lease. Prefer the
# value already stored in tunnel.json; else generate a UUID and keep it
# for the bind step (daemon persists it via /cli/tunnel/config).
_connect_device_id() {
    python3 -c '
import json, os, sys, uuid
path = os.path.join(os.environ.get("K2_HOME_FOR_DEVICE") or os.path.expanduser("~/.k2"), "tunnel.json")
try:
    d = json.load(open(path))
    did = (d.get("device_id") or d.get("deviceId") or "").strip()
    if did:
        print(did); raise SystemExit(0)
except SystemExit:
    raise
except Exception:
    pass
print(str(uuid.uuid4()))
' 2>/dev/null
}

_connect_device_label() {
    # Cosmetic holder label — hostname is good enough for headless.
    hostname 2>/dev/null || echo "k2-cli"
}

# The Connect account API is not the daemon: its Python calls pass an
# empty _K5_PY_HEADERS, so they carry no hand-over header (as
# _K2_CURL_NOT_DAEMON does for curl; see _k5_headers).

# Supabase password grant → access_token / refresh_token / email.
# Prints the three fields one-per-line on success; errors on stderr.
# The password reaches python on stdin, never argv or the environment (ps
# shows a command line to every account on this computer, and the environment
# to this user's processes).
_connect_supabase_sign_in() {
    local email="$1" password="$2"
    printf '%s' "$password" | EMAIL="$email" \
    SUPA_URL="${_K2_CONNECT_DB_URL_RESOLVED:-$_K2_SUPABASE_URL}" SUPA_KEY="$_K2_SUPABASE_ANON_KEY" K2_UA="$_K2_CONNECT_UA" _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="" python3 -c '
import json, os, sys, urllib.request, urllib.error
exec(os.environ.pop("_K5_PY_HTTP"))
url = os.environ["SUPA_URL"].rstrip("/") + "/auth/v1/token?grant_type=password"
body = json.dumps({"email": os.environ["EMAIL"], "password": sys.stdin.read()}).encode()
req = urllib.request.Request(url, data=body, method="POST", headers={
    "apikey": os.environ["SUPA_KEY"],
    "User-Agent": os.environ.get("K2_UA") or "k2-cli",
    "Content-Type": "application/json",
})
try:
    with urllib.request.urlopen(k5_request(req), timeout=30) as resp:
        raw = resp.read().decode()
        status = resp.status
except urllib.error.HTTPError as e:
    raw = e.read().decode() if e.fp else ""
    status = e.code
except Exception as e:
    sys.stderr.write("Error: could not reach k2.dev account service: %s\n" % e)
    sys.exit(1)
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    d = {}
if status < 200 or status >= 300 or not d.get("access_token"):
    msg = (
        d.get("error_description") or d.get("msg") or d.get("message")
        or (d.get("error") if isinstance(d.get("error"), str) else None)
        or ("Request failed (%s)" % status)
    )
    sys.stderr.write("Error: sign-in failed: %s\n" % msg)
    sys.exit(1)
user = d.get("user") or {}
email = (user.get("email") if isinstance(user, dict) else None) or os.environ["EMAIL"]
print(d["access_token"])
print(d.get("refresh_token") or "")
print(email)
'
}

# List purchased subdomains for the access token. Prints JSON array.
_connect_list_subdomains() {
    local access="$1"
    # The access token reaches python on stdin, never argv or the environment.
    printf '%s' "$access" | SUPA_URL="${_K2_CONNECT_DB_URL_RESOLVED:-$_K2_SUPABASE_URL}" SUPA_KEY="$_K2_SUPABASE_ANON_KEY" K2_UA="$_K2_CONNECT_UA" _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="" python3 -c '
import json, os, sys, urllib.request, urllib.error
exec(os.environ.pop("_K5_PY_HTTP"))
access = sys.stdin.read()
url = (
    os.environ["SUPA_URL"].rstrip("/")
    + "/rest/v1/subdomains?select=label,status,tunnel_token,claimed_by,claimed_at,claimed_label"
)
req = urllib.request.Request(url, method="GET", headers={
    "apikey": os.environ["SUPA_KEY"],
    "User-Agent": os.environ.get("K2_UA") or "k2-cli",
    "Authorization": "Bearer " + access,
})
try:
    with urllib.request.urlopen(k5_request(req), timeout=30) as resp:
        raw = resp.read().decode()
        status = resp.status
except urllib.error.HTTPError as e:
    raw = e.read().decode() if e.fp else ""
    status = e.code
except Exception as e:
    sys.stderr.write("Error: could not list subdomains: %s\n" % e)
    sys.exit(1)
if status < 200 or status >= 300:
    try:
        d = json.loads(raw) if raw.strip() else {}
    except Exception:
        d = {}
    msg = d.get("message") or d.get("msg") or d.get("error_description") or raw[:200] or ("HTTP %s" % status)
    sys.stderr.write("Error: list subdomains failed: %s\n" % msg)
    sys.exit(1)
try:
    rows = json.loads(raw) if raw.strip() else []
except Exception:
    rows = []
if not isinstance(rows, list):
    rows = []
print(json.dumps(rows))
'
}

# Best-effort claim_subdomain RPC (desktop parity). Non-zero only on
# hard "held by another device"; soft failures warn and continue.
_connect_claim_subdomain() {
    local access="$1" label="$2" device_id="$3" device_label="$4"
    # The access token reaches python on stdin, never argv or the environment.
    printf '%s' "$access" | LABEL="$label" DID="$device_id" DLBL="$device_label" \
    SUPA_URL="${_K2_CONNECT_DB_URL_RESOLVED:-$_K2_SUPABASE_URL}" SUPA_KEY="$_K2_SUPABASE_ANON_KEY" K2_UA="$_K2_CONNECT_UA" _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="" python3 -c '
import json, os, sys, urllib.request, urllib.error
exec(os.environ.pop("_K5_PY_HTTP"))
access = sys.stdin.read()
url = os.environ["SUPA_URL"].rstrip("/") + "/rest/v1/rpc/claim_subdomain"
body = json.dumps({
    "p_label": os.environ["LABEL"],
    "p_device_id": os.environ["DID"],
    "p_device_label": os.environ.get("DLBL") or None,
}).encode()
req = urllib.request.Request(url, data=body, method="POST", headers={
    "apikey": os.environ["SUPA_KEY"],
    "User-Agent": os.environ.get("K2_UA") or "k2-cli",
    "Authorization": "Bearer " + access,
    "Content-Type": "application/json",
})
try:
    with urllib.request.urlopen(k5_request(req), timeout=30) as resp:
        raw = resp.read().decode()
        status = resp.status
except urllib.error.HTTPError as e:
    raw = e.read().decode() if e.fp else ""
    status = e.code
except Exception as e:
    sys.stderr.write("Warning: claim_subdomain failed (%s) — continuing bind.\n" % e)
    sys.exit(0)
if status < 200 or status >= 300:
    sys.stderr.write("Warning: claim_subdomain HTTP %s — continuing bind.\n" % status)
    sys.exit(0)
try:
    rows = json.loads(raw) if raw.strip() else []
except Exception:
    rows = []
row = rows[0] if isinstance(rows, list) and rows else (rows if isinstance(rows, dict) else {})
if not isinstance(row, dict):
    sys.exit(0)
if row.get("claimed"):
    sys.exit(0)
holder = row.get("holder")
if holder and holder != os.environ["DID"]:
    label = os.environ["LABEL"]
    hl = row.get("holder_label") or holder
    sys.stderr.write(
        "Error: %s.k2.dev is in use on another device (%s) — stop it there first.\n" % (label, hl)
    )
    sys.exit(1)
'
}

cmd_connect_login() {
    local email="" token="" subdomain="" password="" token_stdin=0 token_flag=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --email)
                email="${2:-}"; shift 2 || { echo "Error: --email needs a value" >&2; return 2; } ;;
            --email=*)
                email="${1#--email=}"; shift ;;
            --token-stdin)
                token_stdin=1; shift ;;
            --token)
                token="${2:-}"; token_flag="--token"; shift 2 || { echo "Error: --token needs a value" >&2; return 2; } ;;
            --token=*)
                token="${1#--token=}"; token_flag="--token"; shift ;;
            --subdomain)
                subdomain="${2:-}"; shift 2 || { echo "Error: --subdomain needs a value" >&2; return 2; } ;;
            --subdomain=*)
                subdomain="${1#--subdomain=}"; shift ;;
            --password|--pass)
                echo "Error: passwords must not be passed on the command line." >&2
                echo "Use the interactive prompt, or --token-stdin for scripting." >&2
                return 2
                ;;
            --help|-h)
                _connect_usage; return 0 ;;
            -*)
                echo "Error: unknown flag '$1'" >&2
                _connect_usage
                return 2
                ;;
            *)
                echo "Error: unexpected argument '$1'" >&2
                _connect_usage
                return 2
                ;;
        esac
    done

    _k2_refuse_if_airgap

    # The access JWT off argv: --token-stdin reads one line from stdin.
    # --token <jwt> still works for now, with a note: a command line is
    # visible to every account on this computer while the command runs.
    if [ "$token_stdin" = 1 ]; then
        [ -z "$token_flag" ] || { echo "Error: --token-stdin or --token, not both" >&2; return 2; }
        _cli_read_secret_stdin "Access JWT: "
        token="${_K2_SECRET%$'\r'}"; _K2_SECRET=""
        [ -n "$token" ] || { echo "Error: --token-stdin: no token on stdin" >&2; return 2; }
    elif [ -n "$token_flag" ]; then
        echo "note: --token is visible to every account on this computer while the command runs; use --token-stdin" >&2
    fi

    local access_token="" refresh_token="" account_email=""
    if [ -n "$token" ]; then
        access_token="$token"
        account_email="${email}"
        refresh_token=""
        # Optional: if we already have a saved session for display, ignore.
        if [ -z "$account_email" ]; then
            _connect_account_read
            account_email="$_CONNECT_ACC_EMAIL"
        fi
        [ -z "$account_email" ] && account_email="(token)"
    else
        if [ -z "$email" ]; then
            printf "k2.dev email: " >&2
            IFS= read -r email || true
        fi
        if [ -z "$email" ]; then
            echo "Error: email is required (or pass --token-stdin with an access JWT)." >&2
            return 2
        fi
        password="$(python3 -c '
import getpass, sys
try:
    pw = getpass.getpass("k2.dev password: ")
except (EOFError, KeyboardInterrupt):
    sys.stderr.write("\nError: password prompt cancelled.\n")
    sys.exit(1)
if not pw:
    sys.stderr.write("Error: password must not be empty.\n")
    sys.exit(1)
sys.stdout.write(pw)
')" || return 1
        local sign_out
        sign_out="$(_connect_supabase_sign_in "$email" "$password")" || {
            unset password
            return 1
        }
        unset password
        access_token="$(printf '%s\n' "$sign_out" | sed -n '1p')"
        refresh_token="$(printf '%s\n' "$sign_out" | sed -n '2p')"
        account_email="$(printf '%s\n' "$sign_out" | sed -n '3p')"
        [ -z "$account_email" ] && account_email="$email"
    fi

    # Persist account session BEFORE daemon steps so a later re-run can
    # skip re-typing credentials once the daemon is up.
    _connect_account_save "$access_token" "$refresh_token" "$account_email"
    echo "Signed in as ${account_email}." >&2

    local subs_json
    subs_json="$(_connect_list_subdomains "$access_token")" || return 1

    # Pick subdomain (interactive numbered list, or --subdomain).
    # The picker sees the rows without their tunnel tokens: SUBS_JSON is in
    # its environment, and a token never is (ps shows a command line to every
    # account on this computer, and the environment to this user's processes).
    # The chosen row's token is read on stdin below.
    local pick_json subs_public
    subs_public="$(printf '%s' "$subs_json" | python3 -c '
import json, sys
rows = json.loads(sys.stdin.read() or "[]")
if isinstance(rows, list):
    rows = [{k: v for k, v in r.items() if k != "tunnel_token"} if isinstance(r, dict) else r for r in rows]
print(json.dumps(rows))')" || return 1
    pick_json=$(SUBS_JSON="$subs_public" WANT="$subdomain" python3 -c '
import json, os, sys
rows = json.loads(os.environ.get("SUBS_JSON") or "[]")
want = (os.environ.get("WANT") or "").strip().lower()
if not rows:
    sys.stderr.write(
        "Error: this account has no purchased subdomains.\n"
        "Buy one at https://k2.dev/dashboard then re-run: k2 connect login\n"
    )
    sys.exit(1)
# Normalize
norm = []
for r in rows:
    if not isinstance(r, dict):
        continue
    label = (r.get("label") or "").strip()
    if not label:
        continue
    norm.append(r)
if not norm:
    sys.stderr.write(
        "Error: this account has no purchased subdomains.\n"
        "Buy one at https://k2.dev/dashboard then re-run: k2 connect login\n"
    )
    sys.exit(1)
chosen = None
if want:
    for r in norm:
        if (r.get("label") or "").strip().lower() == want:
            chosen = r; break
    if chosen is None:
        sys.stderr.write("Error: subdomain %r is not in your account.\n" % want)
        sys.stderr.write("Available: %s\n" % ", ".join((r.get("label") or "?") for r in norm))
        sys.exit(1)
else:
    if len(norm) == 1:
        chosen = norm[0]
        sys.stderr.write("Using only subdomain: %s.k2.dev\n" % (chosen.get("label") or "?"))
    else:
        sys.stderr.write("Your subdomains:\n")
        for i, r in enumerate(norm, 1):
            st = (r.get("status") or "").strip()
            extra = (" (%s)" % st) if st and st != "active" else ""
            sys.stderr.write("  %d) %s.k2.dev%s\n" % (i, r.get("label") or "?", extra))
        sys.stderr.write("Pick a number [1-%d]: " % len(norm))
        sys.stderr.flush()
        try:
            line = sys.stdin.readline()
        except Exception:
            line = ""
        try:
            n = int((line or "").strip())
        except Exception:
            sys.stderr.write("Error: invalid selection.\n"); sys.exit(1)
        if n < 1 or n > len(norm):
            sys.stderr.write("Error: selection out of range.\n"); sys.exit(1)
        chosen = norm[n - 1]
label = (chosen.get("label") or "").strip()
if not label:
    sys.stderr.write("Error: selected subdomain is missing label or tunnel_token.\n")
    sys.exit(1)
print(json.dumps({"label": label, "status": chosen.get("status") or ""}))
') || return 1

    local label tunnel_token
    label=$(printf '%s' "$pick_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["label"])')
    tunnel_token=$(printf '%s' "$subs_json" | WANT_LABEL="$label" python3 -c '
import json, os, sys
rows = json.loads(sys.stdin.read() or "[]")
want = os.environ["WANT_LABEL"]
tok = next(((r.get("tunnel_token") or "").strip() for r in (rows if isinstance(rows, list) else [])
            if isinstance(r, dict) and (r.get("label") or "").strip() == want), "")
if not tok:
    sys.stderr.write("Error: selected subdomain is missing label or tunnel_token.\n")
    sys.exit(1)
print(tok)') || return 1

    # Daemon required for bind + start.
    _connect_require_daemon || return 1
    export K2_HOME_FOR_DEVICE="$K2_HOME"
    local device_id device_label
    device_id="$(_connect_device_id)"
    device_label="$(_connect_device_label)"

    # Best-effort claim (nice-to-have); hard-fail only if held by other device.
    _connect_claim_subdomain "$access_token" "$label" "$device_id" "$device_label" || return 1

    local cfg_body cfg_resp
    # The tunnel token reaches python on stdin, never argv or the environment.
    cfg_body=$(printf '%s' "$tunnel_token" | LABEL="$label" DID="$device_id" DLBL="$device_label" python3 -c '
import json, os, sys
print(json.dumps({
    "subdomain": os.environ["LABEL"],
    "token": sys.stdin.read(),
    "deviceId": os.environ["DID"],
    "deviceLabel": os.environ.get("DLBL") or "",
    "autoStart": True,
}))
')
    # Owner-auth via TOKEN (external: disk owner; in-cell: passport — ID tool).
    cfg_resp=$(cli_send cli_post_json "/cli/tunnel/config" "$cfg_body") || cfg_resp=""
    printf '%s' "$cfg_resp" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    sys.stderr.write("Error: tunnel/config returned unparseable response: %r\n" % (raw[:200],))
    sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err is not None:
    hint = err.get("hint") if isinstance(err, dict) else err
    sys.stderr.write("Error binding tunnel config: %s\n" % hint)
    sys.exit(1)
if not isinstance(d, dict):
    sys.stderr.write("Error: tunnel/config failed: %s\n" % (raw[:200] or "empty"))
    sys.exit(1)
# success = redacted view with tokenSet / subdomain
if d.get("tokenSet") is False and not d.get("subdomain"):
    sys.stderr.write("Error: tunnel/config did not accept the token.\n")
    sys.exit(1)
' || return 1

    local start_resp
    # The tunnel is live only when the daemon says so: no answer (curl
    # refused, timed out or got an empty reply), an answer that isn't JSON,
    # one that isn't an object and an error answer each end with exit 1,
    # never the live line.
    start_resp=$(cli_send cli_post "/cli/tunnel/start" "subdomain=$(urlencode "$label")") || start_resp=""
    printf '%s' "$start_resp" | LABEL="$label" python3 -c '
import json, os, sys
raw = sys.stdin.read()
label = os.environ.get("LABEL") or "?"
if not raw.strip():
    sys.stderr.write("Error starting tunnel: no answer from the K2 daemon on /cli/tunnel/start; check with: k2 connect status\n")
    sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    sys.stderr.write("Error: tunnel/start returned unparseable response: %r\n" % (raw[:200],))
    sys.exit(1)
if not isinstance(d, dict):
    sys.stderr.write("Error: tunnel/start failed: %s\n" % raw[:200])
    sys.exit(1)
err = d.get("error")
if err is not None:
    hint = err.get("hint") if isinstance(err, dict) else err
    sys.stderr.write("Error starting tunnel: %s\n" % hint)
    sys.exit(1)
url = d.get("public_url") or ("https://%s.k2.dev" % label)
print("✓ live at %s" % url)
' || return 1
}

cmd_connect_status() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --help|-h) _connect_usage; return 0 ;;
            *) echo "Usage: k2 connect status" >&2; return 2 ;;
        esac
    done
    local acc_email="" acc_present="no"
    if [ -f "$(_connect_account_file)" ]; then
        acc_present="yes"
        _connect_account_read
        acc_email="$_CONNECT_ACC_EMAIL"
    fi
    local sub="" token_set="no"
    if [ -f "${K2_HOME}/tunnel.json" ]; then
        # The file's fields are data: Python prints each one ended by a NUL
        # byte and the shell reads them as text, so nothing in them runs.
        # A file that can't be read leaves both as they are.
        { IFS= read -r -d '' sub && IFS= read -r -d '' token_set; } < <(python3 -c '
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    d = {}
sub = (d.get("subdomain") or "").strip().replace("\0", "")
tok = (d.get("token") or "").strip()
out = "%s\0%s\0" % (sub, "yes" if tok else "no")
sys.stdout.buffer.write(out.encode("utf-8", "replace"))
' "${K2_HOME}/tunnel.json" 2>/dev/null) || { sub=""; token_set="no"; }
    fi
    # 0.45.7: a desktop keeps the token in the K2 vault (tunnel.json's is
    # empty since 0.45.4) — ask the daemon, which reads both.
    if [ "$token_set" = "no" ] && _connect_require_daemon 2>/dev/null; then
        token_set="$(cli_send cli_request "/cli/tunnel/config" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception:
    d = {}
if isinstance(d, dict) and d.get("tokenSet"):
    print("yes")
elif isinstance(d, dict) and d.get("tokenUnreadable"):
    print("unknown (the K2 vault could not be read)")
else:
    print("no")
' 2>/dev/null)" || token_set="no"
        # That call ran in a subshell: a passport it healed is lost there.
        # Take it up here so the status call below sends it.
        _cli_use_healed_passport || true
    fi
    echo "K2 Connect account"
    echo "  Account session: $acc_present$([ -n "$acc_email" ] && echo "  ($acc_email)")"
    if [ -n "$sub" ]; then
        printf '  Paired subdomain: %s → https://%s.k2.dev\n' "$sub" "$sub"
    else
        printf '%s\n' "  Paired subdomain: (none)"
    fi
    echo "  Tunnel token:     $token_set"

    # Tunnel live status when the daemon is up.
    if _connect_require_daemon 2>/dev/null; then
        local raw
        raw=$(cli_send cli_request "/cli/tunnel/status") || raw=""
        printf '%s' "$raw" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    print("  Tunnel status:   (unparseable daemon response)")
    raise SystemExit(0)
if not isinstance(d, dict) or d.get("error"):
    err = d.get("error") if isinstance(d, dict) else None
    print("  Tunnel status:   (error: %s)" % (err or "unknown"))
    raise SystemExit(0)
running = "yes" if d.get("running") else "no"
print("  Tunnel running:  %s" % running)
if d.get("running"):
    print("  Public URL:      %s" % (d.get("public_url") or "(assigned by server)"))
elif d.get("released"):
    print("  State:           RELEASED — re-pair with: k2 connect login")
elif d.get("enabled") is False:
    print("  State:           DISABLED — re-enable with: k2 tunnel enable")
else:
    print("  State:           stopped")
' 2>/dev/null || echo "  Tunnel status:   (daemon unreachable)"
    else
        echo "  Tunnel status:   (daemon not running — start with: k2 daemon start)"
    fi
}

cmd_connect_logout() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --help|-h) _connect_usage; return 0 ;;
            *) echo "Usage: k2 connect logout" >&2; return 2 ;;
        esac
    done
    # Best-effort server-side logout when we still have an access token.
    local access
    _connect_account_read
    access="$_CONNECT_ACC_ACCESS"
    if [ -n "$access" ]; then
        # The access token reaches python on stdin, never argv or the
        # environment.
        printf '%s' "$access" | SUPA_URL="${_K2_CONNECT_DB_URL_RESOLVED:-$_K2_SUPABASE_URL}" SUPA_KEY="$_K2_SUPABASE_ANON_KEY" K2_UA="$_K2_CONNECT_UA" _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="" python3 -c '
import os, sys, urllib.request
exec(os.environ.pop("_K5_PY_HTTP"))
access = sys.stdin.read()
url = os.environ["SUPA_URL"].rstrip("/") + "/auth/v1/logout"
req = urllib.request.Request(url, data=b"", method="POST", headers={
    "apikey": os.environ["SUPA_KEY"],
    "User-Agent": os.environ.get("K2_UA") or "k2-cli",
    "Authorization": "Bearer " + access,
})
try:
    urllib.request.urlopen(k5_request(req), timeout=10)
except Exception:
    pass
' 2>/dev/null || true
    fi
    _connect_account_clear
    echo "Logged out of k2.dev account (cleared ~/.k2/connect-account.json)."
    echo "Tunnel pairing is unchanged. To unpair this device: k2 tunnel release --confirm"
}

cmd_connect() {
    local sub="${1:-}"
    shift 2>/dev/null || true
    case "$sub" in
        login|logout)
            case " $* " in *" --help "*|*" -h "*) ;; *)
                _K2_CONNECT_DB_URL_RESOLVED="$(_connect_db_resolve)" || return 1 ;;
            esac
            ;;
    esac
    case "$sub" in
        login)
            cmd_connect_login "$@"
            ;;
        status)
            cmd_connect_status "$@"
            ;;
        logout)
            cmd_connect_logout "$@"
            ;;
        ""|help|--help|-h)
            _connect_usage
            # Bare `k2 connect` without a subcommand is usage, exit 2.
            [ -n "$sub" ] && [ "$sub" != "" ] && return 0
            # help/-h exit 0; bare empty exits 2 so scripts detect missing subcommand.
            if [ -z "$sub" ]; then return 2; fi
            return 0
            ;;
        # Former `k2 connect` → publish surface (0.40.41). Point people over.
        subdomain|publish)
            echo "Nested subdomains moved to: k2 publish ${sub}${*:+ $*}" >&2
            echo "  e.g. k2 publish subdomain list" >&2
            echo "  e.g. k2 publish status" >&2
            echo "Account pairing is now: k2 connect login | status | logout" >&2
            return 2
            ;;
        *)
            # Heuristic: anything that looks like an old publish verb.
            case "$sub" in
                create|list|point|rm|claim|unclaim)
                    echo "Unknown 'k2 connect' subcommand: $sub" >&2
                    echo "Did you mean: k2 publish subdomain $sub … ?" >&2
                    echo "Account pairing: k2 connect login | status | logout" >&2
                    return 2
                    ;;
            esac
            echo "Unknown 'k2 connect' subcommand: $sub" >&2
            _connect_usage
            return 2
            ;;
    esac
}

