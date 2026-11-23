# ── Updates ──────────────────────────────────────────────────────────

# `k2 update --help` / `-h`: the usage, on stdout. Sends nothing.
_update_usage() {
    cat <<'EOF'
Usage: k2 update [--app|--cli [--check]|--list]

  k2 update, k2 update --app
                 Show this CLI's version and the newest K2 release on GitHub,
                 with the page to download it from.
  k2 update --list
                 List the ten newest K2 releases on GitHub.
  k2 update --cli [--check]
                 Show the CLI's and the local daemon's versions. The CLI
                 updates with K2; --check leaves out "Nothing was changed."

  None of these downloads or installs anything. --app and --list ask
  GitHub, so they are refused while air-gap is on; they need no running
  daemon. --cli asks this machine's daemon for its version.
EOF
}

# GitHub's own hosts: the only ones a GitHub fetch follows a redirect to.
# api.github.com answers a moved repository's old path with a 301 to its
# new one (on api.github.com), github.com does the same for its pages and
# downloads, and github.com's release downloads redirect to
# release-assets.githubusercontent.com (objects.githubusercontent.com for
# older links). Curl can't limit -L to a list of hosts, so the fetch never
# uses -L: it follows each hop itself, https only, after checking the host,
# and refuses a redirect anywhere else before that address is contacted.
_K2_GITHUB_HOSTS="api.github.com github.com objects.githubusercontent.com release-assets.githubusercontent.com"
# The most redirects one fetch follows.
_K2_GITHUB_MAX_REDIRS=5
# Why _k2_github_curl refused a redirect; empty when it didn't.
_K2_GITHUB_REFUSED=""

# `_k2_github_url_ok <url>`: 0 when <url> is https on one of GitHub's hosts,
# with no name before an @ and no port.
_k2_github_url_ok() {
    local rest host h
    case "$1" in https://*) ;; *) return 1 ;; esac
    rest="${1#https://}"
    host="${rest%%[/?#]*}"
    for h in $_K2_GITHUB_HOSTS; do
        [ "$host" = "$h" ] && return 0
    done
    return 1
}

# `_k2_github_where <url>`: the scheme and host of <url>, printable
# characters only, for an error line (never the path or the query).
_k2_github_where() {
    local scheme rest
    case "$1" in
        *://*) scheme="${1%%://*}"; rest="${1#*://}"; rest="${scheme}://${rest%%[/?#]*}" ;;
        *) rest="${1%%[/?#]*}" ;;
    esac
    printf '%s' "$rest" | LC_ALL=C tr -cd '[:graph:]'
}

# `_k2_github_curl <url> <curl args...>`: curl <url> with <args>, which
# send the body to a file (-o), following redirects on GitHub's hosts one
# hop at a time (at most _K2_GITHUB_MAX_REDIRS). https only, also when a
# ~/.curlrc says -L. Sends no token: these calls carry no -K config and no
# Authorization header. Returns curl's exit code when curl fails. A
# redirect off GitHub's hosts, to anything but https, or past the cap sets
# _K2_GITHUB_REFUSED to the one-line reason and returns 1, and nothing is
# sent to that address. Runs in the caller's shell, so the caller reads
# _K2_GITHUB_REFUSED after it.
_k2_github_curl() {
    local url="$1" hops=0 next rc
    shift
    _K2_GITHUB_REFUSED=""
    while :; do
        rc=0
        next=$(curl --no-location --proto =https "$@" -w '%{redirect_url}' "$url") || rc=$?
        [ "$rc" -eq 0 ] || return "$rc"
        [ -n "$next" ] || return 0
        hops=$((hops + 1))
        if [ "$hops" -gt "$_K2_GITHUB_MAX_REDIRS" ]; then
            _K2_GITHUB_REFUSED="GitHub redirected more than $_K2_GITHUB_MAX_REDIRS times (the last to $(_k2_github_where "$next")); stopped there"
            return 1
        fi
        case "$next" in
            https://*) ;;
            *) _K2_GITHUB_REFUSED="GitHub redirected to $(_k2_github_where "$next"), which is not https; nothing was sent there"
               return 1 ;;
        esac
        if ! _k2_github_url_ok "$next"; then
            _K2_GITHUB_REFUSED="GitHub redirected to $(_k2_github_where "$next"), which is not GitHub; nothing was sent there"
            return 1
        fi
        url="$next"
    done
}

# `_k2_github_get <url> <file>`: GET <url> into <file> through
# _k2_github_curl: curl's own error on stderr (-sS), or a refused
# redirect's one line. Returns non-zero on either.
_k2_github_get() {
    local rc=0
    _K2_GITHUB_REFUSED=""
    _k2_github_curl "$1" -fsS -o "$2" || rc=$?
    if [ -n "$_K2_GITHUB_REFUSED" ]; then
        echo "Error: $_K2_GITHUB_REFUSED" >&2
        return 1
    fi
    return "$rc"
}

# `_app_update_get <url>`: GET a GitHub API URL, the body on stdout. It
# follows GitHub's own redirects (a moved repository answers its old path
# with a 301) and refuses any other with one line on stderr and returns 1
# (_k2_github_curl). When GitHub can't be reached (no network, refused,
# timed out, an HTTP error) it prints one line on stderr, with curl's exit
# code and message, and returns 1. Before, under set -e the failed curl
# stopped the command with curl's code and no word.
_app_update_get() {
    local tmp rc=0 msg
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/k2-update.XXXXXX") || {
        echo "Error: couldn't make a temp file to ask GitHub" >&2
        return 1
    }
    _K2_GITHUB_REFUSED=""
    _k2_github_curl "$1" -fsS --connect-timeout 10 --max-time 30 \
        -H "Accept: application/vnd.github.v3+json" -o "$tmp/body" 2>"$tmp/err" || rc=$?
    if [ -n "$_K2_GITHUB_REFUSED" ]; then
        rm -rf "$tmp"
        echo "Error: $_K2_GITHUB_REFUSED" >&2
        return 1
    fi
    msg=$(head -n 1 "$tmp/err" | sed 's/^curl: ([0-9]*) //')
    if [ "$rc" -ne 0 ]; then
        rm -rf "$tmp"
        echo "Error: couldn't reach GitHub (curl exit $rc${msg:+: $msg})" >&2
        return 1
    fi
    # curl makes no file for an empty body.
    if [ -f "$tmp/body" ]; then cat "$tmp/body"; fi
    rm -rf "$tmp"
}

# `k2 update` / `--app`: this CLI's version and the latest release's tag,
# with its download page. `k2 update --list`: the ten newest releases.
# cmd_update_v2 is the only caller, with --list or with nothing.
# GitHub out of reach: one line on stderr and exit 1 (_app_update_get).
cmd_app_update() {
    # Both forms ask GitHub: refused under air-gap, like k2 daemon install.
    _k2_refuse_if_airgap
    if [ "${1:-}" = "--list" ]; then
        local releases
        releases=$(_app_update_get "https://api.github.com/repos/${_K2_RELEASE_REPO}/releases?per_page=10") || exit 1
        echo "Available K2 releases:"
        printf '%s' "$releases" | python3 -c "
import json, sys
data = json.loads(sys.stdin.read())
for r in data:
    tag = r.get('tag_name', '?')
    date = r.get('published_at', '')[:10]
    pre = ' (pre-release)' if r.get('prerelease') else ''
    print(f'  {tag}  {date}{pre}')
" 2>/dev/null
        return
    fi

    # Check for latest and show info
    echo "Checking for app updates..."
    local latest
    latest=$(_app_update_get "https://api.github.com/repos/${_K2_RELEASE_REPO}/releases/latest") || exit 1
    local tag
    tag=$(echo "$latest" | python3 -c "import json,sys; print(json.loads(sys.stdin.read()).get('tag_name','unknown'))" 2>/dev/null)
    echo "  Current: v$K2_CLI_VERSION"
    echo "  Latest:  $tag"
    echo ""
    echo "To update the app, download from:"
    echo "  https://github.com/${_K2_RELEASE_REPO}/releases/tag/$tag"
    echo ""
    echo "Or use: k2 update --app --list  to see all versions"
}

# `k2 update --cli [--check]`. The CLI never updates itself: it ships with K2
# (the app bundle, Linux packages), and the daemon's CLI staging and the root
# helper's cli-refresh keep the installed copy matched to the daemon. So this
# only says which version is installed, whether the local daemon runs the
# same one, and how K2 updates. It downloads, replaces and runs nothing.
# The daemon's version is the one read from loopback /boot-status (no token).
_update_daemon_version() {
    [ -n "${PORT:-}" ] || return 0
    local body
    body=$(_K2_CURL_TRIES=1 _k2_curl "" -s --connect-timeout 2 --max-time 4 \
        "http://127.0.0.1:${PORT}/boot-status" 2>/dev/null) || return 0
    printf '%s' "$body" | python3 -c '
import json, sys
try:
    b = json.loads(sys.stdin.read() or "null")
except Exception:
    sys.exit(0)
v = b.get("version") if isinstance(b, dict) else None
if isinstance(v, str) and v.strip():
    print(v.strip())
' 2>/dev/null || true
}

cmd_update() {
    local check_only=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --check|-c) check_only=1; shift ;;
            *) shift ;;
        esac
    done

    local daemon_version
    daemon_version=$(_update_daemon_version)
    daemon_version="${daemon_version#v}"

    echo "k2 CLI ${K2_CLI_VERSION}"
    if [ -z "$daemon_version" ]; then
        echo "K2 daemon: didn't answer, so its version is unknown."
    elif [ "$daemon_version" = "$K2_CLI_VERSION" ]; then
        echo "K2 daemon: ${daemon_version} (same as the CLI)"
    else
        echo "K2 daemon: ${daemon_version} (differs from the CLI's ${K2_CLI_VERSION})"
    fi
    echo ""
    echo "The CLI updates together with K2; k2 never downloads itself."
    echo "  Desktop: k2 update --app, or the app's own updater."
    echo "  Linux:   your K2 package, or the install script."
    echo "  The K2 daemon and root helper keep this CLI matched to K2."
    if [ "$check_only" != "1" ]; then
        echo ""
        echo "Nothing was changed."
    fi
    exit 0
}
