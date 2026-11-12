# ── P2: headless standalone-daemon installer ─────────────────────────
#
# `k2 daemon install` fetches the per-OS standalone daemon binary
# published by the release pipeline (P1), minisign-verifies its .sig2 with
# the K2 update keys (embedded below; SS4), and
# drops it into a bin dir + writes a supervisor unit so a crash/restart
# respawns the process.
#
# This command MUST run on a fresh box with NO daemon running, so the
# top-level router intercepts it BEFORE the connection gate (search for
# DAEMON_INSTALL_EARLY_BYPASS). It uses no cli_request()/HTTP-to-daemon.
#
# The K2 update keys (prd-security-sweep-0464-v1 SS4): minisign key lines,
# U2 (primary) then U3 (offline backup), space-separated. A copy of
# k2_release_keys::UPDATER_KEYS (crates/k2-release-keys); a test fails when
# the copies drift (also scripts/install-daemon.sh). Written only by
# scripts/updater-key-ceremony.sh pubkeys. Empty until the key ceremony
# lands: then `k2 daemon install` refuses every binary.
_DAEMON_UPDATE_PUBKEYS="RWRpK+630xTKdopKvlpWVqvnEwqyK1CZ+F5deeRjpGA2ywyWS8NnOrpv RWSkAlEaeKUBUZhQA52GoMLa0rt8KLdEoto26gLHRpm9QIC5ColsvG/Y"
# Releases before this carry no .sig2 (signed with the retired key only).
_DAEMON_FIRST_U2_VERSION="0.46.6"

_DAEMON_DEFAULT_LATEST_URL="https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json"
_DAEMON_VERSION_URL_TMPL="https://github.com/Alakazam-211/K2/releases/download/v%s/daemon-latest.json"

# Map uname → manifest artifact key (e.g. "macos-aarch64"). Echoes the
# key on stdout; on an unknown platform prints an error and returns 1.
_daemon_platform_key() {
    local os arch
    case "$(uname -s)" in
        Darwin) os="macos" ;;
        Linux)  os="linux" ;;
        *) echo "Unsupported OS for standalone daemon: $(uname -s) (expected Darwin or Linux)" >&2; return 1 ;;
    esac
    case "$(uname -m)" in
        arm64|aarch64) arch="aarch64" ;;
        x86_64|amd64)  arch="x86_64" ;;
        *) echo "Unsupported CPU arch for standalone daemon: $(uname -m) (expected arm64/aarch64 or x86_64)" >&2; return 1 ;;
    esac
    printf '%s-%s\n' "$os" "$arch"
    return 0
}

# Extract one field for a platform key from a daemon-latest.json blob.
# Args: <json> <platform-key> <field>  (field ∈ url|sig|sha256)
# Pure sed/grep — no python. Isolates the platform object first so a
# field from a sibling platform can't bleed in.
_daemon_manifest_field() {
    local json="$1" key="$2" field="$3"
    # Collapse newlines so the object lives on a single logical line,
    # then carve out from "<key>": { ... } up to the closing brace.
    local obj
    obj=$(printf '%s' "$json" | tr '\n' ' ' \
        | sed -n "s/.*\"${key}\"[[:space:]]*:[[:space:]]*{\([^}]*\)}.*/\1/p")
    [ -n "$obj" ] || return 1
    printf '%s' "$obj" \
        | sed -n "s/.*\"${field}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p"
}

# Top-level "version" field from the manifest (informational).
_daemon_manifest_version() {
    printf '%s' "$1" | tr '\n' ' ' \
        | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p'
}

# _daemon_version_lt <a> <b>: a is an older x.y.z than b (unparsable = older).
_daemon_version_lt() {
    awk -v a="$1" -v b="$2" 'BEGIN {
        if (a !~ /^v?[0-9]+\.[0-9]+\.[0-9]+$/ || b !~ /^v?[0-9]+\.[0-9]+\.[0-9]+$/) exit 0
        sub(/^v/, "", a); sub(/^v/, "", b)
        split(a, x, "."); split(b, y, ".")
        for (i = 1; i <= 3; i++) { if (x[i] + 0 < y[i] + 0) exit 0; if (x[i] + 0 > y[i] + 0) exit 1 }
        exit 1
    }'
}

_daemon_retired_key_msg() {
    echo "Versions before $_DAEMON_FIRST_U2_VERSION are signed with the retired update key, which this k2 doesn't trust." >&2
    echo "Install $_DAEMON_FIRST_U2_VERSION or newer." >&2
}

# _daemon_verify_update_sig <bin> <minisig> <asset> <version>: one of the K2
# update keys signed it, and the signed trusted comment names exactly this
# asset (file:) and version (version:). 0 = verified.
_daemon_verify_update_sig() {
    local bin="$1" sigf="$2" asset="$3" ver="$4" k tc files vers
    if [ -z "$_DAEMON_UPDATE_PUBKEYS" ]; then
        echo "This k2 trusts no update key yet (the update-key ceremony has not landed). Refusing." >&2
        return 1
    fi
    # Prehashed (ED) only, like the daemon: `minisign -V` alone would also
    # take a legacy (Ed) signature. Base64 "RU[QRST]" is exactly "ED".
    case "$(sed -n 2p "$sigf")" in
        RU[QRST]*) ;;
        *) echo "The signature is not a prehashed minisign signature. Refusing." >&2; return 1 ;;
    esac
    for k in $_DAEMON_UPDATE_PUBKEYS; do
        if tc=$(minisign -V -P "$k" -m "$bin" -x "$sigf" -Q 2>/dev/null); then
            files=$(printf '%s\n' "$tc" | tr '\t' '\n' | grep '^file:' || true)
            vers=$(printf '%s\n' "$tc" | tr '\t' '\n' | grep '^version:' || true)
            if [ "$files" != "file:$asset" ]; then
                echo "The signature is for '${files#file:}', not $asset. Refusing." >&2
                return 1
            fi
            if [ "$vers" != "version:$ver" ]; then
                echo "The signature is for version '${vers#version:}', not $ver. Refusing." >&2
                return 1
            fi
            return 0
        fi
    done
    echo "Not signed by a K2 update key." >&2
    return 1
}

# Compute sha256 hex of a file using whichever tool is present.
_daemon_sha256() {
    local f="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$f" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$f" | awk '{print $1}'
    else
        echo "Neither sha256sum nor shasum found — cannot verify checksum." >&2
        return 1
    fi
}

# Linux: the binary an installed systemd unit runs — the last non-empty
# ExecStart= of `systemctl cat` (drop-ins included; an empty ExecStart=
# resets), its first word, without systemd's -@:+! prefixes, %h expanded.
# Args: <scope> <unit>. Prints the path or nothing.
_daemon_unit_exec_path() {
    local scope="$1" unit="$2" line
    line=$(_daemon_systemctl "$scope" cat "$unit" 2>/dev/null \
        | sed -n 's/^[[:space:]]*ExecStart=[[:space:]]*//p' | sed '/^$/d' | tail -1)
    [ -n "$line" ] || return 1
    line="${line%% *}"
    while :; do
        case "$line" in
            [-@:+!]*) line="${line#?}" ;;
            *) break ;;
        esac
    done
    line="${line//%h/$HOME}"
    [ -n "$line" ] || return 1
    printf '%s\n' "$line"
}

# Restart an installed daemon unit after its binary was replaced: a plain
# restart (KillMode=process keeps the agents). A system unit needs root:
# try systemctl, then `sudo -n` (never a password prompt).
# Args: <scope> <unit>.
_daemon_unit_restart() {
    local scope="$1" unit="$2"
    if [ "$scope" = "user" ]; then
        systemctl --user restart "$unit"
        return $?
    fi
    if systemctl restart "$unit" 2>/dev/null; then
        return 0
    fi
    if [ "$(id -u)" != "0" ] && command -v sudo >/dev/null 2>&1; then
        sudo -n systemctl restart "$unit" 2>/dev/null && return 0
    fi
    return 1
}

cmd_daemon_install() {
    local version="" bin_dir="" manifest_url="" no_service=0 dry_run=0
    # Help must still print when air-gap is on (no GitHub). Network is
    # refused after flag parse.
    local _saw_help=0
    for _a in "$@"; do
        case "$_a" in -h|--help) _saw_help=1 ;; esac
    done
    if [ "$_saw_help" != "1" ]; then
        _k2_refuse_if_airgap
    fi
    while [ $# -gt 0 ]; do
        case "$1" in
            --version)      version="${2:-}"; shift 2 ;;
            --bin-dir)      bin_dir="${2:-}"; shift 2 ;;
            --manifest-url) manifest_url="${2:-}"; shift 2 ;;
            --no-service)   no_service=1; shift ;;
            --dry-run)      dry_run=1; shift ;;
            -h|--help)
                echo "Usage: k2 daemon install [--version <x.y.z>] [--bin-dir <dir>]"
                echo "                           [--manifest-url <url>] [--no-service] [--dry-run]"
                echo ""
                echo "  Install the STANDALONE (headless, no-GUI) K2 daemon binary"
                echo "  from a signed GitHub release ($_DAEMON_FIRST_U2_VERSION or newer). Minisign-verifies"
                echo "  the binary with the embedded K2 update keys before installing."
                echo ""
                echo "  --version       install a specific release tag (default: latest)."
                echo "                  $_DAEMON_FIRST_U2_VERSION or newer; an OLDER release than the one installed"
                echo "                  is allowed here on purpose (a local, explicit action). The"
                echo "                  daemon's own self-update refuses a downgrade unless the"
                echo "                  owner asks for it."
                echo "  --bin-dir       install dir (default: ~/.local/bin)."
                echo "  --manifest-url  override the daemon-latest.json URL"
                echo "                  (supports file:// for local testing)."
                echo "  --no-service    skip writing the systemd/launchd supervisor unit."
                echo "  --dry-run       print resolved platform/artifact/commands; no download."
                echo ""
                echo "  Linux: when a K2 daemon systemd unit is already installed (a system"
                echo "  unit from provisioning, or a user unit), the binary that unit runs"
                echo "  is replaced in place and the unit restarted; no new unit is written."
                return 0
                ;;
            *) echo "k2 daemon install: unknown argument: $1" >&2; return 2 ;;
        esac
    done

    local bin_dir_given=0
    [ -n "$bin_dir" ] && bin_dir_given=1
    [ -n "$bin_dir" ] || bin_dir="$HOME/.local/bin"

    # Resolve manifest URL: explicit override > version tag > latest.
    local resolved_manifest_url
    if [ -n "$manifest_url" ]; then
        resolved_manifest_url="$manifest_url"
    elif [ -n "$version" ]; then
        if _daemon_version_lt "$version" "$_DAEMON_FIRST_U2_VERSION"; then
            _daemon_retired_key_msg
            return 1
        fi
        resolved_manifest_url=$(printf "$_DAEMON_VERSION_URL_TMPL" "$version")
    else
        resolved_manifest_url="$_DAEMON_DEFAULT_LATEST_URL"
    fi

    # Platform detect.
    local platform
    platform=$(_daemon_platform_key) || return 1

    # Fetch manifest. file:// is read directly; http(s) via curl.
    local manifest
    case "$resolved_manifest_url" in
        file://*)
            local mpath="${resolved_manifest_url#file://}"
            if [ ! -f "$mpath" ]; then
                echo "Manifest not found: $mpath" >&2
                return 1
            fi
            manifest=$(cat "$mpath")
            ;;
        *)
            if ! command -v curl >/dev/null 2>&1; then
                echo "curl is required to fetch the daemon manifest." >&2
                return 1
            fi
            manifest=$(curl -fsSL "$resolved_manifest_url") || {
                echo "Failed to fetch daemon manifest: $resolved_manifest_url" >&2
                return 1
            }
            ;;
    esac

    local mver url sig sha256
    mver=$(_daemon_manifest_version "$manifest")
    url=$(_daemon_manifest_field "$manifest" "$platform" "url") || true
    # Only the K2 update keys' signature (.sig2) counts; `sig` is for
    # installs older than $_DAEMON_FIRST_U2_VERSION.
    sig=$(_daemon_manifest_field "$manifest" "$platform" "sig2") || true
    sha256=$(_daemon_manifest_field "$manifest" "$platform" "sha256") || true

    if [ -z "$url" ] || [ -z "$sha256" ] || [ -z "$mver" ]; then
        echo "Manifest has no complete artifact for platform '$platform'." >&2
        echo "  url=${url:-<missing>} sha256=${sha256:-<missing>}" >&2
        echo "  (manifest version: ${mver:-<unknown>})" >&2
        return 1
    fi
    if [ -z "$sig" ] || _daemon_version_lt "$mver" "$_DAEMON_FIRST_U2_VERSION"; then
        _daemon_retired_key_msg
        return 1
    fi
    if [ -n "$version" ] && [ "$mver" != "${version#v}" ]; then
        echo "The manifest is for $mver, not the requested $version. Refusing." >&2
        return 1
    fi
    local asset="k2-daemon-$platform"

    local bin_path="$bin_dir/k2-daemon"

    # Service unit targets (computed for dry-run + real install).
    local svc_kind svc_path
    if [ "$(uname -s)" = "Darwin" ]; then
        svc_kind="launchd"
        svc_path="$HOME/Library/LaunchAgents/${_daemon_label}.plist"
    else
        svc_kind="systemd"
        svc_path="$HOME/.config/systemd/user/k2-daemon.service"
    fi

    # Linux with the daemon already installed under systemd (a SYSTEM unit
    # from provisioning, or a user unit from an earlier install): update the
    # binary THAT unit runs, in place, and restart that unit. Never write a
    # second unit or a second binary beside it (akzm 2026-10-23: a stray
    # user unit + binary next to the system k2-daemon.service).
    local existing_scope="" existing_unit="" existing_bin="" existing_ctl=""
    if [ "$svc_kind" = "systemd" ] && [ "$no_service" != "1" ]; then
        local _found
        if _found=$(_daemon_linux_unit 2>/dev/null) && [ -n "$_found" ]; then
            existing_scope="${_found%% *}"
            existing_unit="${_found#* }"
            [ "$existing_scope" = "user" ] && existing_ctl="--user "
            existing_bin=$(_daemon_unit_exec_path "$existing_scope" "$existing_unit") || existing_bin=""
            if [ -z "$existing_bin" ]; then
                echo "k2 daemon install: found the $existing_scope unit $existing_unit but could not read its ExecStart= binary." >&2
                echo "  Check it with: systemctl ${existing_ctl}cat $existing_unit" >&2
                return 1
            fi
            if [ "$bin_dir_given" = "1" ] && [ "$(dirname "$existing_bin")" != "$bin_dir" ]; then
                echo "Note: --bin-dir $bin_dir is ignored — the installed $existing_scope unit $existing_unit runs $existing_bin, which is updated in place."
            fi
            bin_path="$existing_bin"
            bin_dir="$(dirname "$existing_bin")"
        fi
    fi

    if [ "$dry_run" = "1" ]; then
        echo "k2 daemon install — DRY RUN (nothing downloaded or written)"
        echo "  platform key:   $platform"
        echo "  manifest url:   $resolved_manifest_url"
        echo "  manifest ver:   ${mver:-<unknown>}"
        echo "  requested ver:  ${version:-<latest>}"
        echo "  binary url:     $url"
        echo "  sha256:         $sha256"
        echo "  sig2 (minisign): ${sig%% *}..."
        echo "  install path:   $bin_path"
        echo "  minisign verify: minisign -V -P <K2 update key> (file:$asset version:$mver)"
        if [ "$no_service" = "1" ]; then
            echo "  service:        (skipped — --no-service)"
        elif [ -n "$existing_unit" ]; then
            echo "  service unit:   installed $existing_scope unit $existing_unit (kept; binary updated in place)"
            echo "  restart cmd:    systemctl ${existing_ctl}restart $existing_unit"
        else
            echo "  service unit:   $svc_kind → $svc_path"
            if [ "$svc_kind" = "systemd" ]; then
                echo "  enable cmds:    systemctl --user daemon-reload && systemctl --user enable --now k2-daemon"
            else
                echo "  enable cmds:    launchctl bootstrap gui/\$(id -u) $svc_path"
            fi
        fi
        return 0
    fi

    # ── Real install ─────────────────────────────────────────────────
    # Minisign is MANDATORY. Never install an unverified binary.
    if ! command -v minisign >/dev/null 2>&1; then
        echo "minisign is required to verify the daemon binary, but it was not found." >&2
        echo "Install it and re-run:" >&2
        echo "  macOS:  brew install minisign" >&2
        echo "  Debian: sudo apt-get install minisign" >&2
        echo "  Other:  https://jedisct1.github.io/minisign/" >&2
        echo "Refusing to install an unverified binary." >&2
        return 1
    fi

    local tmp
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/k2-daemon-install.XXXXXX") || {
        echo "Failed to create temp dir." >&2; return 1
    }
    # shellcheck disable=SC2064
    trap "rm -rf '$tmp'" EXIT

    local dl_bin="$tmp/k2-daemon" dl_sig="$tmp/k2-daemon.minisig"

    echo "Downloading daemon binary ($platform, manifest ver ${mver:-?})..."
    case "$url" in
        file://*) cp "${url#file://}" "$dl_bin" ;;
        *) curl -fsSL "$url" -o "$dl_bin" ;;
    esac || { echo "Failed to download binary: $url" >&2; return 1; }

    # `sig2` is a URL to the .sig2 asset (raw minisig text); a local
    # manifest may inline it. Base64-wrapped text is accepted too.
    case "$sig" in
        http://*|https://*) curl -fsSL "$sig" -o "$dl_sig" \
            || { echo "Failed to download signature: $sig" >&2; return 1; } ;;
        file://*) cp "${sig#file://}" "$dl_sig" ;;
        *) printf '%s\n' "$sig" > "$dl_sig" ;;
    esac
    if ! head -c 17 "$dl_sig" | grep -q "untrusted comment"; then
        if base64 -d "$dl_sig" > "$dl_sig.dec" 2>/dev/null \
            || base64 -D "$dl_sig" > "$dl_sig.dec" 2>/dev/null \
            || base64 -D -i "$dl_sig" -o "$dl_sig.dec" 2>/dev/null; then
            mv "$dl_sig.dec" "$dl_sig"
        else
            rm -f "$dl_sig.dec"
            echo "Signature is not a minisig and base64-decode failed." >&2
            return 1
        fi
    fi

    echo "Verifying minisign signature (K2 update keys)..."
    if ! _daemon_verify_update_sig "$dl_bin" "$dl_sig" "$asset" "$mver"; then
        echo "MINISIGN VERIFICATION FAILED for $url" >&2
        echo "Refusing to install. The binary may be corrupt or tampered with." >&2
        return 1
    fi

    echo "Verifying sha256..."
    local got_sha
    got_sha=$(_daemon_sha256 "$dl_bin") || return 1
    if [ "$got_sha" != "$sha256" ]; then
        echo "SHA256 MISMATCH for $url" >&2
        echo "  expected: $sha256" >&2
        echo "  got:      $got_sha" >&2
        echo "Refusing to install." >&2
        return 1
    fi

    echo "Verified. Installing to $bin_path ..."
    mkdir -p "$bin_dir"
    # Atomic rename — plain cp over a RUNNING daemon binary fails with
    # "Text file busy"; rename swaps the inode and re-runs converge.
    cp "$dl_bin" "$bin_path.new"
    chmod +x "$bin_path.new"
    mv -f "$bin_path.new" "$bin_path"

    if [ -n "$existing_unit" ]; then
        # The installed unit keeps its own definition; only its binary
        # changed. A plain restart picks the new binary up.
        echo "Restarting the $existing_scope unit $existing_unit ..."
        if _daemon_unit_restart "$existing_scope" "$existing_unit"; then
            echo "Restarted $existing_unit (now running $bin_path)."
        else
            echo "Installed $bin_path, but could not restart the $existing_scope unit $existing_unit." >&2
            if [ "$existing_scope" = "system" ]; then
                echo "Restart it as root: sudo systemctl restart $existing_unit" >&2
            else
                echo "Restart it with: systemctl --user restart $existing_unit" >&2
            fi
            return 1
        fi
    elif [ "$no_service" != "1" ]; then
        if [ "$svc_kind" = "systemd" ]; then
            mkdir -p "$(dirname "$svc_path")"
            # P82: the unit is k2-daemon.service (what the enable hint and
            # `k2 daemon stop|restart|status` use).
            cat > "$svc_path" <<EOF
[Unit]
Description=K2 standalone daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$bin_path
Restart=always
RestartSec=2
# Agents run in this unit's cgroup. The default OOMPolicy=stop turns one
# OOM-killed agent into a stop + restart of the whole daemon; continue lets
# only that process die. (The stage H drop-in carries the same line.)
OOMPolicy=continue
# KillMode=process (k2-ptyd, prd-k2-ptyd-v1 P87): a stop, restart or crash
# signals only the daemon. k2-ptyd, the agents it hosts, frpc and Apps live
# on in the unit's cgroup and the next daemon adopts them; an explicit stop
# ends the agents itself (the daemon reads the systemd stop job). With
# control-group a restart would kill every agent. Runtime stage H's drop-in
# (<unit>.d/k2-handoff.conf) carries the same line, plus NotifyAccess=main
# and ExecReload=SIGUSR2 for the no-downtime systemctl reload.
KillMode=process
# A hand-off successor (systemctl reload) is not systemd's child: systemd
# can't wait for it and would SIGKILL it at stop/restart, skipping its
# graceful shutdown (k2-ptyd: a stop must end agents, a restart keeps
# them). systemd waits for its own ExecStop: SIGTERM the main process
# and wait until it is gone. The daemon hands off only with this line.
ExecStop=/bin/sh -c 'kill -TERM "\$\$MAINPID" 2>/dev/null; while kill -0 "\$\$MAINPID" 2>/dev/null; do sleep 0.2; done' k2-wait-mainpid
# The unit's own runtime dir (/run/k2; XDG_RUNTIME_DIR/k2 for a user unit)
# holds frpc.toml and agents' secret files: systemd-logind deletes
# /run/user/<uid> and the user's /dev/shm files when their last login
# session ends. Preserve keeps it across restarts (frpc and agents outlive
# the daemon process). (The stage H drop-in carries the same lines.)
RuntimeDirectory=k2
RuntimeDirectoryMode=0700
RuntimeDirectoryPreserve=yes

[Install]
WantedBy=default.target
EOF
            echo "Wrote systemd user unit: $svc_path"
            echo "Enable + start it with:"
            echo "  systemctl --user daemon-reload && systemctl --user enable --now k2-daemon"
        else
            mkdir -p "$(dirname "$svc_path")"
            cat > "$svc_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${_daemon_label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>$bin_path</string>
    </array>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$HOME/.k2/daemon.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>$HOME/.k2/daemon.stderr.log</string>
</dict>
</plist>
EOF
            echo "Wrote launchd plist: $svc_path"
            echo "Bootstrap + start it with:"
            echo "  launchctl bootstrap gui/\$(id -u) $svc_path"
        fi
    fi

    echo ""
    echo "Standalone K2 daemon installed: $bin_path"
    echo "NOTE: pairing this headless box to a K2 Connect account from the"
    echo "      CLI is a follow-up — token bootstrap is still an open PRD"
    echo "      question, so this installer does NOT pair the daemon. Once"
    echo "      the daemon is running, pair it via the documented K2 Connect"
    echo "      flow."
    return 0
}

cmd_reviews() {
    cli_request "/cli/reviews" | python3 -c "
import json, sys
data = json.loads(sys.stdin.read())
if not data:
    print('No pending reviews.')
    sys.exit(0)
for r in data:
    agent = r.get('agentName', '?')
    branch = r.get('branch', '')
    items = len(r.get('workItems', []))
    files = len(r.get('diffSummary', []))
    print(f'  {agent:<20} {branch:<30} {items} items, {files} files changed')
" 2>/dev/null || cat
}

cmd_review_approve() {
    local agent="${1:-}" branch="${2:-}"
    if [ -z "$agent" ] || [ -z "$branch" ]; then
        echo "Usage: k2 review approve <agent> <branch>" >&2
        exit 1
    fi
    cli_request_post "/cli/review/approve" "agent=$(urlencode "$agent")&branch=$(urlencode "$branch")"
}

cmd_review_reject() {
    local agent="${1:-}" reason=""
    shift 2>/dev/null
    while [ $# -gt 0 ]; do
        case "$1" in
            --reason) reason="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    if [ -z "$agent" ]; then
        echo "Usage: k2 review reject <agent> [--reason \"...\"]" >&2
        exit 1
    fi
    local params="agent=$(urlencode "$agent")"
    [ -n "$reason" ] && params="${params}&reason=$(urlencode "$reason")"
    cli_request_post "/cli/review/reject" "$params"
}

cmd_review_feedback() {
    local agent="${1:-}" feedback=""
    shift 2>/dev/null
    while [ $# -gt 0 ]; do
        case "$1" in
            --message|-m) feedback="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    if [ -z "$agent" ] || [ -z "$feedback" ]; then
        echo "Usage: k2 review feedback <agent> --message \"...\"" >&2
        exit 1
    fi
    cli_request_post "/cli/review/feedback" "agent=$(urlencode "$agent")&feedback=$(urlencode "$feedback")"
}

# 0.39.0f Phase 2.1b: cmd_agents_running / cmd_agents_reap removed.
# Use `k2 workspace list --running` / `k2 daemon reap` instead.

# GH#8 #3 — resolve a workspace NAME (or path) to a LIVE v2 session
# UUID for the /cli/terminal/write route, which only accepts a session
# UUID (the daemon's handle_write parses `id` as a UUID and nothing
# else). This is the write-side complement to how `k2 read` / `k2 talk`
# address a peer by name: the daemon resolves `workspace=` for READS,
# but there is no `workspace=` form for WRITES, so we resolve here, in
# the CLI, with no daemon change.
#
# Back-compat: if the arg ALREADY looks like a session UUID we echo it
# verbatim — existing `terminal write <uuid> "text"` callers are
# unaffected (byte-identical request).
#
# Name resolution mirrors the existing patterns in this script:
#   1. name/path -> canonical project path  (/cli/workspace/resolve, as
#      cmd_workspace_resume_chat_args does)
#   2. project path -> live v2 session UUID  (/cli/sessions/list-for-workspace,
#      as cmd_sessions_live does)
# Best-effort selection: prefer a session matching `--agent <name>`,
# else the workspace's sole/first live v2 session. On no live session it
# prints a hint to stderr and returns non-zero (same failure shape as
# `k2 read`). Echoes the resolved UUID on stdout.
resolve_write_session_id() {
    local target="$1"
    local agent="${2:-}"
    # UUID back-compat (8-4-4-4-12 hex) — use the arg directly.
    if printf '%s' "$target" | grep -qiE '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'; then
        printf '%s' "$target"
        return 0
    fi
    # Step 1: workspace name/path -> canonical project path.
    local resolved_path
    resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
        | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
    [ -z "$resolved_path" ] && resolved_path="$target"
    # Step 2: project path -> live v2 session UUID.
    local sid
    sid=$(cli_request "/cli/sessions/list-for-workspace" "path=$(urlencode "$resolved_path")" 2>/dev/null \
        | python3 -c "
import json, sys
agent = '''$agent'''
try:
    rows = json.loads(sys.stdin.read())
except Exception:
    rows = []
if not isinstance(rows, list):
    rows = []
v2 = [r for r in rows if r.get('isV2')]
pool = v2 if v2 else rows
if agent:
    matched = [r for r in pool if r.get('agentName') == agent]
    pool = matched if matched else pool
print(pool[0].get('sessionId', '') if pool else '')
" 2>/dev/null)
    if [ -z "$sid" ]; then
        echo "Error: no live session for workspace '$target'${agent:+ agent '$agent'} — it may be asleep (check 'k2 sessions live $target')" >&2
        return 1
    fi
    printf '%s' "$sid"
}

cmd_terminal_write() {
    # GH#8 #3: the first arg is a session UUID (as before) OR a workspace
    # NAME. A name is resolved to the workspace's live session via
    # resolve_write_session_id (the write-side mirror of `k2 read`'s
    # name-addressing), then posted to /cli/terminal/write. UUID callers
    # are byte-identical.
    local target="${1:-}"
    shift || true
    # Optional leading `--agent <name>` selects a specific agent's session
    # within the workspace. Consumed only when it's the very next token,
    # so message text after it is untouched (and the historical
    # `terminal write <id> "text"` form keeps `message="$*"` exactly).
    local agent=""
    if [ "${1:-}" = "--agent" ]; then
        agent="${2:-}"; shift 2 || true
    fi
    local message="$*"
    if [ -z "$target" ] || [ -z "$message" ]; then
        echo "Usage: k2 terminal write <terminal-id|workspace> [--agent <name>] \"message\"" >&2
        exit 1
    fi
    local terminal_id
    terminal_id=$(resolve_write_session_id "$target" "$agent") || exit 1
    cli_request_post "/cli/terminal/write" "id=$(urlencode "$terminal_id")&message=$(urlencode "$message")"
}

cmd_terminal_read() {
    local terminal_id="${1:-}"
    local lines=50
    shift || true
    while [ $# -gt 0 ]; do
        case "$1" in
            --lines) lines="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    if [ -z "$terminal_id" ]; then
        echo "Usage: k2 terminal read <terminal-id> [--lines N]" >&2
        exit 1
    fi
    cli_request "/cli/terminal/read" "id=$(urlencode "$terminal_id")&lines=$lines" | python3 -c "
import json, sys
data = json.loads(sys.stdin.read())
for line in data.get('lines', []):
    print(line)
" 2>/dev/null || cli_request "/cli/terminal/read" "id=$(urlencode "$terminal_id")&lines=$lines"
}

# 0.39.x: read a workspace's live terminal by NAME — the read
# complement to `msg` (talk live) and `inbox` (mail). Resolves the
# workspace name daemon-side to its canonical (primary / coordinator)
# session, or a specific agent's session with `--agent <name>`. Prints
# the last N lines of plain text (default 50), one per line. Use this
# to peek another agent before injecting (human-in-the-loop) or to
# diagnose a stuck agent.
cmd_read() {
    local workspace="${1:-}"
    local lines=50
    local agent=""
    local session=""
    local token=""
    shift || true
    while [ $# -gt 0 ]; do
        case "$1" in
            --lines) lines="$2"; shift 2 ;;
            --agent) agent="$2"; shift 2 ;;
            --session) session="$2"; shift 2 ;;
            --token) token="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    if [ -z "$workspace" ] && [ -z "$session" ]; then
        echo "Usage: k2 read <workspace>[@<host>] [--lines N] [--agent <name>] [--session <id>] [--token <tok>]" >&2
        exit 1
    fi
    # Remote addressing (K2-as-a-server): <workspace>@<host> reads a
    # workspace on a CONNECTED remote daemon (owner token over the tunnel).
    # Splits off @host and repoints BASE_URL/TOKEN; local target = no-op.
    _split_remote_target "$workspace"; workspace="$_BARE_TARGET"
    _apply_remote_host "$_REMOTE_HOST" "$token" || exit 1
    local query
    if [ -n "$session" ]; then
        # Address a specific session directly (the route accepts session=).
        query="session=$(urlencode "$session")&lines=$lines"
    else
        query="workspace=$(urlencode "$workspace")&lines=$lines"
        if [ -n "$agent" ]; then
            query="$query&agent=$(urlencode "$agent")"
        fi
    fi
    # Prefer python to unwrap {"lines":[...]} into one line per row and
    # surface daemon {"error":...} to stderr with a non-zero exit (so
    # agents scripting `k2 read` can detect failure). C2: not_connected /
    # gated / forbidden → exit 3 with mail/dns teaching shape.
    # Fall back to the raw daemon JSON only when python3 is genuinely
    # unavailable.
    if command -v python3 >/dev/null 2>&1; then
        # Prefer UDS+scoped when available (C2 peer gate sees principal).
        cli_send cli_request "/cli/terminal/read" "$query" | python3 -c "
import json, sys
raw = sys.stdin.read()
try:
    data = json.loads(raw)
except Exception:
    sys.stderr.write(raw[:200] + '\n')
    sys.exit(1)
if isinstance(data, dict) and 'error' in data:
    err = data['error']
    if isinstance(err, dict):
        code = err.get('code') or 'error'
        hint = err.get('hint') or code
        sys.stderr.write(json.dumps({'error': {'code': code, 'hint': hint}},
                                    ensure_ascii=False, separators=(',', ':')) + '\n')
        sys.exit(3 if code in ('not_connected', 'gated', 'forbidden') else 1)
    sys.stderr.write(str(err) + '\n')
    sys.exit(1)
for line in data.get('lines', []):
    print(line)
"
    else
        cli_send cli_request "/cli/terminal/read" "$query"
    fi
}

