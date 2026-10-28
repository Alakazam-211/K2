#!/usr/bin/env bash
# sign-approve.sh — open the release's signing session: one approval (Touch ID
# or the Mac login password, in the system sheet) per
# release (prd-signing-user-v1 rev 2 §4.4, S2). Sourced by release.sh
# (Step 0.46, before any build) after scripts/lib/sign-updater.sh. Sets no
# shell options.
#
#   k2sa_open_session <version> <base-commit> [--windows] <asset>...
#
# 1. `k2sign status <version>`. Policy approval off → nothing to do. A valid
#    session with the same base commit, assets and Windows flag → reuse it
#    (K2_RELEASE_RESUME runs, a by-hand re-run within 12 h). A session with
#    other parameters → stop loudly: someone else opened one; investigate.
#    An expired session with the same base → re-approve (the prompt says
#    RE-APPROVE).
# 2. `k2sign challenge …` → the challenge JSON the signer made and keeps.
# 3. K2 Release Approve (a small Swift app in ~/Applications, run as this
#    user with `open -W`): Touch ID or the login password, then a Secure Enclave P-256 signature
#    over the challenge's exact bytes. The app is untrusted: without the
#    finger on this Mac's sensor it can't make that signature, and the signer
#    checks it against the enrolled public key.
# 4. A watchdog kills the app after K2_SIGN_APPROVE_TIMEOUT_MIN (default 30)
#    and fails with the re-run line. Nothing has been built yet.
# 5. `k2sign approve <nonce> < signature` → the session.
#
# Settings (env):
#   K2_SIGN_APPROVE_TIMEOUT_MIN   default 30
#   K2_SIGN_APPROVE_APP           default "$HOME/Applications/K2 Release Approve.app"
#   K2_SIGN_APPROVE_CMD           tests only (release.sh refuses it): an
#                                 executable run with --challenge F --out F
#                                 instead of the app

k2sa_log() { printf '[sign-approve] %s\n' "$*" >&2; }
k2sa_fail() { printf '\033[1;31m[sign-approve] ERROR: %s\033[0m\n' "$*" >&2; return 1; }

# JSON field from a k2sign status document on stdin.
k2sa_status_field() {
    python3 -I -c '
import json, sys
d = json.load(sys.stdin)
path = sys.argv[1].split(".")
for p in path:
    d = d.get(p) if isinstance(d, dict) else None
if isinstance(d, bool):
    print("true" if d else "false")
elif isinstance(d, list):
    print(" ".join(sorted(d)))
elif d is None:
    print("")
else:
    print(d)' "$1"
}

# Run the approval helper with a watchdog. Returns 0 when <out> holds a signature.
k2sa_run_helper() {
    local cf="$1" of="$2" version="$3" timeout_min pid waited=0 limit
    timeout_min="${K2_SIGN_APPROVE_TIMEOUT_MIN:-30}"
    [[ "$timeout_min" =~ ^[1-9][0-9]{0,3}$ ]] || { k2sa_fail "K2_SIGN_APPROVE_TIMEOUT_MIN must be a whole number of minutes"; return 1; }
    limit=$((timeout_min * 60))
    if [ -n "${K2_SIGN_APPROVE_CMD:-}" ]; then
        "$K2_SIGN_APPROVE_CMD" --challenge "$cf" --out "$of" &
    else
        local app="${K2_SIGN_APPROVE_APP:-$HOME/Applications/K2 Release Approve.app}"
        [ -d "$app" ] || { k2sa_fail "no approval app at $app (scripts/k2sign/approve/build.sh, then Enrol)"; return 1; }
        /usr/bin/open -W -n -a "$app" --args --challenge "$cf" --out "$of" &
    fi
    pid=$!
    while kill -0 "$pid" 2>/dev/null; do
        if [ "$waited" -ge "$limit" ]; then
            kill "$pid" 2>/dev/null
            # `open -W` is gone, but the app may still be up: it is the one
            # process with this challenge file on its command line.
            /usr/bin/pkill -f -- "--challenge $cf" 2>/dev/null
            wait "$pid" 2>/dev/null
            k2sa_fail "No signing approval (timed out after ${timeout_min} min) — nothing was built. Re-run: ./scripts/release.sh $version"
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done
    wait "$pid" 2>/dev/null
    if [ -s "$of" ]; then
        return 0
    fi
    if [ -s "$of.err" ]; then
        k2sa_fail "No signing approval ($(head -c 300 "$of.err")) — nothing was built. Re-run: ./scripts/release.sh $version"
    else
        k2sa_fail "No signing approval (the app closed without one) — nothing was built. Re-run: ./scripts/release.sh $version"
    fi
    return 1
}

k2sa_open_session() {
    local version="$1" base="$2" windows=0 st approval valid sbase sassets swin assets_sorted
    shift 2
    local assets=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --windows) windows=1 ;;
            *) assets+=("$1") ;;
        esac
        shift
    done
    [ "${#assets[@]}" -gt 0 ] || { k2sa_fail "k2sa_open_session: no assets"; return 1; }
    assets_sorted="$(printf '%s\n' "${assets[@]}" | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//')"

    st="$(k2u_k2sign status "$version")" || { k2sa_fail "k2sign status failed (above)"; return 1; }
    approval="$(printf '%s' "$st" | k2sa_status_field approval)"
    if [ "$approval" = off ]; then
        k2sa_log "the signer's policy has approval off (soft rollback): no approval session; its other checks still run"
        return 0
    fi
    local reapprove=""
    if [ -n "$(printf '%s' "$st" | k2sa_status_field session.base_commit)" ]; then
        sbase="$(printf '%s' "$st" | k2sa_status_field session.base_commit)"
        sassets="$(printf '%s' "$st" | k2sa_status_field session.assets)"
        swin="$(printf '%s' "$st" | k2sa_status_field session.windows)"
        valid="$(printf '%s' "$st" | k2sa_status_field session.valid)"
        if [ "$sbase" != "$base" ] || [ "$sassets" != "$assets_sorted" ] || { [ "$swin" = true ] && [ "$windows" = 0 ]; } || { [ "$swin" = false ] && [ "$windows" = 1 ]; }; then
            k2sa_fail "$version already has an approved session with different parameters (approved $(printf '%s' "$st" | k2sa_status_field session.approved), base ${sbase:0:12}, assets: $sassets). This run wants base ${base:0:12}, assets: $assets_sorted. Investigate before re-approving."
            return 1
        fi
        if [ "$valid" = true ]; then
            k2sa_log "using $version's approved signing session (until $(printf '%s' "$st" | k2sa_status_field session.expires))"
            return 0
        fi
        reapprove=1
        k2sa_log "$version's signing session expired; asking to RE-APPROVE (same base commit)"
    fi

    local work cf of nonce sid
    work="$(mktemp -d "${TMPDIR:-/tmp}/k2-approve.XXXXXX")" || return 1
    cf="$work/challenge.json"; of="$work/approval.der"
    local flags=()
    [ "$windows" = 1 ] && flags+=(--windows)
    [ -n "$reapprove" ] && flags+=(--reapprove)
    if ! k2u_k2sign challenge "$version" "$base" "${assets[@]}" ${flags[@]+"${flags[@]}"} > "$cf"; then
        rm -rf "$work"; k2sa_fail "the signer refused to issue a challenge (above)"; return 1
    fi
    nonce="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["nonce"])' "$cf")" \
        || { rm -rf "$work"; k2sa_fail "the challenge is not JSON"; return 1; }

    printf '\n\033[1;33m══ Approval needed on %s (Touch ID or your login password): %sapprove K2 %s (base %s), %s update files%s ══\033[0m\n' \
        "$(scutil --get LocalHostName 2>/dev/null || hostname -s)" "${reapprove:+RE-}" "$version" "${base:0:7}" "${#assets[@]}" "$([ "$windows" = 1 ] && echo ', INCLUDING the Windows setup (no independent check)')" >&2
    /usr/bin/osascript -e "display notification \"Touch ID or password: ${reapprove:+RE-}approve K2 $version signing\" with title \"K2 release\"" >/dev/null 2>&1 || true

    if ! k2sa_run_helper "$cf" "$of" "$version"; then
        rm -rf "$work"; return 1
    fi
    if ! sid="$(k2u_k2sign approve "$nonce" < "$of")"; then
        rm -rf "$work"; k2sa_fail "the signer did not accept the approval (above) — nothing was built. Re-run: ./scripts/release.sh $version"; return 1
    fi
    rm -rf "$work"
    k2sa_log "signing session $sid open for $version"
}
