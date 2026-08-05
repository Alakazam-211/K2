#!/usr/bin/env bash
# require-mail-oauth-build-env.sh — gate k2-daemon *release* builds so the
# pre-packaged Gmail (and Microsoft) OAuth client is baked in via
# option_env!("K2_GMAIL_CLIENT_*" / "K2_MICROSOFT_CLIENT_ID").
#
# Root cause of the Linux fleet "invalid_client / OAuth client was not found"
# bug: crates/k2-daemon/src/mail/oauth/mod.rs bakes defaults at COMPILE time.
# Without these env vars the binary ships the literal REPLACE_ME placeholder
# and Google rejects every email-link. macOS release builds happened to work
# because scripts/release.sh sources .env; daemon-binaries.yml (Linux) did not.
#
# Usage (from any build script, after sourcing .env if present):
#   source scripts/require-mail-oauth-build-env.sh
#   require_mail_oauth_build_env   # exits 1 if Gmail id/secret missing
#   cargo build --release -p k2-daemon
#   assert_daemon_oauth_not_placeholder path/to/k2-daemon
#
# Never prints secret values.
set -euo pipefail

# True if the value is empty or still the public-source placeholder.
_k2_oauth_is_placeholder() {
    local v="${1:-}"
    case "$v" in
        ""|REPLACE_ME*) return 0 ;;
        *) return 1 ;;
    esac
}

# Require Gmail client id + secret for any shipping daemon build.
# This is the customer-facing path (Google invalid_client when REPLACE_ME ships).
# Microsoft is optional unless K2_REQUIRE_MICROSOFT_OAUTH=1 (separate product track).
require_mail_oauth_build_env() {
    local missing=0
    if _k2_oauth_is_placeholder "${K2_GMAIL_CLIENT_ID:-}"; then
        echo "FATAL: K2_GMAIL_CLIENT_ID unset or REPLACE_ME — daemon will ship unusable Gmail OAuth." >&2
        echo "  Auth URL would use client_id=REPLACE_ME.apps.googleusercontent.com → Google invalid_client." >&2
        echo "  Set it in .env (macOS release) or as GitHub Actions secret K2_GMAIL_CLIENT_ID (daemon-binaries.yml)." >&2
        missing=1
    fi
    if _k2_oauth_is_placeholder "${K2_GMAIL_CLIENT_SECRET:-}"; then
        echo "FATAL: K2_GMAIL_CLIENT_SECRET unset or REPLACE_ME — Gmail token exchange will fail." >&2
        missing=1
    fi
    if _k2_oauth_is_placeholder "${K2_MICROSOFT_CLIENT_ID:-}"; then
        if [ "${K2_REQUIRE_MICROSOFT_OAUTH:-0}" = "1" ]; then
            echo "FATAL: K2_MICROSOFT_CLIENT_ID unset or REPLACE_ME (K2_REQUIRE_MICROSOFT_OAUTH=1)." >&2
            missing=1
        else
            echo "WARN: K2_MICROSOFT_CLIENT_ID unset or REPLACE_ME — Microsoft email-link will need BYO (Gmail is the required OOTB path)." >&2
        fi
    fi
    if [ "$missing" -ne 0 ]; then
        exit 1
    fi
    # Export so cargo/rustc child processes see option_env! inputs.
    export K2_GMAIL_CLIENT_ID K2_GMAIL_CLIENT_SECRET
    if ! _k2_oauth_is_placeholder "${K2_MICROSOFT_CLIENT_ID:-}"; then
        export K2_MICROSOFT_CLIENT_ID
    fi
    echo "  Mail OAuth build env: Gmail client id set (len=${#K2_GMAIL_CLIENT_ID}), secret set (len=${#K2_GMAIL_CLIENT_SECRET})${K2_MICROSOFT_CLIENT_ID:+, Microsoft client id set}."
}

# Fail if a built k2-daemon binary still contains the public placeholder
# client id string (catches CI secret misconfig + env not reaching rustc).
assert_daemon_oauth_not_placeholder() {
    local bin="${1:-}"
    if [ -z "$bin" ] || [ ! -f "$bin" ]; then
        echo "FATAL: assert_daemon_oauth_not_placeholder: binary missing: $bin" >&2
        exit 1
    fi
    # Prefer grep -aF on the binary (reliable). Avoid `strings | grep -q`:
    # under `set -o pipefail`, SIGPIPE from early-close grep yields 141 and
    # can false-negative a real hit (missed REPLACE_ME on Linux fleet bins).
    _k2_bin_has() {
        local needle="$1"
        grep -aF -q "$needle" "$bin" 2>/dev/null
    }
    # Priority: Gmail — this string is what Google sees as client_id= in the auth URL.
    if _k2_bin_has 'REPLACE_ME.apps.googleusercontent.com'; then
        echo "FATAL: $bin still contains REPLACE_ME.apps.googleusercontent.com" >&2
        echo "  K2_GMAIL_CLIENT_ID was not baked at compile time (email-link → Google invalid_client)." >&2
        exit 1
    fi
    if _k2_bin_has 'REPLACE_ME-google-client-secret'; then
        echo "FATAL: $bin still contains REPLACE_ME-google-client-secret" >&2
        echo "  K2_GMAIL_CLIENT_SECRET was not baked at compile time." >&2
        exit 1
    fi
    # Microsoft is secondary; only fail-closed when explicitly required.
    if [ "${K2_REQUIRE_MICROSOFT_OAUTH:-0}" = "1" ] && _k2_bin_has 'REPLACE_ME-microsoft-client-id'; then
        echo "FATAL: $bin still contains REPLACE_ME-microsoft-client-id" >&2
        echo "  K2_MICROSOFT_CLIENT_ID was not baked at compile time." >&2
        exit 1
    fi
    echo "  OAuth placeholder check: $bin is clean (no REPLACE_ME Gmail defaults)."
}
