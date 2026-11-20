# ── k2 remote: this computer's sign-in to another K2 server ───────────
# `k2 remote logout --host H` ends this computer's Connect session on H:
# POST <H>/cli/auth/logout with the token in an Authorization header
# (curl's config on stdin, never argv or the URL). The token is the one
# `k2 talk <ws>@H` sends (_remote_token_for_host: $K2_REMOTE_TOKEN,
# connect-tokens.json, then the keychain), or one read from stdin with
# --token-stdin. There is no --token: a command line is visible to every
# account on this computer.
#
# After H answers 2xx, every saved copy of THAT token for H goes too, the
# way the desktop's Sign out forgets it: the connect-tokens.json entry
# (the key forms connect_cli_token_delete uses; the old .k2so home's copy
# too) and the keychain items
# (dev.k2.connect.host-token and the older com.k2so.connect.host-token,
# account = the host's id in connect-hosts.json). A saved token that
# differs from the one just revoked stays. The server list and a
# remembered password stay, as on the desktop. A refusal or no answer
# keeps everything and exits non-zero.
#
# The server answers 200 whether or not it had the session, and an owner
# token has no session to end, so success proves only that H answered.

_remote_usage() {
    cat <<'EOF'
k2 remote — this computer's sign-in to another K2 server

  k2 remote logout --host HOST [--token-stdin]
      End this computer's session on HOST (POST /cli/auth/logout).
      The token sent is the one `k2 talk <ws>@HOST` sends
      ($K2_REMOTE_TOKEN, then connect-tokens.json, then the
      keychain), or with --token-stdin one read from stdin. A token
      never goes on the command line. HOST is a name (https) or a URL;
      plain http:// only to this computer.
      After HOST answers, the saved copies of that token for HOST go
      too (connect-tokens.json and the keychain). A saved token that
      differs is kept, and so are the server list and a remembered
      password.

  What logout can't do:
    It can't revoke an owner token: an owner token has no session to
    end (a restart of that server's daemon does).
    Success can't prove a session existed: the server answers 200
    whether or not it had one.

  Exit: 0 signed out, 1 no answer or an unexpected one, 2 usage,
  3 HOST refused (nothing is removed here).
EOF
}

cmd_remote() {
    local sub="${1:-}"
    case "$sub" in
        logout) shift; cmd_remote_logout "$@" ;;
        help|-h|--help) _remote_usage ;;
        "") _remote_usage >&2; return 2 ;;
        *)
            echo "k2 remote: unknown verb '$sub'" >&2
            _remote_usage >&2
            return 2
            ;;
    esac
}

# True when URL is http(s) to this computer (127.*, localhost, [::1]).
_remote_url_is_loopback() {
    # _k2_direct_url answers for https:// by the host alone.
    _k2_direct_url "https://${1#*://}"
}

# The id connect-hosts.json gives HOST (the keychain item's account), or
# nothing. Same matching as _remote_token_for_host: the hostname, its
# bare form, or the label.
_remote_host_id_for_host() {
    K2_HOME="$K2_HOME" python3 - "$1" <<'PY'
import json, os, sys

def bare(h):
    h = (h or "").strip().lower()
    while h.endswith(".k2.dev.k2.dev"):
        h = h[: -len(".k2.dev")]
    if h.endswith(".k2.dev"):
        h = h[: -len(".k2.dev")]
    return h

want = sys.argv[1]
b = bare(want)
want_keys = {want, want.lower(), b, b + ".k2.dev", b + ".K2.DEV"}
home = os.environ.get("K2_HOME") or os.path.join(os.path.expanduser("~"), ".k2")
homes = [home]
alt = os.path.join(os.path.expanduser("~"), ".k2so")
if alt not in homes and os.path.isdir(alt):
    homes.append(alt)
for base in homes:
    path = os.path.join(base, "connect-hosts.json")
    if not os.path.isfile(path):
        continue
    with open(path) as fh:
        data = json.load(fh)
    if not isinstance(data, list):
        continue
    for entry in data:
        if not isinstance(entry, dict) or not entry.get("id"):
            continue
        hn = entry.get("hostname") or ""
        if hn in want_keys or bare(hn) == b or bare(entry.get("label") or "") == b:
            print(entry["id"])
            sys.exit(0)
PY
}

# Remove the saved copies of TOKEN for HOST: the connect-tokens.json keys
# connect_cli_token_delete uses (HOST, its bare form, bare.k2.dev) whose
# value IS the token, in this home and in the old .k2so home, and the
# keychain items holding exactly it. Sets _REMOTE_REMOVED to what went
# ("connect-tokens.json", "the old .k2so connect-tokens.json", "keychain").
# The token reaches python3 on stdin; `security` and `secret-tool` get
# only the service and the account.
_remote_forget_token() {
    local host="$1" tok="$2" r id svc kc kc_gone=0
    _REMOTE_REMOVED=""
    # This home's connect-tokens.json, then the old .k2so home's copy (the
    # one _remote_token_for_host also reads), when that is another folder.
    r="$(printf '%s' "$tok" | K2_HOME="$K2_HOME" python3 -c '
import json, os, sys
host = sys.argv[1].strip()
tok = sys.stdin.read()
home = os.environ["K2_HOME"]
homes = [(home, "connect-tokens.json")]
alt = os.path.join(os.path.expanduser("~"), ".k2so")
if os.path.isdir(alt) and os.path.realpath(alt) != os.path.realpath(home):
    homes.append((alt, "the old .k2so connect-tokens.json"))
b = host
while b.endswith(".k2.dev"):
    b = b[: -len(".k2.dev")]
while b.endswith(".K2.DEV"):
    b = b[: -len(".K2.DEV")]
gone = []
for base, label in homes:
    path = os.path.join(base, "connect-tokens.json")
    if not os.path.isfile(path):
        continue
    with open(path) as fh:
        data = json.load(fh)
    if not isinstance(data, dict):
        sys.exit("%s is not a JSON object" % path)
    keys = [k for k in dict.fromkeys([host, b, b + ".k2.dev"]) if k and data.get(k) == tok]
    if not keys:
        continue
    for k in keys:
        del data[k]
    tmp = path + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump(data, fh, indent=2)
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)
    gone.append(label)
print(", ".join(gone))
' "$host")" || { echo "k2 remote logout: could not update connect-tokens.json" >&2; return 1; }
    _REMOTE_REMOVED="$r"
    id="$(_remote_host_id_for_host "$host")" || { echo "k2 remote logout: could not read ${K2_HOME}/connect-hosts.json" >&2; return 1; }
    [ -n "$id" ] || return 0
    for svc in dev.k2.connect.host-token com.k2so.connect.host-token; do
        if command -v security >/dev/null 2>&1; then
            kc="$(security find-generic-password -s "$svc" -a "$id" -w 2>/dev/null || true)"
            if [ -n "$kc" ] && [ "$kc" = "$tok" ]; then
                security delete-generic-password -s "$svc" -a "$id" >/dev/null 2>&1 \
                    || { echo "k2 remote logout: could not remove the keychain item $svc" >&2; return 1; }
                kc_gone=1
            fi
        fi
        if command -v secret-tool >/dev/null 2>&1; then
            kc="$(secret-tool lookup service "$svc" account "$id" 2>/dev/null || true)"
            if [ -n "$kc" ] && [ "$kc" = "$tok" ]; then
                secret-tool clear service "$svc" account "$id" >/dev/null 2>&1 \
                    || { echo "k2 remote logout: could not remove the keyring item $svc" >&2; return 1; }
                kc_gone=1
            fi
        fi
    done
    kc=""
    if [ "$kc_gone" = 1 ]; then
        _REMOTE_REMOVED="${_REMOTE_REMOVED:+$_REMOTE_REMOVED, }keychain"
    fi
    return 0
}

cmd_remote_logout() {
    local usage="usage: k2 remote logout --host HOST [--token-stdin]"
    local host="" from_stdin=0 a
    for a in "$@"; do
        case "$a" in -h|--help) _remote_usage; return 0 ;; esac
    done
    while [ $# -gt 0 ]; do
        case "$1" in
            --host)
                { [ $# -ge 2 ] && [ -n "$2" ]; } || { echo "$usage" >&2; return 2; }
                host="$2"; shift 2 ;;
            --host=*) host="${1#--host=}"; shift ;;
            --token-stdin) from_stdin=1; shift ;;
            --token|--token=*)
                echo "k2 remote logout: no --token (a command line is visible to every account on this computer); use --token-stdin" >&2
                return 2 ;;
            *) echo "$usage" >&2; return 2 ;;
        esac
    done
    [ -n "$host" ] || { echo "$usage" >&2; return 2; }
    host="${host%/}"
    local base
    case "$host" in
        https://*|HTTPS://*) base="$host" ;;
        http://*|HTTP://*)
            _remote_url_is_loopback "$host" || {
                echo "k2 remote logout: plain http:// only to this computer; use https://" >&2
                return 2
            }
            base="$host" ;;
        *) base="https://$host" ;;
    esac

    local tok=""
    if [ "$from_stdin" = 1 ]; then
        _cli_read_secret_stdin "Token for $host: "
        tok="${_K2_SECRET%$'\r'}"; _K2_SECRET=""
        [ -n "$tok" ] || { echo "k2 remote logout: --token-stdin: no token on stdin" >&2; return 2; }
    else
        tok="$(_remote_token_for_host "$host")"
        [ -n "$tok" ] || {
            echo "k2 remote logout: no saved token for '$host' (\$K2_REMOTE_TOKEN, ${K2_HOME}/connect-tokens.json or the keychain); pass it with --token-stdin" >&2
            return 2
        }
    fi

    local _K2_CFG="" out code body
    _k2_cfg_add url "${base}/cli/auth/logout"
    _k2_cfg_auth "$tok"
    _k2_cfg_add data-raw "{}"
    out="$(_K2_CURL_TRIES=1 _k2_curl "$_K2_CFG" -s -X POST --connect-timeout 5 --max-time 20 \
        -H "Content-Type: application/json" -w $'\n%{http_code}' 2>/dev/null || printf '\n000'; printf x)"
    _K2_CFG=""
    out="${out%x}"
    code="${out##*$'\n'}"
    body="${out%$'\n'*}"
    body="$(printf '%s' "$body" | tr '\n' ' ')"
    case "$code" in
        2??) ;;
        000)
            tok=""
            echo "k2 remote logout: no answer from $host; nothing removed here (the token may still work)" >&2
            return 1 ;;
        401|403)
            tok=""
            echo "k2 remote logout: $host refused (HTTP $code): ${body:0:300}" >&2
            echo "Nothing removed here." >&2
            return 3 ;;
        *)
            tok=""
            echo "k2 remote logout: $host answered HTTP $code: ${body:0:300}" >&2
            echo "Nothing removed here." >&2
            return 1 ;;
    esac

    _remote_forget_token "$host" "$tok" || {
        tok=""
        echo "k2 remote logout: $host ended the session, but a saved copy of the token is still here" >&2
        return 1
    }
    tok=""
    if [ -n "$_REMOTE_REMOVED" ]; then
        echo "signed out of $host (it ends this session if it had one); removed the saved token: $_REMOTE_REMOVED"
    else
        echo "signed out of $host (it ends this session if it had one); no saved copy of this token here"
    fi
}
