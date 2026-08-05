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

# Fail if a built k2-daemon did NOT bake the real Gmail client (option_env!).
#
# IMPORTANT: rustc may still embed the `None => "REPLACE_ME…"` string literal
# in the binary even when option_env! took the Some branch (dead arm not
# always stripped). So "REPLACE_ME present" is NOT a reliable failure signal.
# Require the real env values appear as contiguous bytes instead.
assert_daemon_oauth_not_placeholder() {
    local bin="${1:-}"
    if [ -z "$bin" ] || [ ! -f "$bin" ]; then
        echo "FATAL: assert_daemon_oauth_not_placeholder: binary missing: $bin" >&2
        exit 1
    fi
    _k2_bin_has() {
        local needle="$1"
        [ -n "$needle" ] && grep -aF -q "$needle" "$bin" 2>/dev/null
    }
    # Env must already be non-placeholder (require_mail_oauth_build_env).
    if _k2_oauth_is_placeholder "${K2_GMAIL_CLIENT_ID:-}"; then
        echo "FATAL: assert_daemon_oauth_not_placeholder: K2_GMAIL_CLIENT_ID unset/placeholder" >&2
        exit 1
    fi
    if _k2_oauth_is_placeholder "${K2_GMAIL_CLIENT_SECRET:-}"; then
        echo "FATAL: assert_daemon_oauth_not_placeholder: K2_GMAIL_CLIENT_SECRET unset/placeholder" >&2
        exit 1
    fi
    if ! _k2_bin_has "$K2_GMAIL_CLIENT_ID"; then
        echo "FATAL: $bin does not contain the real K2_GMAIL_CLIENT_ID" >&2
        echo "  option_env! did not bake the Gmail client — email-link would send REPLACE_ME to Google." >&2
        exit 1
    fi
    if ! _k2_bin_has "$K2_GMAIL_CLIENT_SECRET"; then
        echo "FATAL: $bin does not contain the real K2_GMAIL_CLIENT_SECRET" >&2
        echo "  option_env! did not bake the Gmail secret — token exchange would fail." >&2
        exit 1
    fi
    if [ "${K2_REQUIRE_MICROSOFT_OAUTH:-0}" = "1" ]; then
        if _k2_oauth_is_placeholder "${K2_MICROSOFT_CLIENT_ID:-}"; then
            echo "FATAL: K2_MICROSOFT_CLIENT_ID required but unset/placeholder" >&2
            exit 1
        fi
        if ! _k2_bin_has "$K2_MICROSOFT_CLIENT_ID"; then
            echo "FATAL: $bin does not contain the real K2_MICROSOFT_CLIENT_ID" >&2
            exit 1
        fi
    fi
    # Optional note: REPLACE_ME may still appear as a dead match-arm string.
    if _k2_bin_has 'REPLACE_ME.apps.googleusercontent.com'; then
        echo "  note: binary still contains REPLACE_ME string literal (rustc dead arm); real Gmail client id is present — OK."
    fi
    echo "  OAuth bake check: $bin contains real Gmail client id + secret (len ${#K2_GMAIL_CLIENT_ID}/${#K2_GMAIL_CLIENT_SECRET})."
}
