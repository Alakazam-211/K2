#!/bin/sh
# K2 standalone-daemon headless installer (P2 / #614).
#
# One-liner for a fresh, GUI-less box:
#
#   curl -fsSL https://github.com/Alakazam-211/K2/releases/latest/download/install-daemon.sh | sh
#
# Fetches the per-OS standalone daemon binary from a signed GitHub
# release, MANDATORY-minisign-verifies it against the SAME pubkey the
# Tauri app updater uses (embedded literal below), checks sha256, drops
# it into a bin dir, and writes a supervisor unit (systemd on Linux,
# launchd on macOS) so a crash respawns the daemon.
#
# This is the self-contained twin of `k2 daemon install`. Keep the
# two in sync (pubkey, platform detect, verify, service templates).
#
# POSIX sh. No bash-isms.
set -eu

# Honor `set -o pipefail` where the shell supports it (bash/ksh/zsh);
# plain POSIX sh (dash) doesn't, so guard it so `set -e` doesn't trip.
( set -o pipefail ) 2>/dev/null && set -o pipefail || true

# ── Config (overridable via env) ─────────────────────────────────────
# Minisign verify pubkey — literal from plugins.updater.pubkey in
# src-tauri/tauri.conf.json. Rotate here AND in cli/k2 if the updater
# key ever rotates.
K2_DAEMON_PUBKEY="dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEU5MTExNDQ2RjY1RUJCMDUKUldRRnUxNzJSaFFSNlFCcXptaWoyRTlidERHaERXbXBkSCthaDEvTTRQbXVIUElOVVd2S0xmNm8K"

# K2_* is canonical; K2SO_* still honored through the 0.x transition.
K2_VERSION="${K2_VERSION:-${K2SO_VERSION:-}}"
K2_BIN_DIR="${K2_BIN_DIR:-${K2SO_BIN_DIR:-$HOME/.local/bin}}"
K2_MANIFEST_URL="${K2_MANIFEST_URL:-${K2SO_MANIFEST_URL:-}}"
K2_NO_SERVICE="${K2_NO_SERVICE:-${K2SO_NO_SERVICE:-0}}"
K2_DRY_RUN="${K2_DRY_RUN:-${K2SO_DRY_RUN:-0}}"
K2_DAEMON_LABEL="dev.k2.daemon"

DEFAULT_LATEST_URL="https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json"

usage() {
    cat >&2 <<'EOF'
Usage: install-daemon.sh [--version x.y.z] [--bin-dir DIR]
                         [--manifest-url URL] [--no-service] [--dry-run]

Install the STANDALONE (headless, no-GUI) K2 daemon from a signed
GitHub release, minisign-verified against the embedded updater pubkey.

Flags (or matching env vars K2_VERSION / K2_BIN_DIR /
K2_MANIFEST_URL / K2_NO_SERVICE=1 / K2_DRY_RUN=1; legacy K2SO_*
twins still honored):
  --version       install a specific release tag (default: latest)
  --bin-dir       install dir (default: ~/.local/bin)
  --manifest-url  override daemon-latest.json URL (supports file://)
  --no-service    skip the systemd/launchd supervisor unit
  --dry-run       print the plan; download/install nothing
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)      K2_VERSION="${2:-}"; shift 2 ;;
        --bin-dir)      K2_BIN_DIR="${2:-}"; shift 2 ;;
        --manifest-url) K2_MANIFEST_URL="${2:-}"; shift 2 ;;
        --no-service)   K2_NO_SERVICE=1; shift ;;
        --dry-run)      K2_DRY_RUN=1; shift ;;
        -h|--help)      usage; exit 0 ;;
        *) echo "install-daemon.sh: unknown argument: $1" >&2; usage; exit 2 ;;
    esac
done

die() { echo "$@" >&2; exit 1; }

# ── Platform detect ──────────────────────────────────────────────────
detect_platform() {
    _os=""; _arch=""
    case "$(uname -s)" in
        Darwin) _os="macos" ;;
        Linux)  _os="linux" ;;
        *) die "Unsupported OS: $(uname -s) (expected Darwin or Linux)" ;;
    esac
    case "$(uname -m)" in
        arm64|aarch64) _arch="aarch64" ;;
        x86_64|amd64)  _arch="x86_64" ;;
        *) die "Unsupported CPU arch: $(uname -m) (expected arm64/aarch64 or x86_64)" ;;
    esac
    printf '%s-%s\n' "$_os" "$_arch"
}

# Extract one field for a platform key from a manifest blob.
# Args: <json> <platform-key> <field>  (field ∈ url|sig|sha256)
# Pure sed — isolate the platform object first so sibling fields can't
# bleed across.
manifest_field() {
    _json="$1"; _key="$2"; _field="$3"
    _obj=$(printf '%s' "$_json" | tr '\n' ' ' \
        | sed -n "s/.*\"${_key}\"[[:space:]]*:[[:space:]]*{\([^}]*\)}.*/\1/p")
    [ -n "$_obj" ] || return 1
    printf '%s' "$_obj" \
        | sed -n "s/.*\"${_field}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p"
}

manifest_version() {
    printf '%s' "$1" | tr '\n' ' ' \
        | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p'
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "Neither sha256sum nor shasum found — cannot verify checksum."
    fi
}

fetch() {
    # fetch <url> <dest-file>  (dest "-" = stdout)
    _u="$1"; _d="$2"
    case "$_u" in
        file://*)
            _p="${_u#file://}"
            [ -f "$_p" ] || die "File not found: $_p"
            if [ "$_d" = "-" ]; then cat "$_p"; else cp "$_p" "$_d"; fi
            ;;
        *)
            command -v curl >/dev/null 2>&1 || die "curl is required to fetch from the network."
            if [ "$_d" = "-" ]; then
                curl -fsSL "$_u" || die "Failed to fetch: $_u"
            else
                curl -fsSL "$_u" -o "$_d" || die "Failed to fetch: $_u"
            fi
            ;;
    esac
}

# ── Resolve manifest URL ─────────────────────────────────────────────
if [ -n "$K2_MANIFEST_URL" ]; then
    MANIFEST_URL="$K2_MANIFEST_URL"
elif [ -n "$K2_VERSION" ]; then
    MANIFEST_URL="https://github.com/Alakazam-211/K2/releases/download/v${K2_VERSION}/daemon-latest.json"
else
    MANIFEST_URL="$DEFAULT_LATEST_URL"
fi

PLATFORM=$(detect_platform)
MANIFEST=$(fetch "$MANIFEST_URL" -)

MVER=$(manifest_version "$MANIFEST")
URL=$(manifest_field "$MANIFEST" "$PLATFORM" "url" || true)
SIG=$(manifest_field "$MANIFEST" "$PLATFORM" "sig" || true)
SHA256=$(manifest_field "$MANIFEST" "$PLATFORM" "sha256" || true)

if [ -z "$URL" ] || [ -z "$SIG" ] || [ -z "$SHA256" ]; then
    echo "Manifest has no complete artifact for platform '$PLATFORM'." >&2
    echo "  (manifest version: ${MVER:-<unknown>})" >&2
    exit 1
fi

BIN_PATH="$K2_BIN_DIR/k2-daemon"

if [ "$(uname -s)" = "Darwin" ]; then
    SVC_KIND="launchd"
    SVC_PATH="$HOME/Library/LaunchAgents/${K2_DAEMON_LABEL}.plist"
else
    SVC_KIND="systemd"
    SVC_PATH="$HOME/.config/systemd/user/k2-daemon.service"
fi

# ── Dry run ──────────────────────────────────────────────────────────
if [ "$K2_DRY_RUN" = "1" ]; then
    echo "install-daemon.sh — DRY RUN (nothing downloaded or written)"
    echo "  platform key:   $PLATFORM"
    echo "  manifest url:   $MANIFEST_URL"
    echo "  manifest ver:   ${MVER:-<unknown>}"
    echo "  requested ver:  ${K2_VERSION:-<latest>}"
    echo "  binary url:     $URL"
    echo "  sha256:         $SHA256"
    echo "  sig (minisign): $(printf '%s' "$SIG" | cut -c1-24)..."
    echo "  install path:   $BIN_PATH"
    echo "  minisign verify: minisign -Vm <downloaded-bin> -P <embedded-pubkey>"
    if [ "$K2_NO_SERVICE" = "1" ]; then
        echo "  service:        (skipped — --no-service)"
    else
        echo "  service unit:   $SVC_KIND → $SVC_PATH"
        if [ "$SVC_KIND" = "systemd" ]; then
            echo "  enable cmds:    systemctl --user daemon-reload && systemctl --user enable --now k2-daemon"
        else
            echo "  enable cmds:    launchctl bootstrap gui/\$(id -u) $SVC_PATH"
        fi
    fi
    exit 0
fi

# ── Real install — minisign is MANDATORY ─────────────────────────────
if ! command -v minisign >/dev/null 2>&1; then
    echo "minisign is required to verify the daemon binary, but it was not found." >&2
    echo "Install it and re-run:" >&2
    echo "  macOS:  brew install minisign" >&2
    echo "  Debian: sudo apt-get install minisign" >&2
    echo "  Other:  https://jedisct1.github.io/minisign/" >&2
    die "Refusing to install an unverified binary."
fi

TMP=$(mktemp -d "${TMPDIR:-/tmp}/k2-daemon-install.XXXXXX") || die "Failed to create temp dir."
trap 'rm -rf "$TMP"' EXIT INT TERM

DL_BIN="$TMP/k2-daemon"
DL_SIG="$TMP/k2-daemon.minisig"

echo "Downloading daemon binary ($PLATFORM, manifest ver ${MVER:-?})..."
fetch "$URL" "$DL_BIN"

printf '%s\n' "$SIG" > "$DL_SIG"

echo "Verifying minisign signature..."
if ! minisign -Vm "$DL_BIN" -P "$K2_DAEMON_PUBKEY" -x "$DL_SIG" >/dev/null 2>&1; then
    echo "MINISIGN VERIFICATION FAILED for $URL" >&2
    die "Refusing to install. The binary may be corrupt or tampered with."
fi

echo "Verifying sha256..."
GOT_SHA=$(sha256_of "$DL_BIN")
if [ "$GOT_SHA" != "$SHA256" ]; then
    echo "SHA256 MISMATCH for $URL" >&2
    echo "  expected: $SHA256" >&2
    echo "  got:      $GOT_SHA" >&2
    die "Refusing to install."
fi

echo "Verified. Installing to $BIN_PATH ..."
mkdir -p "$K2_BIN_DIR"
cp "$DL_BIN" "$BIN_PATH"
chmod +x "$BIN_PATH"

if [ "$K2_NO_SERVICE" != "1" ]; then
    if [ "$SVC_KIND" = "systemd" ]; then
        mkdir -p "$(dirname "$SVC_PATH")"
        cat > "$SVC_PATH" <<EOF
[Unit]
Description=K2 standalone daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$BIN_PATH
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
EOF
        echo "Wrote systemd user unit: $SVC_PATH"
        echo "Enable + start it with:"
        echo "  systemctl --user daemon-reload && systemctl --user enable --now k2-daemon"
    else
        mkdir -p "$(dirname "$SVC_PATH")"
        cat > "$SVC_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${K2_DAEMON_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BIN_PATH</string>
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
        echo "Wrote launchd plist: $SVC_PATH"
        echo "Bootstrap + start it with:"
        echo "  launchctl bootstrap gui/\$(id -u) $SVC_PATH"
    fi
fi

echo ""
echo "Standalone K2 daemon installed: $BIN_PATH"
echo "NOTE: pairing this headless box to a K2 Connect account from the CLI"
echo "      is a follow-up — token bootstrap is still an open PRD question,"
echo "      so this installer does NOT pair the daemon. Pair it via the"
echo "      documented K2 Connect flow once the daemon is running."
