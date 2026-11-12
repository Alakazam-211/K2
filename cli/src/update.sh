# ── Self-Update ──────────────────────────────────────────────────────

K2SO_CLI_RAW_URL="https://raw.githubusercontent.com/Alakazam-211/K2/main/cli/k2"

cmd_app_update() {
    local version="${1:-}"
    if [ "$version" = "--list" ] || [ "$version" = "-l" ]; then
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

    if [ -z "$version" ]; then
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
        return
    fi

    # Download specific version
    local tag="$version"
    # Add 'v' prefix if not present
    [[ "$tag" != v* ]] && tag="v$tag"
    local dmg_url="https://github.com/Alakazam-211/K2/releases/download/${tag}/K2_${tag#v}_aarch64.dmg"
    echo "Downloading K2 $tag..."
    echo "  URL: $dmg_url"
    local tmpfile="/tmp/K2_${tag}.dmg"
    if curl -L --progress-bar -o "$tmpfile" "$dmg_url" 2>&1; then
        echo ""
        echo "Downloaded to: $tmpfile"
        echo "Opening DMG..."
        open "$tmpfile"
    else
        echo "Error: Failed to download. Check the version tag exists." >&2
        exit 1
    fi
}

cmd_update() {
    local check_only=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --check|-c) check_only=1; shift ;;
            *) shift ;;
        esac
    done

    echo "Checking for updates..."

    # Download the latest version string from GitHub
    local remote_script
    remote_script=$(curl -sS --connect-timeout 5 --max-time 15 "$K2SO_CLI_RAW_URL" 2>/dev/null)
    if [ -z "$remote_script" ]; then
        echo "Error: Could not reach GitHub to check for updates." >&2
        exit 1
    fi

    # Extract the version from the remote script.
    # Use cut/tr — never a sed that matches K2_CLI_VERSION="0.46.11" (release.sh
    # rewrites every occurrence of that form when bumping the version).
    local remote_version
    remote_version=$(echo "$remote_script" | grep -m1 '^K2_CLI_VERSION=' | cut -d'"' -f2)
    if [ -z "$remote_version" ]; then
        echo "Error: Could not parse remote version." >&2
        exit 1
    fi

    echo "  Installed: $K2_CLI_VERSION"
    echo "  Latest:    $remote_version"

    # Compare versions (simple string comparison works for semver with same format)
    if [ "$K2_CLI_VERSION" = "$remote_version" ]; then
        echo "Already up to date."
        exit 0
    fi

    # Check if remote is actually newer (not a downgrade)
    local newer
    newer=$(printf '%s\n%s\n' "$K2_CLI_VERSION" "$remote_version" | sort -V | tail -1)
    if [ "$newer" = "$K2_CLI_VERSION" ]; then
        echo "Installed version is newer than remote (dev build?)."
        exit 0
    fi

    if [ "$check_only" = "1" ]; then
        echo "Update available: $K2_CLI_VERSION → $remote_version"
        echo "Run 'k2 update' to install."
        exit 0
    fi

    # Find where k2 is installed
    local install_path
    install_path=$(which k2 2>/dev/null)
    if [ -z "$install_path" ]; then
        echo "Error: Could not find k2 in PATH." >&2
        exit 1
    fi

    # Check if we can write to the install path
    if [ ! -w "$install_path" ]; then
        echo "Updating $install_path (requires sudo)..."
        echo "$remote_script" | sudo tee "$install_path" > /dev/null
        sudo chmod +x "$install_path"
    else
        echo "Updating $install_path..."
        echo "$remote_script" > "$install_path"
        chmod +x "$install_path"
    fi

    echo "Updated k2: $K2_CLI_VERSION → $remote_version"
}

