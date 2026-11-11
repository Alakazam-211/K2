#!/usr/bin/env bash
# sign-approve.sh — open the release's signing session: one approval (Touch ID
# or the Mac login password, in the system sheet) per
# release (prd-signing-user-v1 rev 2 §4.4, S2). Sourced by release.sh after
# scripts/lib/sign-updater.sh; called at Step 1.55, after the pre-flight
# gates (a failed gate costs no approval) and before anything is built.
# Sets no shell options.
#
#   k2sa_open_session <version> <base-commit> [--windows] <asset>...
#
# 1. `k2sign status <version>`. Policy approval off → nothing to do. A valid
#    session with the same base commit, assets and Windows flag → reuse it
#    (K2_RELEASE_RESUME runs, a by-hand re-run within 12 h). A session with
#    other parameters: when status says it is unused (nothing signed under
#    the version, no compute evidence: an earlier run stopped at a gate, then
#    a fix commit) → ask to REPLACE it, one Touch ID like any approval (the
#    sheet says "RE-approve K2 <v> for new code <base7> (the earlier approval
#    for <oldbase7> was never used)"); the signer re-checks, keeps the old
#    session aside and logs SESSION-REPLACED. Used → stop loudly; investigate.
#    No session file but last_version is this version and nothing was signed
#    (a session moved aside by hand) → the same replacement. An expired
#    session with the same base → re-approve (the prompt says RE-APPROVE).
#    Break-glass only (admin password): k2sign-admin cancel-session.
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
# Tests only (release.sh and windows-sign-after-release.sh refuse them; no
# test may ever show UI, a notification, or ask for Touch ID or a password):
#   K2SA_HELPER     the ONLY approval helper run (wins over K2_SIGN_APPROVE_CMD);
#                   the app is never opened
#   K2SA_OPEN       stands in for /usr/bin/open on the app path (tests of the
#                   app's capability checks; the stub must never be reached)
#   K2SA_NOTIFY=0   no macOS notification (osascript is not run)
# In test mode (K2SIGN_TEST_ROOT set) with no helper, the app path is refused
# unless K2SA_OPEN and K2_SIGN_APPROVE_APP are both set: a test never reaches
# /usr/bin/open or the real app.

k2sa_log() { printf '[sign-approve] %s\n' "$*" >&2; }
# A macOS notification for the person at the Mac; K2SA_NOTIFY=0 (tests) skips it.
k2sa_notify() {
    [ "${K2SA_NOTIFY:-1}" = 0 ] && return 0
    /usr/bin/osascript -e "display notification \"$1\" with title \"K2 release\"" >/dev/null 2>&1 || true
}
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

# The approval app must be a build that closes its own sheet (--timeout):
# an older one would leave a sheet up after the watchdog gave up. A
# replacement also needs a build that says so in the sheet (replace-session).
#   k2sa_app_current <app> [<capability>]
k2sa_app_current() {
    local app="$1" need="${2:-}" caps
    caps="$("$app/Contents/MacOS/K2 Release Approve" --capabilities 2>/dev/null)" || caps=""
    case " $caps " in
        *" timeout "*) ;;
        *) k2sa_fail "K2 Release Approve at $app is too old (no --timeout). Rebuild it (your approver key is kept): bash scripts/k2sign/approve/build.sh — nothing was built. Re-run: ./scripts/release.sh ${VERSION:-<version>}"
           return 1 ;;
    esac
    [ -z "$need" ] && return 0
    case " $caps " in
        *" $need "*) return 0 ;;
    esac
    k2sa_fail "K2 Release Approve at $app is too old (no $need: its sheet wouldn't say this approval replaces an unused one). Rebuild it (your approver key is kept): bash scripts/k2sign/approve/build.sh — nothing was built. Re-run: ./scripts/release.sh ${VERSION:-<version>}"
    return 1
}

# Run the approval helper with a watchdog. Returns 0 when <out> holds a signature.
k2sa_run_helper() {
    local cf="$1" of="$2" version="$3" need="${4:-}" timeout_min pid waited=0 limit
    timeout_min="${K2_SIGN_APPROVE_TIMEOUT_MIN:-30}"
    [[ "$timeout_min" =~ ^[1-9][0-9]{0,3}$ ]] || { k2sa_fail "K2_SIGN_APPROVE_TIMEOUT_MIN must be a whole number of minutes"; return 1; }
    limit=$((timeout_min * 60))
    local helper="${K2SA_HELPER:-${K2_SIGN_APPROVE_CMD:-}}"
    if [ -n "$helper" ]; then
        "$helper" --challenge "$cf" --out "$of" --timeout "$limit" &
    else
        if [ -n "${K2SIGN_TEST_ROOT:-}" ] && { [ -z "${K2SA_OPEN:-}" ] || [ -z "${K2_SIGN_APPROVE_APP:-}" ]; }; then
            k2sa_fail "test mode (K2SIGN_TEST_ROOT) with no K2SA_HELPER or K2_SIGN_APPROVE_CMD: refusing to open the approval app (a test sets K2SA_HELPER, or K2SA_OPEN and K2_SIGN_APPROVE_APP to stubs)"
            return 1
        fi
        local app="${K2_SIGN_APPROVE_APP:-$HOME/Applications/K2 Release Approve.app}" open_cmd="${K2SA_OPEN:-/usr/bin/open}"
        [ -d "$app" ] || { k2sa_fail "no approval app at $app (scripts/k2sign/approve/build.sh, then Enrol)"; return 1; }
        k2sa_app_current "$app" "$need" || return 1
        # --timeout: the app cancels its own sheet when the watchdog would.
        "$open_cmd" -W -n -a "$app" --args --challenge "$cf" --out "$of" --timeout "$limit" &
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
    local reapprove="" replace="" oldbase="" unused
    unused="$(printf '%s' "$st" | k2sa_status_field unused)"
    if [ -n "$(printf '%s' "$st" | k2sa_status_field session.base_commit)" ]; then
        sbase="$(printf '%s' "$st" | k2sa_status_field session.base_commit)"
        sassets="$(printf '%s' "$st" | k2sa_status_field session.assets)"
        swin="$(printf '%s' "$st" | k2sa_status_field session.windows)"
        valid="$(printf '%s' "$st" | k2sa_status_field session.valid)"
        if [ "$sbase" != "$base" ] || [ "$sassets" != "$assets_sorted" ] || { [ "$swin" = true ] && [ "$windows" = 0 ]; } || { [ "$swin" = false ] && [ "$windows" = 1 ]; }; then
            local mismatch scount ssigned
            mismatch="$version already has an approved session with different parameters (approved $(printf '%s' "$st" | k2sa_status_field session.approved), base ${sbase:0:12}, assets: $sassets). This run wants base ${base:0:12}, assets: $assets_sorted."
            scount="$(printf '%s' "$st" | k2sa_status_field session.count)"
            ssigned="$(printf '%s' "$st" | k2sa_status_field signed)"
            case "$unused" in
                true)
                    # Nothing was signed under it: a new approval replaces it
                    # (the signer checks again before it does).
                    replace=1; oldbase="$sbase"
                    k2sa_log "$version's earlier approval (base ${sbase:0:7}) signed nothing: asking to RE-approve it for this code" ;;
                false)
                    k2sa_fail "$mismatch Investigate before re-approving: something was signed under $version (session count ${scount:-?}; signed: ${ssigned:-none in the registry; see the audit log}), so it is never replaced (the release may be out)."
                    return 1 ;;
                *)
                    k2sa_fail "$mismatch Investigate before re-approving. The installed signer can't replace an unused approval yet: re-install it from this checkout (scripts/k2sign/install.sh), then re-run: ./scripts/release.sh $version"
                    return 1 ;;
            esac
        elif [ "$valid" = true ]; then
            k2sa_log "using $version's approved signing session (until $(printf '%s' "$st" | k2sa_status_field session.expires))"
            return 0
        else
            reapprove=1
            k2sa_log "$version's signing session expired; asking to RE-APPROVE (same base commit)"
        fi
    elif [ "$(printf '%s' "$st" | k2sa_status_field last_version)" = "$version" ] && [ "$unused" = true ]; then
        # Approved before, the session file is gone (moved aside by hand) and
        # nothing was signed: the same replacement.
        replace=1
        k2sa_log "$version was approved before (no session file now) and signed nothing: asking to RE-approve it for this code"
    fi

    local work cf of nonce sid
    work="$(mktemp -d "${TMPDIR:-/tmp}/k2-approve.XXXXXX")" || return 1
    cf="$work/challenge.json"; of="$work/approval.der"
    local flags=()
    [ "$windows" = 1 ] && flags+=(--windows)
    [ -n "$reapprove" ] && flags+=(--reapprove)
    [ -n "$replace" ] && flags+=(--replace)
    if ! k2u_k2sign challenge "$version" "$base" "${assets[@]}" ${flags[@]+"${flags[@]}"} > "$cf"; then
        rm -rf "$work"; k2sa_fail "the signer refused to issue a challenge (above)"; return 1
    fi
    nonce="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["nonce"])' "$cf")" \
        || { rm -rf "$work"; k2sa_fail "the challenge is not JSON"; return 1; }

    local what
    if [ -n "$replace" ]; then
        if [ -n "$oldbase" ]; then
            what="RE-approve K2 $version for new code ${base:0:7} (the earlier approval for ${oldbase:0:7} was never used)"
        else
            what="RE-approve K2 $version for new code ${base:0:7} (an earlier approval of $version was never used)"
        fi
    else
        what="${reapprove:+RE-}approve K2 $version (base ${base:0:7})"
    fi
    printf '\n\033[1;33m══ Approval needed on %s (Touch ID or your login password): %s, %s update files%s ══\033[0m\n' \
        "$(scutil --get LocalHostName 2>/dev/null || hostname -s)" "$what" "${#assets[@]}" "$([ "$windows" = 1 ] && echo ', INCLUDING the Windows setup (no independent check)')" >&2
    k2sa_notify "Touch ID or password: ${replace:+RE-}${reapprove:+RE-}approve K2 $version signing"

    if ! k2sa_run_helper "$cf" "$of" "$version" "${replace:+replace-session}"; then
        rm -rf "$work"; return 1
    fi
    if ! sid="$(k2u_k2sign approve "$nonce" < "$of")"; then
        rm -rf "$work"; k2sa_fail "the signer did not accept the approval (above) — nothing was built. Re-run: ./scripts/release.sh $version"; return 1
    fi
    rm -rf "$work"
    k2sa_log "signing session $sid open for $version"
}
