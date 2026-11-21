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
# Linux, 0.46.16 and newer: the k5 command's program (k5-cli-linux-<arch>)
# goes in beside the daemon, verified the same way (its .sig2, one of the
# K2 update keys, the signed comment naming that asset and version). A
# missing or bad k5 program or signature stops the install before anything
# is installed.
#
# Linux: it also installs (or updates) K2's root helper at the SAME version
# as the daemon, before the daemon binary is swapped, through
# install-root-helper.sh (the signed release asset, checked against the
# root-helper keys). That step needs root: run as root it runs directly
# (`--user <name>` installs the daemon for that user); run as a normal user
# it asks `sudo` once. If it can't, the script stops non-zero and the daemon
# is left as it was. A root helper a package manager owns is left alone.
# macOS has no root helper.
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
K2_DAEMON_PUBKEYS="RWRpK+630xTKdopKvlpWVqvnEwqyK1CZ+F5deeRjpGA2ywyWS8NnOrpv RWSkAlEaeKUBUZhQA52GoMLa0rt8KLdEoto26gLHRpm9QIC5ColsvG/Y"
# Releases before this carry no .sig2 (signed with the retired key only).
K2_FIRST_U2_VERSION="0.46.6"
# The first release whose k5 program (k5-cli-linux-<arch>) carries a .sig2:
# from it on, Linux installs need both and refuse without them. Older ones
# ship no signed k5 program; the k2 command runs the script there.
K2_FIRST_K5_VERSION="0.46.16"

# K2_* is canonical; K2SO_* still honored through the 0.x transition.
K2_VERSION="${K2_VERSION:-${K2SO_VERSION:-}}"
# Empty = ~/.local/bin of the daemon user (resolved after --user).
K2_BIN_DIR="${K2_BIN_DIR:-${K2SO_BIN_DIR:-}}"
K2_MANIFEST_URL="${K2_MANIFEST_URL:-${K2SO_MANIFEST_URL:-}}"
K2_NO_SERVICE="${K2_NO_SERVICE:-${K2SO_NO_SERVICE:-0}}"
K2_DRY_RUN="${K2_DRY_RUN:-${K2SO_DRY_RUN:-0}}"
K2_DAEMON_LABEL="dev.k2.daemon"
# The App engine the root helper installed (0.46.4, update continuity R3):
# when it exists, the matching engine is staged beside the daemon for the
# boot heal. Overridable for tests only.
K2_APP_ENGINE_PATH="${K2_APP_ENGINE_PATH:-/usr/local/libexec/k2-app-engine}"
# The root helper (Linux only; see usage). Env twins of the flags.
K2_DAEMON_USER="${K2_DAEMON_USER:-}"
K2_HELPER_UNIT="${K2_HELPER_UNIT:-}"
K2_HELPER_SCOPE="${K2_HELPER_SCOPE:-}"
K2_NO_ROOT_HELPER="${K2_NO_ROOT_HELPER:-0}"
K2_ROOT_HELPER_FROM="${K2_ROOT_HELPER_FROM:-}"
K2_ROOT_HELPER_FILE="${K2_ROOT_HELPER_FILE:-}"
# Where install-root-helper.sh is fetched from when it isn't beside this
# script (file:// works for a mirror).
RELEASE_BASE="${K2_RELEASE_BASE:-https://github.com/Alakazam-211/K2/releases/download}"
# DESTDIR-style prefix for the hermetic tests only (install-root-helper.sh
# reads the same variable). Production leaves it empty.
RH_PREFIX="${K2_INSTALL_ROOT:-}"
RH_CANON="/usr/local/libexec/k2-root-helper"

DEFAULT_LATEST_URL="https://github.com/Alakazam-211/K2/releases/latest/download/daemon-latest.json"

usage() {
    # @FIRST_U2@ is K2_FIRST_U2_VERSION (one value; the copies test pins it),
    # @FIRST_K5@ is K2_FIRST_K5_VERSION.
    sed -e "s/@FIRST_U2@/$K2_FIRST_U2_VERSION/g" -e "s/@FIRST_K5@/$K2_FIRST_K5_VERSION/g" >&2 <<'EOF'
Usage: install-daemon.sh [--version x.y.z] [--bin-dir DIR]
                         [--manifest-url URL] [--no-service] [--dry-run]
                         [--user NAME] [--unit UNIT] [--scope system|user]
                         [--root-helper-from DIR] [--root-helper-file PATH]
                         [--no-root-helper]

Install the STANDALONE (headless, no-GUI) K2 daemon from a signed
GitHub release, minisign-verified with the embedded K2 update keys
(@FIRST_U2@ and newer).

Flags (or matching env vars K2_VERSION / K2_BIN_DIR /
K2_MANIFEST_URL / K2_NO_SERVICE=1 / K2_DRY_RUN=1; legacy K2SO_*
twins still honored):
  --version       install a specific release tag (default: latest);
                  @FIRST_U2@ or newer. An OLDER release than the one installed
                  is allowed here on purpose (a local, explicit action); the
                  daemon's own self-update refuses a downgrade unless the
                  owner asks for it.
  --bin-dir       install dir (default: ~/.local/bin)
  --manifest-url  override daemon-latest.json URL (supports file://)
  --no-service    skip the systemd/launchd supervisor unit
  --dry-run       print the plan; download/install nothing

Linux, @FIRST_K5@ and newer: the k5 command's program goes in beside the
daemon, verified with the same keys; without it, or with a bad
signature, nothing is installed and the exit code is non-zero.

Linux only: before the daemon binary is replaced, the K2 root helper
(/usr/local/libexec/k2-root-helper) is installed or updated to the
daemon's version with install-root-helper.sh. That needs root: run as
root, it runs directly; otherwise it asks sudo once. If it fails, the
daemon is not installed and the exit code is non-zero.
  --user NAME     the daemon user. As root: install the daemon for NAME
                  (files written as NAME) and the root helper for NAME.
                  Default: you. Env K2_DAEMON_USER
  --unit UNIT     the daemon's systemd unit, for the helper's caller check
                  (default: k2-daemon.service, or what the box's
                  /etc/k2-root-helper/daemon.toml names for this user)
  --scope S       system or user (default: user when this script writes the
                  user unit; with --no-service, what daemon.toml names for
                  this user, else system)
  --root-helper-from DIR   use the signed k2-root-helper-linux-<arch> and its
                  .sig from DIR, no network (default for a file:// manifest:
                  the manifest's folder)
  --root-helper-file PATH  use this local k2-root-helper build (it must be
                  the daemon's version; a .sig beside it is checked)
  --no-root-helper  leave the root helper alone, for a box where it is
                  installed another way (env K2_NO_ROOT_HELPER=1)
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)      K2_VERSION="${2:-}"; shift 2 ;;
        --bin-dir)      K2_BIN_DIR="${2:-}"; shift 2 ;;
        --manifest-url) K2_MANIFEST_URL="${2:-}"; shift 2 ;;
        --no-service)   K2_NO_SERVICE=1; shift ;;
        --dry-run)      K2_DRY_RUN=1; shift ;;
        --user)         K2_DAEMON_USER="${2:-}"; shift 2 ;;
        --unit)         K2_HELPER_UNIT="${2:-}"; shift 2 ;;
        --scope)        K2_HELPER_SCOPE="${2:-}"; shift 2 ;;
        --no-root-helper)   K2_NO_ROOT_HELPER=1; shift ;;
        --root-helper-from) K2_ROOT_HELPER_FROM="${2:-}"; shift 2 ;;
        --root-helper-file) K2_ROOT_HELPER_FILE="${2:-}"; shift 2 ;;
        -h|--help)      usage; exit 0 ;;
        *) echo "install-daemon.sh: unknown argument: $1" >&2; usage; exit 2 ;;
    esac
done

# A release is named with or without its tag's v: 0.46.7 and v0.46.7
# are the same release (the manifest URL below adds the v).
K2_VERSION="${K2_VERSION#v}"

die() { echo "$@" >&2; exit 1; }

# ── Daemon user ──────────────────────────────────────────────────────
# Root with --user NAME installs for NAME: every file under NAME's home is
# written as NAME (runuser), so root never writes through a path NAME
# controls. The root helper step then runs as root directly.
ME="$(id -un)"
AS_USER=""
DAEMON_USER="${K2_DAEMON_USER:-$ME}"
if [ "$DAEMON_USER" != "$ME" ]; then
    printf '%s' "$DAEMON_USER" | grep -Eq '^[a-z_][a-z0-9_-]{0,31}$' \
        || die "--user must be a plain user name (got '$DAEMON_USER')."
    [ "$(id -u)" = "0" ] || die "--user $DAEMON_USER installs for another user, which needs root: sudo sh install-daemon.sh --user $DAEMON_USER ... (or leave --user out to install for $ME)."
    [ "$(uname -s)" = "Linux" ] || die "--user is for Linux boxes."
    command -v runuser >/dev/null 2>&1 || die "runuser (util-linux) is required for --user."
    _home="$(getent passwd "$DAEMON_USER" 2>/dev/null | cut -d: -f6 || true)"
    { [ -n "$_home" ] && [ -d "$_home" ]; } || die "User '$DAEMON_USER' does not exist or has no home directory."
    HOME="$_home"
    AS_USER="$DAEMON_USER"
fi
[ -n "$K2_BIN_DIR" ] || K2_BIN_DIR="$HOME/.local/bin"

# as_user <cmd...>: run as the daemon user in root mode, else as is.
as_user() {
    if [ -n "$AS_USER" ]; then runuser -u "$AS_USER" -- "$@"; else "$@"; fi
}

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

# GitHub's own hosts: the only ones a fetch from GitHub follows a redirect
# to. After a repository move github.com answers the old path with a
# redirect to the new one, and its release downloads redirect to
# release-assets.githubusercontent.com (objects.githubusercontent.com for
# older links); api.github.com moves the same way. Curl can't limit -L to
# a list of hosts, so github_fetch never uses -L: it follows each hop
# itself, https only, after checking the host, and refuses a redirect
# anywhere else before that address is contacted. The same walk as the
# k2 CLI's _k2_github_curl (cli/src/update.sh); this script stands alone,
# so it carries its own copy.
K2_GITHUB_HOSTS="api.github.com github.com objects.githubusercontent.com release-assets.githubusercontent.com"
# The most redirects one fetch follows.
K2_GITHUB_MAX_REDIRS=5

# github_url_ok <url>: 0 when <url> is https on one of GitHub's hosts, with
# no name before an @ and no port.
github_url_ok() {
    case "$1" in https://*) ;; *) return 1 ;; esac
    _ghrest="${1#https://}"
    _ghhost="${_ghrest%%[/?#]*}"
    for _ghk in $K2_GITHUB_HOSTS; do
        [ "$_ghhost" = "$_ghk" ] && return 0
    done
    return 1
}

# github_where <url>: the scheme and host of <url>, printable characters
# only, for an error line (never the path or the query).
github_where() {
    case "$1" in
        *://*) _ghw="${1#*://}"; _ghw="${1%%://*}://${_ghw%%[/?#]*}" ;;
        *) _ghw="${1%%[/?#]*}" ;;
    esac
    printf '%s' "$_ghw" | LC_ALL=C tr -cd '[:graph:]'
}

# github_fetch <url> <file>: GET <url> into <file>, following redirects on
# GitHub's hosts one hop at a time (at most K2_GITHUB_MAX_REDIRS), https
# only, also when a ~/.curlrc says -L. Sends no token: no config, no
# Authorization header. Returns 1 when curl fails (curl's own line on
# stderr). A redirect off GitHub's hosts, to anything but https, or past
# the cap prints one line and returns 2; nothing is sent to that address.
github_fetch() {
    _ghu="$1"; _ghf="$2"; _ghn=0
    while :; do
        _ghnext=$(curl --no-location --proto =https -fsS -o "$_ghf" -w '%{redirect_url}' "$_ghu") || return 1
        [ -n "$_ghnext" ] || return 0
        _ghn=$((_ghn + 1))
        if [ "$_ghn" -gt "$K2_GITHUB_MAX_REDIRS" ]; then
            echo "Error: GitHub redirected more than $K2_GITHUB_MAX_REDIRS times (the last to $(github_where "$_ghnext")); stopped there" >&2
            return 2
        fi
        case "$_ghnext" in
            https://*) ;;
            *) echo "Error: GitHub redirected to $(github_where "$_ghnext"), which is not https; nothing was sent there" >&2
               return 2 ;;
        esac
        if ! github_url_ok "$_ghnext"; then
            echo "Error: GitHub redirected to $(github_where "$_ghnext"), which is not GitHub; nothing was sent there" >&2
            return 2
        fi
        _ghu="$_ghnext"
    done
}

fetch() {
    # fetch <url> <dest-file>  (dest "-" = stdout). A URL on GitHub's hosts
    # (the default manifest, the release's assets) goes through
    # github_fetch; any other is one the person or their manifest named (a
    # mirror, K2_MANIFEST_URL): fetched as before, with curl -L.
    _u="$1"; _d="$2"
    case "$_u" in
        file://*)
            _p="${_u#file://}"
            [ -f "$_p" ] || die "File not found: $_p"
            if [ "$_d" = "-" ]; then cat "$_p"; else cp "$_p" "$_d"; fi
            ;;
        *)
            command -v curl >/dev/null 2>&1 || die "curl is required to fetch from the network."
            if github_url_ok "$_u"; then
                if [ "$_d" = "-" ]; then
                    _ft=$(mktemp "${TMPDIR:-/tmp}/k2-install-fetch.XXXXXX") \
                        || die "Failed to make a temp file to fetch $_u"
                    github_fetch "$_u" "$_ft" || { rm -f "$_ft"; die "Failed to fetch: $_u"; }
                    cat "$_ft"
                    rm -f "$_ft"
                else
                    github_fetch "$_u" "$_d" || die "Failed to fetch: $_u"
                fi
            elif [ "$_d" = "-" ]; then
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
    # Prehashed (ED) only, like the daemon: `minisign -V` alone would also
    # take a legacy (Ed) signature. Base64 "RU[QRST]" is exactly "ED".
    case "$(sed -n 2p "$_vs")" in
        RU[QRST]*) ;;
        *) die "The signature is not a prehashed minisign signature. Refusing." ;;
    esac
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
# for installs older than $K2_FIRST_U2_VERSION.
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

# The k5 command's program (Linux): installed beside the daemon, where
# k2-client's cli_install looks for it. Off by default: only the owner's
# "New k5 command (Beta)" switch points the k2 and k5 links at it, and only
# when its version mark is this release's; otherwise they run the script.
# Signed like the daemon (sign-daemon.sh, U2): its .sig2 must verify with
# the K2 update keys for exactly this asset and version, or the install
# stops before anything is installed. Releases before K2_FIRST_K5_VERSION
# have no signed k5 program: skipped, the commands run the script.
K5_ON=0
K5_OLD=0
if [ "${PLATFORM%%-*}" = "linux" ] && [ -n "$MVER" ]; then
    if version_lt "$MVER" "$K2_FIRST_K5_VERSION"; then K5_OLD=1; else K5_ON=1; fi
fi
K5_ASSET="k5-cli-$PLATFORM"
# A file:// manifest (a mirror, an air-gapped box): from its folder, no
# network, as the root helper.
case "$MANIFEST_URL" in
    file://*) K5_URL="file://$(dirname "${MANIFEST_URL#file://}")/$K5_ASSET" ;;
    *) K5_URL="$RELEASE_BASE/v$MVER/$K5_ASSET" ;;
esac
K5_PATH="$K2_BIN_DIR/k5-cli"

if [ "$(uname -s)" = "Darwin" ]; then
    SVC_KIND="launchd"
    SVC_PATH="$HOME/Library/LaunchAgents/${K2_DAEMON_LABEL}.plist"
else
    SVC_KIND="systemd"
    SVC_PATH="$HOME/.config/systemd/user/k2-daemon.service"
fi

# ── Root helper plan (Linux) ─────────────────────────────────────────
# Who the helper serves (daemon.toml: user, unit, scope), where the signed
# binary comes from, and which install-root-helper.sh installs it. Nothing
# here needs root or the network.
RH_ON=0
if [ "${PLATFORM%%-*}" = "linux" ] && [ "$K2_NO_ROOT_HELPER" != "1" ]; then
    RH_ON=1
fi
RH_PATH="$RH_PREFIX$RH_CANON"
RH_TOML="$RH_PREFIX/etc/k2-root-helper/daemon.toml"
RH_SUDOERS="$RH_PREFIX/etc/sudoers.d/k2-root-helper"
RH_ASSET="k2-root-helper-linux-${PLATFORM#*-}"
# rh_toml <key>: one `key = "value"` from the box's daemon.toml (0644).
rh_toml() {
    if [ -f "$RH_TOML" ] && [ ! -L "$RH_TOML" ] && [ -r "$RH_TOML" ]; then
        sed -n "s/^$1 = \"\([^\"]*\)\"\$/\1/p" "$RH_TOML" | head -n1
    fi
}
RH_TOML_USER="$(rh_toml user)"
RH_TOML_UNIT="$(rh_toml unit)"
RH_TOML_SCOPE="$(rh_toml scope)"
RH_UNIT="$K2_HELPER_UNIT"
RH_SCOPE="$K2_HELPER_SCOPE"
if [ "$K2_NO_SERVICE" != "1" ]; then
    # This script writes the user unit below: that is the daemon.
    [ -n "$RH_UNIT" ] || RH_UNIT="k2-daemon.service"
    [ -n "$RH_SCOPE" ] || RH_SCOPE="user"
elif [ "$RH_TOML_USER" = "$DAEMON_USER" ]; then
    # The caller supervises the daemon: keep what the box already names.
    [ -n "$RH_UNIT" ] || RH_UNIT="$RH_TOML_UNIT"
    [ -n "$RH_SCOPE" ] || RH_SCOPE="$RH_TOML_SCOPE"
fi
[ -n "$RH_UNIT" ] || RH_UNIT="k2-daemon.service"
[ -n "$RH_SCOPE" ] || RH_SCOPE="system"
case "$RH_UNIT" in *.service) ;; *) RH_UNIT="$RH_UNIT.service" ;; esac
if [ "$RH_ON" = "1" ]; then
    printf '%s' "$RH_UNIT" | grep -Eq '^[A-Za-z0-9@._-]{1,64}\.service$' \
        || die "--unit must be a systemd unit name (got '$RH_UNIT')."
    case "$RH_SCOPE" in system|user) ;; *) die "--scope must be system or user (got '$RH_SCOPE')." ;; esac
    [ -z "$K2_ROOT_HELPER_FROM" ] || [ -z "$K2_ROOT_HELPER_FILE" ] \
        || die "Pass --root-helper-from or --root-helper-file, not both."
fi
RH_FROM="$K2_ROOT_HELPER_FROM"
if [ -z "$RH_FROM" ] && [ -z "$K2_ROOT_HELPER_FILE" ]; then
    # A local manifest means no network: the helper sits beside it.
    case "$MANIFEST_URL" in file://*) RH_FROM="$(dirname "${MANIFEST_URL#file://}")" ;; esac
fi
if [ -n "$K2_ROOT_HELPER_FILE" ]; then
    RH_SRC="$K2_ROOT_HELPER_FILE"
    RH_SRC_DESC="local build $RH_SRC"
elif [ -n "$RH_FROM" ]; then
    RH_SRC="$RH_FROM/$RH_ASSET"
    RH_SRC_DESC="$RH_SRC + .sig (no network)"
else
    RH_SRC=""
    RH_SRC_DESC="release v$MVER $RH_ASSET + .sig + .sha256"
fi
# install-root-helper.sh: beside this script, else in the --root-helper-from
# folder, else (network) the same release's copy.
RH_SELF_DIR=""
if [ -f "$0" ]; then RH_SELF_DIR="$(cd "$(dirname "$0")" && pwd)"; fi
if [ -n "$RH_SELF_DIR" ] && [ -f "$RH_SELF_DIR/install-root-helper.sh" ]; then
    RH_INSTALLER="$RH_SELF_DIR/install-root-helper.sh"
elif [ -n "$RH_FROM" ]; then
    RH_INSTALLER="$RH_FROM/install-root-helper.sh"
else
    RH_INSTALLER="${RELEASE_BASE%/}/v$MVER/install-root-helper.sh"
fi
if [ "$(id -u)" = "0" ]; then RH_HOW="as root"; else RH_HOW="through sudo (one prompt)"; fi

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
    if [ "$K5_ON" = "1" ]; then
        echo "  k5 program:     $K5_URL (+ .sig2: K2 update key, file:$K5_ASSET version:$MVER) -> $K5_PATH (off until the owner turns it on)"
    elif [ "$K5_OLD" = "1" ]; then
        echo "  k5 program:     (none signed before $K2_FIRST_K5_VERSION; the k2 command runs the script)"
    fi
    if [ "$RH_ON" = "1" ]; then
        echo "  root helper:    $RH_CANON v$MVER for $DAEMON_USER ($RH_UNIT, $RH_SCOPE scope), before the daemon"
        echo "  helper source:  $RH_SRC_DESC"
        echo "  helper install: $RH_INSTALLER, $RH_HOW"
    elif [ "${PLATFORM%%-*}" = "linux" ]; then
        echo "  root helper:    (skipped — --no-root-helper)"
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
# Root mode: the daemon user copies the verified files out of this
# root-owned folder (readable, never writable by them).
[ -z "$AS_USER" ] || chmod 0755 "$TMP"

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

# ── The k5 program (Linux): verified like the daemon, before any install ──
# k5_refused <reason>: the one way out when the k5 program doesn't verify.
k5_refused() {
    echo "K5 PROGRAM NOT VERIFIED: $1" >&2
    die "Refusing to install. Nothing was installed or changed (the daemon, the root helper and $K5_ASSET are as they were)."
}
DL_K5="$TMP/$K5_ASSET"
if [ "$K5_ON" = "1" ]; then
    echo "Downloading the k5 program ($K5_ASSET) and its signature..."
    ( fetch "$K5_URL" "$DL_K5" ) || k5_refused "$K5_ASSET could not be fetched from $K5_URL."
    ( fetch "$K5_URL.sig2" "$DL_K5.sig2" ) || k5_refused "$K5_ASSET.sig2 could not be fetched from $K5_URL.sig2 (this release's k5 program must carry its signature)."
    echo "Verifying the k5 program's minisign signature (K2 update keys)..."
    ( verify_update_sig "$DL_K5" "$DL_K5.sig2" "$K5_ASSET" "$MVER" ) \
        || k5_refused "MINISIGN VERIFICATION FAILED for $K5_ASSET ($K5_URL): not signed by a K2 update key for $K5_ASSET v$MVER."
elif [ "$K5_OLD" = "1" ]; then
    echo "The k5 program: v$MVER is older than $K2_FIRST_K5_VERSION, the first release that signs it; not installed (the k2 command runs the script)."
fi

# ── Root helper (Linux): installed or updated BEFORE the daemon swap ──
# rh_marker_version <file>: the version in a root helper's build marker
# (read, never executed).
rh_marker_version() {
    [ -f "$1" ] || return 0
    LC_ALL=C grep -a -o 'K2_ROOT_HELPER_BUILD v=[0-9.]* proto=[0-9]* arch=[a-z0-9_]*;' "$1" 2>/dev/null \
        | head -n1 | sed 's/^K2_ROOT_HELPER_BUILD v=\([0-9.]*\) .*/\1/' || true
}
# rh_marker_protocol <file>: the protocol in the build marker (0 if none).
rh_marker_protocol() {
    _p=""
    [ -f "$1" ] && _p="$(LC_ALL=C grep -a -o 'K2_ROOT_HELPER_BUILD v=[0-9.]* proto=[0-9]*' "$1" 2>/dev/null \
        | head -n1 | sed 's/.* proto=//' || true)"
    echo "${_p:-0}"
}
# The first protocol that can update itself (k2_root_proto
# SELF_UPDATE_MIN_PROTOCOL); 1 and 2 are the legacy helpers.
RH_SELF_UPDATE_MIN_PROTOCOL=3
# A package-owned helper: the canonical path is a link into the package's
# own folder (the helper then reports packageOwned and the daemon never
# updates it), or dpkg owns the path.
rh_package_owned() {
    [ -L "$RH_PATH" ] && return 0
    command -v dpkg-query >/dev/null 2>&1 && dpkg-query -S "$RH_CANON" >/dev/null 2>&1 && return 0
    return 1
}
# rh_door_ok: the daemon user's one sudoers line is in place (root reads
# it; anyone else asks sudo without a prompt).
rh_door_ok() {
    if [ "$(id -u)" = "0" ]; then
        [ -f "$RH_SUDOERS" ] && [ ! -L "$RH_SUDOERS" ] \
            && [ "$(cat "$RH_SUDOERS")" = "$(printf '%s ALL=(root) NOPASSWD: %s\nDefaults:%s !requiretty' "$DAEMON_USER" "$RH_CANON" "$DAEMON_USER")" ]
    else
        command -v sudo >/dev/null 2>&1 && sudo -n -l "$RH_CANON" >/dev/null 2>&1
    fi
}
rh_failed() {
    echo "ROOT HELPER NOT INSTALLED: $1" >&2
    echo "The daemon was not installed or changed. Fix that and re-run. As root one run installs both:" >&2
    echo "  sudo sh install-daemon.sh --user $DAEMON_USER --version $MVER [--no-service]" >&2
    echo "If a package or your own tooling manages the root helper, --no-root-helper skips this step." >&2
    exit 1
}
root_helper_step() {
    if [ "$RH_ON" != "1" ]; then
        [ "${PLATFORM%%-*}" != "linux" ] || echo "Root helper: not touched (--no-root-helper)."
        return 0
    fi
    if rh_package_owned; then
        if [ "$(id -u)" != "0" ] && [ -L "$RH_PATH" ] && [ ! -e "$RH_PATH" ]; then
            rh_failed "$RH_CANON is a link to a file that is gone (a package helper, broken); reinstall that package."
        fi
        echo "Root helper: $RH_CANON is managed by your package manager; left alone."
        return 0
    fi
    _have=""
    [ -L "$RH_PATH" ] || _have="$(rh_marker_version "$RH_PATH")"
    if [ -n "$_have" ] && ! version_lt "$_have" "$MVER" \
        && [ "$RH_TOML_USER" = "$DAEMON_USER" ] && [ "$RH_TOML_UNIT" = "$RH_UNIT" ] \
        && [ "$RH_TOML_SCOPE" = "$RH_SCOPE" ] && rh_door_ok; then
        echo "Root helper: v$_have is installed for $DAEMON_USER ($RH_UNIT, $RH_SCOPE scope); nothing to change."
        return 0
    fi
    # Not root (the fleet's `sudo -u <daemon user>` form): an older helper
    # that is root-owned, can update itself (protocol 3+), names this user
    # in daemon.toml and lets this user in is the daemon's auto-heal's job
    # after it starts. Anything else (missing, legacy, not allowed) still
    # needs the root step below, which fails loudly without sudo.
    if [ "$(id -u)" != "0" ] && [ -n "$_have" ] \
        && [ "$(rh_marker_protocol "$RH_PATH")" -ge "$RH_SELF_UPDATE_MIN_PROTOCOL" ] 2>/dev/null \
        && case "$(stat -c '%U:%G %a' "$RH_PATH" 2>/dev/null)" in root:*) true ;; *) false ;; esac \
        && [ "$RH_TOML_USER" = "$DAEMON_USER" ] && rh_door_ok; then
        echo "Root helper: v$_have is older; the daemon will update it after it starts (auto-heal). To do it now: sudo sh install-daemon.sh --user $DAEMON_USER --version $MVER"
        return 0
    fi
    if [ -n "$RH_SRC" ]; then
        [ -f "$RH_SRC" ] || rh_failed "$RH_SRC was not found."
        if [ -z "$K2_ROOT_HELPER_FILE" ] && [ ! -f "$RH_SRC.sig" ]; then
            rh_failed "$RH_SRC.sig was not found; a root helper from $RH_FROM must carry its signature."
        fi
        _v="$(rh_marker_version "$RH_SRC")"
        [ "$_v" = "$MVER" ] || rh_failed "$RH_SRC is v${_v:-unknown}, but the daemon is v$MVER; they must match."
        set -- --file "$RH_SRC"
    else
        set -- --version "$MVER"
    fi
    set -- "$@" --user "$DAEMON_USER" --unit "$RH_UNIT" --scope "$RH_SCOPE"
    case "$RH_INSTALLER" in
        http://*|https://*|file://*)
            ( fetch "$RH_INSTALLER" "$TMP/install-root-helper.sh" ) \
                || rh_failed "could not download $RH_INSTALLER."
            _inst="$TMP/install-root-helper.sh" ;;
        *)
            [ -f "$RH_INSTALLER" ] || rh_failed "no install-root-helper.sh beside this script or in $RH_FROM (every release ships one)."
            _inst="$RH_INSTALLER" ;;
    esac
    command -v bash >/dev/null 2>&1 || rh_failed "bash is required to run install-root-helper.sh."
    echo "Installing the K2 root helper v$MVER for $DAEMON_USER ($RH_UNIT, $RH_SCOPE scope; from $RH_SRC_DESC)..."
    if [ "$(id -u)" = "0" ]; then
        bash "$_inst" "$@" || rh_failed "install-root-helper.sh failed (its reason is above)."
    else
        command -v sudo >/dev/null 2>&1 || rh_failed "installing it needs root, and sudo is not installed."
        echo "sudo asks for your password once, to install the root helper."
        if [ -n "${K2_RELEASE_BASE:-}" ]; then
            sudo env "K2_RELEASE_BASE=$K2_RELEASE_BASE" bash "$_inst" "$@"
        else
            sudo bash "$_inst" "$@"
        fi || rh_failed "install-root-helper.sh through sudo failed (the reason is above). It needs an admin who may use sudo."
    fi
    _now="$(rh_marker_version "$RH_PATH")"
    [ "$_now" = "$MVER" ] || rh_failed "after the install, $RH_CANON reports v${_now:-unknown}, not v$MVER."
    echo "Root helper: v$MVER installed for $DAEMON_USER."
}
root_helper_step

echo "Verified. Installing to $BIN_PATH ..."
[ -z "$AS_USER" ] || chmod 0644 "$DL_BIN"
as_user mkdir -p "$K2_BIN_DIR"
# Atomic rename — plain cp over a RUNNING daemon binary fails with
# "Text file busy"; rename swaps the inode and re-runs converge.
as_user cp "$DL_BIN" "$BIN_PATH.new"
as_user chmod +x "$BIN_PATH.new"
as_user mv -f "$BIN_PATH.new" "$BIN_PATH"

# The k5 command's program beside the daemon (verified above).
if [ "$K5_ON" = "1" ]; then
    echo "Installing the k5 program for v$MVER..."
    [ -z "$AS_USER" ] || chmod 0644 "$DL_K5"
    as_user cp "$DL_K5" "$K5_PATH.new"
    as_user chmod 0755 "$K5_PATH.new"
    as_user mv -f "$K5_PATH.new" "$K5_PATH"
    echo "Installed $K5_PATH, minisign-verified (the k2 command runs it only when the owner turns on the new k5 command)."
fi

# Stage the matching App engine (R3): sha256-checked; never fatal (the
# daemon's boot heal downloads it when this didn't work).
if [ -n "$ENGINE_URL" ] && [ -n "$ENGINE_SHA" ]; then
    echo "Staging the App engine for v${MVER:-?}..."
    DL_ENG="$TMP/$ENGINE_KEY"
    if ( fetch "$ENGINE_URL" "$DL_ENG" ) 2>/dev/null && [ "$(sha256_of "$DL_ENG")" = "$ENGINE_SHA" ]; then
        [ -z "$AS_USER" ] || chmod 0644 "$DL_ENG"
        as_user mkdir -p "$ENGINE_STAGE_DIR"
        as_user rm -f "$ENGINE_STAGE_DIR/$ENGINE_KEY.sig"
        as_user cp "$DL_ENG" "$ENGINE_STAGE_DIR/$ENGINE_KEY.part" && as_user mv -f "$ENGINE_STAGE_DIR/$ENGINE_KEY.part" "$ENGINE_STAGE_DIR/$ENGINE_KEY"
        if [ -n "$ENGINE_SIG" ] && ( fetch "$ENGINE_SIG" "$TMP/$ENGINE_KEY.sig" ) 2>/dev/null; then
            [ -z "$AS_USER" ] || chmod 0644 "$TMP/$ENGINE_KEY.sig"
            as_user cp "$TMP/$ENGINE_KEY.sig" "$ENGINE_STAGE_DIR/$ENGINE_KEY.sig"
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
        as_user mkdir -p "$(dirname "$SVC_PATH")"
        as_user sh -c 'cat > "$1"' sh "$SVC_PATH" <<EOF
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
# k2-deskd (remote desktop, off until an Owner turns it on) is the daemon's
# child too: it reads the console session's DISPLAY / WAYLAND_DISPLAY from
# `systemctl --user show-environment` (or loginctl), so this unit needs no
# Environment= for it, and it exits by itself (releasing any held key) when
# the daemon's pipe closes, so KillMode=process leaves no stray helper.
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
