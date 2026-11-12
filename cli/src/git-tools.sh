# ── k2 git-tools — is git usable on this machine (0.46.4) ──────────────
# A fresh Mac has only Apple's git shim until the Command Line Tools are
# installed; a Linux box may have no git. The daemon adds workspaces anyway
# and does their git steps once git runs. `install` opens Apple's installer
# on the Mac (owner only, never sudo); for SSH or Linux, status prints the
# commands to run yourself.
_git_tools_usage() {
    cat <<'EOF'
k2 git-tools — is git usable here (Apple's command line tools on macOS)

  k2 git-tools [status] [--json]   git found?, install state, how to install, waiting workspaces
  k2 git-tools install [--json]    macOS: open Apple's installer for the command line tools
                                   (owner; a dialog on this Mac's screen; no sudo)
  k2 git-tools recheck [--json]    look for git again now and do the waiting git steps

Over SSH (no screen for Apple's dialog), status prints the softwareupdate
steps; they need an admin password, so run them yourself.
EOF
}

# Pure bash + curl: on a fresh Mac `/usr/bin/python3` is one of Apple's
# shims, so these verbs never touch python3 (or `urlencode`, which uses
# it). The daemon renders the text (`?format=text`).
_git_tools_q() {
    # Percent-encode $1 for a query value, byte by byte.
    local LC_ALL=C s="$1" out="" c i
    for ((i = 0; i < ${#s}; i++)); do
        c="${s:i:1}"
        case "$c" in
            [A-Za-z0-9._~-]) out+="$c" ;;
            *) printf -v c '%%%02X' "'$c"; out+="$c" ;;
        esac
    done
    printf '%s' "$out"
}

_git_tools_send() {
    # _git_tools_send <GET|POST> <path> <as_json>: the body, then the HTTP code.
    local method="$1" path="$2" as_json="$3" q
    q="token=$(_git_tools_q "$TOKEN")"
    [ "$as_json" = "1" ] || q="${q}&format=text"
    # Through _k2_curl: a refused connect (a daemon restart) is retried (S5).
    if [ "$method" = "GET" ]; then
        _k2_curl "" -s "${BASE_URL}${path}?${q}" --connect-timeout 5 --max-time 120 \
            -w $'\n%{http_code}' 2>/dev/null || true
    else
        _k2_curl "" -s -X POST "${BASE_URL}${path}?${q}" --connect-timeout 5 --max-time 120 \
            -H "Content-Type: application/json" --data-raw '{}' \
            -w $'\n%{http_code}' 2>/dev/null || true
    fi
}

_git_tools_render() {
    # _git_tools_render <as_json> <raw "body\ncode">
    local as_json="$1" raw="$2" code body err
    code="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ]; then
        echo "k2 git-tools: daemon unreachable" >&2
        exit 1
    fi
    if [ "$as_json" = "1" ]; then
        printf '%s\n' "$body"
        [ "$code" = "200" ] || exit 1
        return 0
    fi
    if [ "$code" = "200" ]; then
        printf '%s' "$body"
        return 0
    fi
    case "$body" in
        "k2 git-tools: "*) printf '%s' "$body" >&2 ;;
        *)
            # A JSON refusal from the router (no token, owner only, 405).
            if [[ "$body" =~ \"error\"[[:space:]]*:[[:space:]]*\"([^\"]*)\" ]]; then
                err="${BASH_REMATCH[1]}"
            else
                err="HTTP $code"
            fi
            echo "k2 git-tools: error: $err" >&2
            # 0.46.5: an agent's `install` is the owner's step: refused
            # (exit 3) with what to do instead, not a bare exit 1.
            if [ "$code" = "403" ] && [ "$err" = "owner_only" ]; then
                if [[ "$body" =~ \"hint\"[[:space:]]*:[[:space:]]*\"([^\"]*)\" ]]; then
                    echo "  ${BASH_REMATCH[1]}" >&2
                fi
                exit 3
            fi
            ;;
    esac
    exit 1
}

cmd_git_tools() {
    local sub="${1:-status}"
    [ $# -gt 0 ] && shift
    case "$sub" in
        help|--help|-h) _git_tools_usage; return 0 ;;
        --json) sub=status; set -- --json "$@" ;;
    esac
    local as_json=0 raw
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            -h|--help) _git_tools_usage; return 0 ;;
            *) echo "usage: unknown argument '$1' (k2 git-tools --help)" >&2; exit 2 ;;
        esac
    done
    case "$sub" in
        status)  raw="$(_git_tools_send GET /cli/git-tools "$as_json")" ;;
        install) raw="$(_git_tools_send POST /cli/git-tools/install "$as_json")" ;;
        recheck) raw="$(_git_tools_send POST /cli/git-tools/recheck "$as_json")" ;;
        *)
            echo "usage: unknown git-tools verb '$sub' (status|install|recheck)" >&2
            exit 2
            ;;
    esac
    _git_tools_render "$as_json" "$raw"
}

