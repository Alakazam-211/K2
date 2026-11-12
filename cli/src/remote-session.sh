# ── Remote Session (owner/admin + grant-token drive) ──────────────────
# Stages 1–3 daemon: Layer 0 toggle, grants (k2rs_…), shell PTY + I/O.
# Routes live under /cli/remote-session/* and terminal write/read with
# grant-token support. Owner/admin for status/enable/disable/grant/revoke;
# shell/write/read accept a grant token via --token k2rs_….

# Parse TTL strings into integer seconds.
# Accepts: bare seconds (1800), 30m, 45m, 1h, 90s. Echoes seconds or
# returns non-zero on invalid input.
_rs_parse_ttl() {
    local raw="${1:-}"
    if [ -z "$raw" ]; then
        echo "1800"
        return 0
    fi
    case "$raw" in
        *[!0-9smhSMH]*) return 1 ;;
    esac
    local lower
    lower=$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]')
    case "$lower" in
        *h)
            local n="${lower%h}"
            [[ "$n" =~ ^[0-9]+$ ]] || return 1
            echo $((10#$n * 3600))
            ;;
        *m)
            local n="${lower%m}"
            [[ "$n" =~ ^[0-9]+$ ]] || return 1
            echo $((10#$n * 60))
            ;;
        *s)
            local n="${lower%s}"
            [[ "$n" =~ ^[0-9]+$ ]] || return 1
            echo $((10#$n))
            ;;
        *)
            [[ "$lower" =~ ^[0-9]+$ ]] || return 1
            echo $((10#$lower))
            ;;
    esac
}

# Shared error printer for remote-session daemon responses.
# stdin: raw body. Maps nested {error:{code,hint}} + string errors.
# Exit 3 for teaching/auth codes (REMOTE_SESSIONS_DISABLED, NO_GRANT,
# GRANT_*, owner_only, forbidden); 2 for usage/bad request; 1 otherwise.
_rs_print_err() {
    python3 -c '
import json, sys
raw = sys.stdin.read().strip()
if not raw:
    print("Error: empty response from daemon (is it running?)", file=sys.stderr)
    sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    print("Error: " + raw[:200], file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict):
    print("Error: unexpected response: %r" % (raw[:200],), file=sys.stderr)
    sys.exit(1)
err = d.get("error")
if err is None and d.get("ok") is False:
    err = d
if isinstance(err, dict):
    code = (err.get("code") or "").strip()
    hint = (err.get("hint") or err.get("message") or "").strip()
    if not code and not hint:
        # Flat string-shaped error already handled below; treat unknown dict.
        print("Error: %s" % (err,), file=sys.stderr)
        sys.exit(1)
    teach = {
        "REMOTE_SESSIONS_DISABLED": "Remote Sessions are OFF. Enable with: k2 remote-session enable",
        "NO_GRANT": "No live grant covers this principal. Mint one: k2 remote-session grant [--ttl 30m] [--label \"...\"]",
        "GRANT_EXPIRED": "Grant expired. Mint a new one: k2 remote-session grant [--ttl 30m]",
        "GRANT_REVOKED": "Grant was revoked. Mint a new one: k2 remote-session grant [--ttl 30m]",
        "owner_only": "requires owner/admin — ask your human",
        "forbidden": "requires owner/admin — ask your human",
        "gated": "requires owner/admin — ask your human",
    }
    if code in teach:
        # Prefer daemon hint when present; append teach line when distinct.
        base = hint or teach[code]
        if hint and teach[code] not in hint:
            print("Error: %s" % base, file=sys.stderr)
            print("  → %s" % teach[code], file=sys.stderr)
        else:
            print("Error: %s" % base, file=sys.stderr)
        if code == "REMOTE_SESSIONS_DISABLED" and "enable" not in (hint or "").lower():
            print("  → k2 remote-session enable", file=sys.stderr)
        sys.exit(3)
    # Classic forbidden / invalid token string often lands as plain str below.
    if code in ("owner_only", "forbidden", "gated"):
        print("Error: %s" % (hint or code), file=sys.stderr)
        sys.exit(3)
    msg = hint or code or str(err)
    print("Error: %s" % msg, file=sys.stderr)
    sys.exit(2 if code in ("usage", "bad_request") else 1)
if isinstance(err, str):
    low = err.lower()
    print("Error: %s" % err, file=sys.stderr)
    if "token" in low or "forbidden" in low or "owner" in low:
        print("  → requires owner/admin token (or a live k2rs_ grant for shell/write/read)", file=sys.stderr)
        sys.exit(3)
    sys.exit(1)
# Success-shaped body that caller should not have routed here.
print("Error: unexpected response: %s" % (raw[:200],), file=sys.stderr)
sys.exit(1)
'
}

cmd_help_remote_session() {
    cat <<'EOF'
k2 remote-session — owner/admin Remote Session surface (Layer 0 + grants + shell)

Master switch defaults OFF. Drive (shell/write/read) is denied until enable.
Grants mint a one-time k2rs_… token; shell PTYs and terminal I/O require a
live grant (or owner break-glass on write/read).

USAGE
  k2 remote-session status [--json] [--host HOST] [--token TOK]
  k2 remote-session enable  [--host HOST] [--token TOK]
  k2 remote-session disable [--host HOST] [--token TOK]
  k2 remote-session grant [--scope shell] [--ttl 30m|1800] [--label "..."]
                          [--host HOST] [--token TOK]
  k2 remote-session grants [--json] [--host HOST] [--token TOK]
  k2 remote-session revoke <grant-id> [--host HOST] [--token TOK]
  k2 remote-session shell [--cwd PATH] [--host HOST] [--token k2rs_…]
  k2 remote-session write <session-id> "text" [--token k2rs_…] [--host HOST]
  k2 remote-session read  <session-id> [--lines N] [--token k2rs_…] [--host HOST]

ADMIN (owner/admin token)
  status     Layer 0 enabled?, active grants/sessions, recent denials
  enable     Turn Remote Sessions ON
  disable    Turn OFF and kill grant-bound shells
  grant      Mint shell grant; prints token ONCE
  grants     List grants (no secrets)
  revoke     Revoke by id (kills bound shells)

DRIVE (prefer --token k2rs_… grant)
  shell      Spawn daemon-user login shell PTY under the grant
  write      Inject text into a remote-session PTY (terminal/write)
  read       Peek last N lines of a remote-session PTY (terminal/read)

OPTIONS
  --scope shell     Only scope implemented (default shell)
  --ttl <dur>       Grant lifetime: 30m, 45m, 1h, 90s, or bare seconds (1800)
  --label "..."     Optional human label on the grant
  --cwd PATH        Working directory for shell spawn (default: daemon HOME)
  --lines N         Lines for read (default 50)
  --host HOST       Remote daemon (https://HOST or full URL). Reuses
                    connect-tokens / --token / $K2_REMOTE_TOKEN like k2 talk.
  --token TOK       Owner/connect token (admin) or k2rs_… grant (drive)
  --json            Machine-readable status / grants

EXAMPLES
  k2 remote-session enable
  k2 remote-session grant --ttl 30m --label "lab overnight"
  k2 remote-session shell --token k2rs_…
  k2 remote-session write <session-id> "ls -la" --token k2rs_…
  k2 remote-session read  <session-id> --lines 80 --token k2rs_…
  k2 remote-session revoke rs_…
  k2 remote-session status --host box.k2.dev   # owner token from connect-tokens

EXIT CODES
  0  ok
  1  unexpected / transport
  2  usage / bad request
  3  teaching auth: REMOTE_SESSIONS_DISABLED, NO_GRANT, GRANT_*, owner_only
EOF
}

# Apply --host / --token for a remote-session subcommand.
# Grant tokens (k2rs_) set TOKEN even without --host (local drive).
# With --host, reuses _apply_remote_host (connect-tokens / K2_REMOTE_TOKEN).
_rs_apply_target() {
    local host="${1:-}" token="${2:-}"
    if [ -n "$host" ]; then
        _apply_remote_host "$host" "$token" || return 1
        return 0
    fi
    if [ -n "$token" ]; then
        TOKEN="$token"
    fi
    return 0
}

cmd_remote_session() {
    local sub="${1:-}"
    if [ -z "$sub" ] || [ "$sub" = "--help" ] || [ "$sub" = "-h" ] || [ "$sub" = "help" ]; then
        cmd_help_remote_session
        return 0
    fi
    shift || true

    case "$sub" in
        status)
            local host="" token="" json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --host)  host="${2:-}"; shift 2 ;;
                    --token) token="${2:-}"; shift 2 ;;
                    --json)  json=1; shift ;;
                    --help|-h) cmd_help_remote_session; return 0 ;;
                    *) echo "Unknown flag: $1" >&2; cmd_help_remote_session >&2; return 2 ;;
                esac
            done
            _rs_apply_target "$host" "$token" || return 1
            local raw
            raw=$(cli_request "/cli/remote-session/status") || raw=""
            if printf '%s' "$raw" | python3 -c '
import json,sys
d=json.loads(sys.stdin.read() or "{}")
sys.exit(0 if isinstance(d,dict) and d.get("ok") is True else 1)
' 2>/dev/null; then
                if [ "$json" = "1" ]; then
                    printf '%s' "$raw" | python3 -m json.tool 2>/dev/null || printf '%s\n' "$raw"
                else
                    printf '%s' "$raw" | python3 -c '
import json,sys,datetime
d=json.loads(sys.stdin.read())
print("Remote Sessions: %s" % ("ON" if d.get("enabled") else "OFF"))
print("Active grants:   %s" % d.get("activeGrants", 0))
sessions = d.get("activeSessions") or []
print("Active shells:   %s" % len(sessions))
for s in sessions:
    if isinstance(s, dict):
        print("  session=%s  grant=%s" % (s.get("sessionId","?"), s.get("grantId","?")))
denials = d.get("recentDenials") or []
if denials:
    print("Recent denials (%d):" % len(denials))
    for ev in denials[:10]:
        if not isinstance(ev, dict):
            continue
        ts = ev.get("createdAt") or 0
        try:
            tstr = datetime.datetime.utcfromtimestamp(int(ts)).strftime("%Y-%m-%d %H:%M:%SZ")
        except Exception:
            tstr = str(ts)
        print("  [%s] %s  %s  %s" % (
            tstr,
            ev.get("code") or "-",
            ev.get("principalLabel") or "?",
            (ev.get("payload") or "")[:80],
        ))
else:
    print("Recent denials:  none")
'
                fi
                return 0
            fi
            printf '%s' "$raw" | _rs_print_err
            return $?
            ;;

        enable|disable)
            local host="" token=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --host)  host="${2:-}"; shift 2 ;;
                    --token) token="${2:-}"; shift 2 ;;
                    --help|-h) cmd_help_remote_session; return 0 ;;
                    *) echo "Unknown flag: $1" >&2; return 2 ;;
                esac
            done
            _rs_apply_target "$host" "$token" || return 1
            local raw
            raw=$(cli_post_json "/cli/remote-session/${sub}" "{}") || raw=""
            if printf '%s' "$raw" | python3 -c '
import json,sys
d=json.loads(sys.stdin.read() or "{}")
sys.exit(0 if isinstance(d,dict) and d.get("ok") is True else 1)
' 2>/dev/null; then
                printf '%s' "$raw" | python3 -c '
import json,sys
d=json.loads(sys.stdin.read())
print("Remote Sessions: %s" % ("ON" if d.get("enabled") else "OFF"))
killed = d.get("killedSessions")
if killed is not None:
    print("Killed sessions: %s" % killed)
'
                return 0
            fi
            printf '%s' "$raw" | _rs_print_err
            return $?
            ;;

        grant)
            local host="" token="" scope="shell" ttl_raw="30m" label=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --host)  host="${2:-}"; shift 2 ;;
                    --token) token="${2:-}"; shift 2 ;;
                    --scope) scope="${2:-}"; shift 2 ;;
                    --ttl)   ttl_raw="${2:-}"; shift 2 ;;
                    --label) label="${2:-}"; shift 2 ;;
                    --help|-h) cmd_help_remote_session; return 0 ;;
                    *) echo "Unknown flag: $1" >&2; return 2 ;;
                esac
            done
            local ttl_secs
            if ! ttl_secs=$(_rs_parse_ttl "$ttl_raw"); then
                echo "Error: invalid --ttl '$ttl_raw' (use 30m, 1h, 90s, or bare seconds)" >&2
                return 2
            fi
            _rs_apply_target "$host" "$token" || return 1
            local body raw
            body=$(SCOPE="$scope" TTL="$ttl_secs" LABEL="$label" python3 -c '
import json,os
b={"scope": os.environ.get("SCOPE") or "shell", "ttlSeconds": int(os.environ["TTL"])}
lab=(os.environ.get("LABEL") or "").strip()
if lab: b["label"]=lab
print(json.dumps(b))
')
            raw=$(cli_post_json "/cli/remote-session/grant" "$body") || raw=""
            if printf '%s' "$raw" | python3 -c '
import json,sys
d=json.loads(sys.stdin.read() or "{}")
sys.exit(0 if isinstance(d,dict) and d.get("ok") is True else 1)
' 2>/dev/null; then
                printf '%s' "$raw" | python3 -c '
import json,sys,datetime
d=json.loads(sys.stdin.read())
g = d.get("grant") or {}
tok = d.get("token") or ""
gid = g.get("id") or "?"
exp = g.get("expiresAt") or g.get("expires_at")
try:
    exp_s = datetime.datetime.utcfromtimestamp(int(exp)).strftime("%Y-%m-%d %H:%M:%SZ") if exp else "?"
except Exception:
    exp_s = str(exp)
print("Grant id: %s" % gid)
print("Token (save now, shown once): %s" % tok)
print("Expires: %s" % exp_s)
if g.get("label"):
    print("Label: %s" % g.get("label"))
print("Scope: %s" % (g.get("scope") or "shell"))
'
                return 0
            fi
            printf '%s' "$raw" | _rs_print_err
            return $?
            ;;

        grants)
            local host="" token="" json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --host)  host="${2:-}"; shift 2 ;;
                    --token) token="${2:-}"; shift 2 ;;
                    --json)  json=1; shift ;;
                    --help|-h) cmd_help_remote_session; return 0 ;;
                    *) echo "Unknown flag: $1" >&2; return 2 ;;
                esac
            done
            _rs_apply_target "$host" "$token" || return 1
            local raw
            raw=$(cli_request "/cli/remote-session/grants") || raw=""
            if printf '%s' "$raw" | python3 -c '
import json,sys
d=json.loads(sys.stdin.read() or "{}")
sys.exit(0 if isinstance(d,dict) and d.get("ok") is True else 1)
' 2>/dev/null; then
                if [ "$json" = "1" ]; then
                    printf '%s' "$raw" | python3 -m json.tool 2>/dev/null || printf '%s\n' "$raw"
                else
                    printf '%s' "$raw" | python3 -c '
import json,sys,datetime
d=json.loads(sys.stdin.read())
grants = d.get("grants") or []
if not grants:
    print("No grants."); sys.exit(0)
def fmt_ts(ts):
    try:
        return datetime.datetime.utcfromtimestamp(int(ts)).strftime("%Y-%m-%d %H:%M:%SZ")
    except Exception:
        return str(ts)
rows=[]
for g in grants:
    if not isinstance(g, dict):
        continue
    state = "revoked" if g.get("revokedAt") else "active"
    rows.append((
        str(g.get("id") or "?"),
        state,
        str(g.get("scope") or "shell"),
        fmt_ts(g.get("expiresAt")),
        str(g.get("label") or "-"),
        str(g.get("issuedBy") or "-"),
    ))
if not rows:
    print("No grants."); sys.exit(0)
w_id = max(len(r[0]) for r in rows + [("ID",)])
w_st = max(len(r[1]) for r in rows + [("STATE",)])
w_sc = max(len(r[2]) for r in rows + [("SCOPE",)])
print("%-*s  %-*s  %-*s  %-20s  %s  %s" % (w_id,"ID", w_st,"STATE", w_sc,"SCOPE", "EXPIRES", "LABEL", "ISSUED_BY"))
for r in rows:
    print("%-*s  %-*s  %-*s  %-20s  %s  %s" % (w_id,r[0], w_st,r[1], w_sc,r[2], r[3], r[4], r[5]))
'
                fi
                return 0
            fi
            printf '%s' "$raw" | _rs_print_err
            return $?
            ;;

        revoke)
            local id="" host="" token=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --host)  host="${2:-}"; shift 2 ;;
                    --token) token="${2:-}"; shift 2 ;;
                    --help|-h) cmd_help_remote_session; return 0 ;;
                    -*)
                        echo "Unknown flag: $1" >&2; return 2 ;;
                    *)
                        if [ -z "$id" ]; then id="$1"; shift
                        else echo "Unexpected argument: $1" >&2; return 2
                        fi ;;
                esac
            done
            if [ -z "$id" ]; then
                echo "Usage: k2 remote-session revoke <grant-id>" >&2
                return 2
            fi
            _rs_apply_target "$host" "$token" || return 1
            local body raw
            body=$(ID="$id" python3 -c 'import json,os; print(json.dumps({"id": os.environ["ID"]}))')
            raw=$(cli_post_json "/cli/remote-session/revoke" "$body") || raw=""
            if printf '%s' "$raw" | python3 -c '
import json,sys
d=json.loads(sys.stdin.read() or "{}")
sys.exit(0 if isinstance(d,dict) and d.get("ok") is True else 1)
' 2>/dev/null; then
                printf '%s' "$raw" | python3 -c '
import json,sys
d=json.loads(sys.stdin.read())
g = d.get("grant") or {}
print("Revoked grant %s." % (g.get("id") or "?"))
killed = d.get("killedSessions")
if killed is not None:
    print("Killed sessions: %s" % killed)
'
                return 0
            fi
            printf '%s' "$raw" | _rs_print_err
            return $?
            ;;

        shell)
            local host="" token="" cwd=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --host)  host="${2:-}"; shift 2 ;;
                    --token) token="${2:-}"; shift 2 ;;
                    --cwd)   cwd="${2:-}"; shift 2 ;;
                    --help|-h) cmd_help_remote_session; return 0 ;;
                    *) echo "Unknown flag: $1" >&2; return 2 ;;
                esac
            done
            _rs_apply_target "$host" "$token" || return 1
            local body raw
            if [ -n "$cwd" ]; then
                body=$(CWD="$cwd" python3 -c 'import json,os; print(json.dumps({"cwd": os.environ["CWD"]}))')
            else
                body="{}"
            fi
            raw=$(cli_post_json "/cli/remote-session/shell/spawn" "$body") || raw=""
            if printf '%s' "$raw" | python3 -c '
import json,sys
d=json.loads(sys.stdin.read() or "{}")
sys.exit(0 if isinstance(d,dict) and d.get("ok") is True else 1)
' 2>/dev/null; then
                printf '%s' "$raw" | python3 -c '
import json,sys
d=json.loads(sys.stdin.read())
sid = d.get("sessionId") or "?"
print("sessionId: %s" % sid)
print("grantId:   %s" % (d.get("grantId") or "?"))
print("ready:     %s" % ("true" if d.get("ready") else "false"))
print("")
print("Next:")
print("  k2 remote-session write %s \"command\" --token <k2rs_…>" % sid)
print("  k2 remote-session read  %s [--lines 50] --token <k2rs_…>" % sid)
'
                return 0
            fi
            printf '%s' "$raw" | _rs_print_err
            return $?
            ;;

        write)
            local sid="" text="" host="" token=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --host)  host="${2:-}"; shift 2 ;;
                    --token) token="${2:-}"; shift 2 ;;
                    --help|-h) cmd_help_remote_session; return 0 ;;
                    -*)
                        echo "Unknown flag: $1" >&2; return 2 ;;
                    *)
                        if [ -z "$sid" ]; then sid="$1"; shift
                        elif [ -z "$text" ]; then text="$1"; shift
                        else echo "Unexpected argument: $1" >&2; return 2
                        fi ;;
                esac
            done
            if [ -z "$sid" ] || [ -z "$text" ]; then
                echo "Usage: k2 remote-session write <session-id> \"text\" [--token k2rs_…] [--host HOST]" >&2
                return 2
            fi
            _rs_apply_target "$host" "$token" || return 1
            local raw
            raw=$(cli_request_post "/cli/terminal/write" \
                "id=$(urlencode "$sid")" \
                "message=$(urlencode "$text")") || raw=""
            # Empty or ok:true → ack. Nested/string error → teach.
            if [ -z "$raw" ]; then
                echo "wrote to $sid"
                return 0
            fi
            if printf '%s' "$raw" | python3 -c '
import json,sys
raw=sys.stdin.read().strip()
if not raw:
    sys.exit(0)
try:
    d=json.loads(raw)
except Exception:
    # Non-JSON success-ish body
    print(raw)
    sys.exit(0)
if not isinstance(d, dict):
    print(raw); sys.exit(0)
# Error shapes
if d.get("error") is not None and d.get("ok") is not True:
    sys.exit(1)
if d.get("ok") is True or "success" in d or "lines" in d:
    sys.exit(0)
# Unknown JSON without error — show it
print(raw)
sys.exit(0)
' 2>/dev/null; then
                if printf '%s' "$raw" | python3 -c '
import json,sys
raw=sys.stdin.read().strip()
if not raw: sys.exit(0)
try:
    d=json.loads(raw)
except Exception:
    sys.exit(1)
sys.exit(0 if isinstance(d,dict) and (d.get("ok") is True or "success" in d) else 1)
' 2>/dev/null; then
                    echo "wrote to $sid"
                fi
                return 0
            fi
            printf '%s' "$raw" | _rs_print_err
            return $?
            ;;

        read)
            local sid="" lines=50 host="" token=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --host)  host="${2:-}"; shift 2 ;;
                    --token) token="${2:-}"; shift 2 ;;
                    --lines) lines="${2:-}"; shift 2 ;;
                    --help|-h) cmd_help_remote_session; return 0 ;;
                    -*)
                        echo "Unknown flag: $1" >&2; return 2 ;;
                    *)
                        if [ -z "$sid" ]; then sid="$1"; shift
                        else echo "Unexpected argument: $1" >&2; return 2
                        fi ;;
                esac
            done
            if [ -z "$sid" ]; then
                echo "Usage: k2 remote-session read <session-id> [--lines N] [--token k2rs_…] [--host HOST]" >&2
                return 2
            fi
            _rs_apply_target "$host" "$token" || return 1
            local raw
            raw=$(cli_request "/cli/terminal/read" \
                "id=$(urlencode "$sid")" \
                "lines=$lines") || raw=""
            if printf '%s' "$raw" | python3 -c '
import json,sys
raw=sys.stdin.read()
try:
    d=json.loads(raw) if raw.strip() else {}
except Exception:
    sys.exit(1)
if not isinstance(d, dict):
    sys.exit(1)
if d.get("error") is not None and "lines" not in d:
    sys.exit(1)
for line in d.get("lines") or []:
    print(line)
sys.exit(0)
' 2>/dev/null; then
                return 0
            fi
            printf '%s' "$raw" | _rs_print_err
            return $?
            ;;

        *)
            echo "Unknown remote-session subcommand: $sub" >&2
            cmd_help_remote_session >&2
            return 2
            ;;
    esac
}

