# ── Updates ──────────────────────────────────────────────────────────

# `k2 update --help` / `-h`: the usage, on stdout. Sends nothing.
_update_usage() {
    cat <<'EOF'
Usage: k2 update [--app|--cli [--check]|--list]

  k2 update, k2 update --app
                 Show this k2's version and the newest K2 release on GitHub,
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

# `k2 update` / `--app`: this k2's version and the latest release's tag,
# with its download page. `k2 update --list`: the ten newest releases.
# cmd_update_v2 is the only caller, with --list or with nothing.
cmd_app_update() {
    # Both forms ask GitHub: refused under air-gap, like k2 daemon install.
    _k2_refuse_if_airgap
    if [ "${1:-}" = "--list" ]; then
        # List available versions from GitHub
        echo "Available K2 releases:"
        curl -sH "Accept: application/vnd.github.v3+json" \
            "https://api.github.com/repos/Alakazam-211/K2/releases?per_page=10" 2>/dev/null | \
            python3 -c "
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
    latest=$(curl -sH "Accept: application/vnd.github.v3+json" \
        "https://api.github.com/repos/Alakazam-211/K2/releases/latest" 2>/dev/null)
    local tag
    tag=$(echo "$latest" | python3 -c "import json,sys; print(json.loads(sys.stdin.read()).get('tag_name','unknown'))" 2>/dev/null)
    echo "  Current: v$K2_CLI_VERSION"
    echo "  Latest:  $tag"
    echo ""
    echo "To update the app, download from:"
    echo "  https://github.com/Alakazam-211/K2/releases/tag/$tag"
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
