# ── phone-web-off: pair a Companion phone (k2 users pair-phone|phones|unpair-phone)
# Thin over POST /cli/auth/device/pair-code, GET /cli/auth/devices and
# POST /cli/auth/device/revoke. The daemon decides who a code pairs as and
# draws the QR matrix; this only prints what it returns. Device names come
# from phones, so every name is printed with unprintable characters
# replaced (security review F7).

# Shared reader: daemon body on stdin → exit 0 (ok), 3 (refused), 1 (other).
_users_device_error_py='
import json, sys
def fail(raw):
    try:
        d = json.loads(raw) if raw.strip() else {}
    except Exception:
        sys.stderr.write("Error: daemon returned an unparseable response: %r\n" % (raw[:200],)); sys.exit(1)
    if not isinstance(d, dict):
        sys.stderr.write("Error: unexpected response\n"); sys.exit(1)
    err = d.get("error")
    if err is None:
        return d
    if isinstance(err, dict):
        code = err.get("code") or ""
        hint = err.get("hint") or err.get("message") or code or "error"
    else:
        code, hint = "", str(err)
    sys.stderr.write("Error: %s%s\n" % (hint, (" (" + code + ")") if code else ""))
    refused = code in ("login_required", "owner_token_required", "owner_login_not_linked", "password_invalid",
                       "password_change_required", "device_pairing_not_from_browser", "rate_limited",
                       "device_session_ended", "no_such_device", "role_required") or "token" in hint.lower()
    sys.exit(3 if refused else 1)
def safe(s):
    return "".join(c if c.isprintable() else "?" for c in str(s or ""))
'

_users_pair_phone() {
    local user="" as_json=0 invert=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --user) user="${2:-}"; shift 2 || { echo "Error: --user needs a login" >&2; return 2; } ;;
            --user=*) user="${1#--user=}"; shift ;;
            --json) as_json=1; shift ;;
            --invert) invert=1; shift ;;
            --help|-h) _users_usage; return 0 ;;
            *) echo "Error: unexpected argument '$1'" >&2; echo "Usage: k2 users pair-phone [--user <login>] [--json] [--invert]" >&2; return 2 ;;
        esac
    done
    local body resp
    body=$(USER_ARG="$user" python3 -c '
import json, os
b = {}
if os.environ.get("USER_ARG"):
    b["user"] = os.environ["USER_ARG"]
print(json.dumps(b))
')
    resp=$(cli_send cli_post_json "/cli/auth/device/pair-code" "$body") || resp=""
    # A login (not the owner token) must type its password again.
    if printf '%s' "$resp" | grep -q 'type your password again to make a pairing code'; then
        local pw=""
        if [ -t 0 ]; then
            printf 'Password for your login: ' >&2
            IFS= read -rs pw || pw=""
            echo >&2
        else
            IFS= read -r pw || pw=""
        fi
        [ -n "$pw" ] || { echo "Error: a password is needed to make a pairing code for your login." >&2; return 2; }
        body=$(USER_ARG="$user" PW="$pw" python3 -c '
import json, os
b = {"password": os.environ["PW"]}
if os.environ.get("USER_ARG"):
    b["user"] = os.environ["USER_ARG"]
print(json.dumps(b))
')
        pw=""
        resp=$(cli_send cli_post_json "/cli/auth/device/pair-code" "$body") || resp=""
        body=""
    fi
    if [ -z "$resp" ]; then
        echo "Error: empty response from daemon (is it running?)" >&2
        return 1
    fi
    printf '%s' "$resp" | AS_JSON="$as_json" INVERT="$invert" python3 -c "$_users_device_error_py"'
import os
raw = sys.stdin.read()
d = fail(raw)
if os.environ.get("AS_JSON") == "1":
    print(json.dumps(d, ensure_ascii=False))
    sys.exit(0)
qr = d.get("qr") or {}
rows = qr.get("rows") or []
if rows:
    # Light modules are drawn as blocks (dark-background terminals); a
    # 2-module quiet zone. --invert for light-background terminals.
    invert = os.environ.get("INVERT") == "1"
    n = len(rows)
    pad = 2
    def dark(x, y):
        if x < 0 or y < 0 or x >= n or y >= n:
            return False
        return rows[y][x] == "1"
    def ink(x, y):
        d = dark(x, y)
        return d if invert else not d
    out = []
    for y in range(-pad, n + pad, 2):
        line = ""
        for x in range(-pad, n + pad):
            top, bot = ink(x, y), ink(x, y + 1) if y + 1 < n + pad else (False if invert else True)
            line += "█" if top and bot else ("▀" if top else ("▄" if bot else " "))
        out.append(line)
    print("\n".join(out))
    print()
print("Pairing code for %s: %s" % (safe(d.get("user")), safe(d.get("code"))))
if d.get("pairUrl"):
    print("Link: %s" % safe(d.get("pairUrl")))
    print("Scan the QR with the phone (or open the link) to pair it in the K2 app.")
else:
    print("This server has no public host, so there is no link or QR: type the code on the K2 app'"'"'s Pair screen.")
print("Works once, for %d minutes (until %s)." % (int(d.get("expiresInSecs") or 600) // 60, safe(d.get("expiresAt"))))
print("A paired phone stays signed in while used (30 days unused or 180 days in all, then it pairs again).")
'
}

_users_phones() {
    local as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --help|-h) _users_usage; return 0 ;;
            *) echo "Error: unexpected argument '$1'" >&2; echo "Usage: k2 users phones [--json]" >&2; return 2 ;;
        esac
    done
    local resp
    resp=$(cli_send cli_request "/cli/auth/devices") || resp=""
    if [ -z "$resp" ]; then
        echo "Error: empty response from daemon (is it running?)" >&2
        return 1
    fi
    printf '%s' "$resp" | AS_JSON="$as_json" python3 -c "$_users_device_error_py"'
import os
raw = sys.stdin.read()
d = fail(raw)
if os.environ.get("AS_JSON") == "1":
    print(json.dumps(d, ensure_ascii=False))
    sys.exit(0)
pa = d.get("pairAs") or {}
if pa.get("user"):
    print("A new pairing code pairs as: %s" % safe(pa.get("user")))
elif isinstance(pa.get("error"), dict):
    print("A new pairing code: %s" % safe(pa["error"].get("hint") or pa["error"].get("code")))
devices = d.get("devices") or []
if not devices:
    print("No paired phones.")
    sys.exit(0)
rows = [("ID", "LOGIN", "PHONE", "PAIRED", "ENDS IF UNUSED", "PAIRED BY")]
for x in devices:
    name = safe(x.get("name")) + ("  (this one)" if x.get("current") else "")
    rows.append((safe(x.get("id")), safe(x.get("username")), name, safe(x.get("createdAt"))[:19],
                 safe(x.get("expiresAt"))[:19], safe(x.get("pairedBy") or "-")))
w = [max(len(r[i]) for r in rows) for i in range(len(rows[0]))]
for r in rows:
    print("  ".join(c.ljust(w[i]) for i, c in enumerate(r)).rstrip())
'
}

_users_unpair_phone() {
    local id="" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --help|-h) _users_usage; return 0 ;;
            -*) echo "Error: unknown flag '$1'" >&2; return 2 ;;
            *) if [ -z "$id" ]; then id="$1"; shift; else echo "Error: unexpected argument '$1'" >&2; return 2; fi ;;
        esac
    done
    if [ -z "$id" ]; then
        echo "Usage: k2 users unpair-phone <id> [--json]   (ids: k2 users phones)" >&2
        return 2
    fi
    local body resp
    body=$(ID="$id" python3 -c 'import json, os; print(json.dumps({"id": os.environ["ID"]}))')
    resp=$(cli_send cli_post_json "/cli/auth/device/revoke" "$body") || resp=""
    if [ -z "$resp" ]; then
        echo "Error: empty response from daemon (is it running?)" >&2
        return 1
    fi
    printf '%s' "$resp" | AS_JSON="$as_json" python3 -c "$_users_device_error_py"'
import os
raw = sys.stdin.read()
d = fail(raw)
if os.environ.get("AS_JSON") == "1":
    print(json.dumps(d, ensure_ascii=False))
    sys.exit(0)
print("Unpaired phone %s (%s). It must pair again to sign in." % (safe(d.get("revoked")), safe(d.get("username"))))
'
}

