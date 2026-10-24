#!/bin/sh
# K2 standalone-daemon headless installer (P2 / #614).
#
# One-liner for a fresh, GUI-less box:
#
#   curl -fsSL https://github.com/Alakazam-211/K2/releases/latest/download/install-daemon.sh | sh
#
# Fetches the per-OS standalone daemon binary from a signed GitHub
# release, MANDATORY-minisign-verifies its .sig2 with the K2 update keys
# (embedded below; the signed comment must name this asset and version),
# checks sha256, drops it into a bin dir, and writes a supervisor unit
# (systemd on Linux, launchd on macOS) so a crash respawns the daemon.
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
# The K2 update keys (prd-security-sweep-0464-v1 SS4): minisign key lines,
# U2 (primary) then U3 (offline backup), space-separated. A copy of
# k2_release_keys::UPDATER_KEYS (crates/k2-release-keys); a test fails when
# the copies drift. Written only by scripts/updater-key-ceremony.sh pubkeys.
# Empty until the key ceremony lands: then this installer refuses every
# binary. Not overridable from the environment.
K2_DAEMON_PUBKEYS=""
# Releases before this carry no .sig2 (signed with the retired key only).
K2_FIRST_U2_VERSION="0.46.4"

# K2_* is canonical; K2SO_* still honored through the 0.x transition.
K2_VERSION="${K2_VERSION:-${K2SO_VERSION:-}}"
K2_BIN_DIR="${K2_BIN_DIR:-${K2SO_BIN_DIR:-$HOME/.local/bin}}"
K2_MANIFEST_URL="${K2_MANIFEST_URL:-${K2SO_MANIFEST_URL:-}}"
K2_NO_SERVICE="${K2_NO_SERVICE:-${K2SO_NO_SERVICE:-0}}"
K2_DRY_RUN="${K2_DRY_RUN:-${K2SO_DRY_RUN:-0}}"
K2_DAEMON_LABEL="dev.k2.daemon"
# The App engine the root helper installed (0.46.4, update continuity R3):
# when it exists, the matching engine is staged beside the daemon for the
# boot heal. Overridable for tests only.
K2_APP_ENGINE_PATH="${K2_APP_ENGINE_PATH:-/usr/local/libexec/k2-app-engine}"

DEFAULT_LATEST_URL="https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json"

usage() {
    cat >&2 <<'EOF'
Usage: install-daemon.sh [--version x.y.z] [--bin-dir DIR]
                         [--manifest-url URL] [--no-service] [--dry-run]

Install the STANDALONE (headless, no-GUI) K2 daemon from a signed
GitHub release, minisign-verified with the embedded K2 update keys
(0.46.4 and newer).

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

# version_lt <a> <b>: a is an older x.y.z than b (unparsable counts as older).
version_lt() {
    awk -v a="$1" -v b="$2" 'BEGIN {
        if (a !~ /^v?[0-9]+\.[0-9]+\.[0-9]+$/ || b !~ /^v?[0-9]+\.[0-9]+\.[0-9]+$/) exit 0
        sub(/^v/, "", a); sub(/^v/, "", b)
        split(a, x, "."); split(b, y, ".")
        for (i = 1; i <= 3; i++) { if (x[i] + 0 < y[i] + 0) exit 0; if (x[i] + 0 > y[i] + 0) exit 1 }
        exit 1
    }'
}

retired_key_release() {
    die "Versions before $K2_FIRST_U2_VERSION are signed with the retired update key, which this installer doesn't trust. Install $K2_FIRST_U2_VERSION or newer (or use an older install-daemon.sh)."
}

# verify_update_sig <binary> <minisig> <asset> <version>: one of the K2
# update keys signed it, and the signed trusted comment names exactly this
# asset (file:) and this version (version:).
verify_update_sig() {
    _vb="$1"; _vs="$2"; _va="$3"; _vv="$4"
    [ -n "$K2_DAEMON_PUBKEYS" ] || die "This installer trusts no update key yet (the update-key ceremony has not landed). Refusing."
    for _k in $K2_DAEMON_PUBKEYS; do
        if _tc=$(minisign -V -P "$_k" -m "$_vb" -x "$_vs" -Q 2>/dev/null); then
            _files=$(printf '%s\n' "$_tc" | tr '\t' '\n' | grep '^file:' || true)
            _vers=$(printf '%s\n' "$_tc" | tr '\t' '\n' | grep '^version:' || true)
            [ "$_files" = "file:$_va" ] || die "The signature is for '${_files#file:}', not $_va. Refusing."
            [ "$_vers" = "version:$_vv" ] || die "The signature is for version '${_vers#version:}', not $_vv. Refusing."
            return 0
        fi
    done
    return 1
}

# ── Resolve manifest URL ─────────────────────────────────────────────
if [ -n "$K2_MANIFEST_URL" ]; then
    MANIFEST_URL="$K2_MANIFEST_URL"
elif [ -n "$K2_VERSION" ]; then
    version_lt "$K2_VERSION" "$K2_FIRST_U2_VERSION" && retired_key_release
    MANIFEST_URL="https://github.com/Alakazam-211/K2/releases/download/v${K2_VERSION}/daemon-latest.json"
else
    MANIFEST_URL="$DEFAULT_LATEST_URL"
fi

PLATFORM=$(detect_platform)
MANIFEST=$(fetch "$MANIFEST_URL" -)

MVER=$(manifest_version "$MANIFEST")
URL=$(manifest_field "$MANIFEST" "$PLATFORM" "url" || true)
SIG2=$(manifest_field "$MANIFEST" "$PLATFORM" "sig2" || true)
SHA256=$(manifest_field "$MANIFEST" "$PLATFORM" "sha256" || true)

if [ -z "$URL" ] || [ -z "$SHA256" ] || [ -z "$MVER" ]; then
    echo "Manifest has no complete artifact for platform '$PLATFORM'." >&2
    echo "  (manifest version: ${MVER:-<unknown>})" >&2
    exit 1
fi
# Only the K2 update keys' signature (.sig2) counts; the old `sig` field is
# for installs older than 0.46.4.
if [ -z "$SIG2" ] || version_lt "$MVER" "$K2_FIRST_U2_VERSION"; then
    retired_key_release
fi
if [ -n "$K2_VERSION" ] && [ "$MVER" != "${K2_VERSION#v}" ]; then
    die "The manifest is for $MVER, not the requested $K2_VERSION. Refusing."
fi
DAEMON_ASSET="k2-daemon-$PLATFORM"

# The App engine (Linux, only when one is installed): its OWN manifest key
# `k2-app-engine-<platform>` (never under the platform key, A1). Staged in
# ~/.k2/update/engine/<version>/ with the asset names the daemon reads; the
# signature may not be published yet (A17): the daemon and the root helper
# verify it, and the boot heal fetches a missing one.
ENGINE_KEY="k2-app-engine-$PLATFORM"
ENGINE_URL=""; ENGINE_SIG=""; ENGINE_SHA=""
ENGINE_STAGE_DIR="$HOME/.k2/update/engine/${MVER:-unknown}"
if [ "${PLATFORM%%-*}" = "linux" ] && [ -f "$K2_APP_ENGINE_PATH" ]; then
    ENGINE_URL=$(manifest_field "$MANIFEST" "$ENGINE_KEY" "url" || true)
    ENGINE_SIG=$(manifest_field "$MANIFEST" "$ENGINE_KEY" "sig" || true)
    ENGINE_SHA=$(manifest_field "$MANIFEST" "$ENGINE_KEY" "sha256" || true)
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
    echo "  sig2 (minisign): $(printf '%s' "$SIG2" | cut -c1-24)..."
    echo "  install path:   $BIN_PATH"
    echo "  minisign verify: minisign -V -P <K2 update key> (file:$DAEMON_ASSET version:$MVER)"
    if [ -n "$ENGINE_URL" ] && [ -n "$ENGINE_SHA" ]; then
        echo "  app engine:     $ENGINE_URL"
        echo "  engine sha256:  $ENGINE_SHA"
        echo "  engine stage:   $ENGINE_STAGE_DIR/$ENGINE_KEY (+ .sig when published)"
    elif [ -f "$K2_APP_ENGINE_PATH" ] && [ "${PLATFORM%%-*}" = "linux" ]; then
        echo "  app engine:     (not in this manifest; the daemon's boot heal downloads it)"
    else
        echo "  app engine:     (none installed; nothing to stage)"
    fi
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

# `sig2` is a URL to the .sig2 asset (raw minisig text); a local manifest
# may inline it. Base64-wrapped text is accepted too.
case "$SIG2" in
    http://*|https://*|file://*) fetch "$SIG2" "$DL_SIG" ;;
    *) printf '%s\n' "$SIG2" > "$DL_SIG" ;;
esac
if ! head -c 17 "$DL_SIG" | grep -q "untrusted comment"; then
    if base64 -d "$DL_SIG" > "$DL_SIG.dec" 2>/dev/null \
        || base64 -D "$DL_SIG" > "$DL_SIG.dec" 2>/dev/null \
        || base64 -D -i "$DL_SIG" -o "$DL_SIG.dec" 2>/dev/null; then
        mv "$DL_SIG.dec" "$DL_SIG"
    else
        rm -f "$DL_SIG.dec"
        die "Signature file is not a minisig and base64-decoding it failed."
    fi
fi

echo "Verifying minisign signature (K2 update keys)..."
if ! verify_update_sig "$DL_BIN" "$DL_SIG" "$DAEMON_ASSET" "$MVER"; then
    echo "MINISIGN VERIFICATION FAILED for $URL" >&2
    die "Refusing to install. The binary may be corrupt or tampered with, or not signed by a K2 update key."
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
# Atomic rename — plain cp over a RUNNING daemon binary fails with
# "Text file busy"; rename swaps the inode and re-runs converge.
cp "$DL_BIN" "$BIN_PATH.new"
chmod +x "$BIN_PATH.new"
mv -f "$BIN_PATH.new" "$BIN_PATH"

# Stage the matching App engine (R3): sha256-checked; never fatal (the
# daemon's boot heal downloads it when this didn't work).
if [ -n "$ENGINE_URL" ] && [ -n "$ENGINE_SHA" ]; then
    echo "Staging the App engine for v${MVER:-?}..."
    DL_ENG="$TMP/$ENGINE_KEY"
    if ( fetch "$ENGINE_URL" "$DL_ENG" ) 2>/dev/null && [ "$(sha256_of "$DL_ENG")" = "$ENGINE_SHA" ]; then
        mkdir -p "$ENGINE_STAGE_DIR"
        rm -f "$ENGINE_STAGE_DIR/$ENGINE_KEY.sig"
        cp "$DL_ENG" "$ENGINE_STAGE_DIR/$ENGINE_KEY.part" && mv -f "$ENGINE_STAGE_DIR/$ENGINE_KEY.part" "$ENGINE_STAGE_DIR/$ENGINE_KEY"
        if [ -n "$ENGINE_SIG" ] && ( fetch "$ENGINE_SIG" "$TMP/$ENGINE_KEY.sig" ) 2>/dev/null; then
            cp "$TMP/$ENGINE_KEY.sig" "$ENGINE_STAGE_DIR/$ENGINE_KEY.sig"
            echo "Staged the App engine and its signature in $ENGINE_STAGE_DIR"
        else
            echo "Staged the App engine in $ENGINE_STAGE_DIR (signature not published yet; the daemon fetches it)"
        fi
    else
        echo "WARNING: the App engine could not be staged (download or sha256); the daemon's boot heal downloads it." >&2
    fi
fi

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
# Linux update swaps the binary in-process before exit (0.40.43+).
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
# Agents run in this unit's cgroup. The default OOMPolicy=stop turns one
# OOM-killed agent into a stop + restart of the whole daemon; continue lets
# only that process die. (The stage H drop-in carries the same line.)
OOMPolicy=continue
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
