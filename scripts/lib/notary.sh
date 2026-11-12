# shellcheck shell=bash
# Overlapped notarization for release.sh (0.46.13). k2-node's submission
# goes in beside the app's at Step 4 and is collected at Step 8.55, so its
# 3–5 min wait runs during the app's and the DMG's instead of after them.
#
#   k2_notary_submit_nowait FILE    submit without waiting; prints only the
#                                   submission id
#   k2_notary_wait_accepted ID LABEL
#                                   wait for it; 0 only when Apple says
#                                   Accepted (anything else prints Apple's
#                                   log and fails)
#
# Needs notary_auth_args (release.sh: the App Store Connect API key, else
# the K2SO-notarize keychain profile). bash 3.2-safe. Tested with a stub
# xcrun by tests/packaging/release-mac-overlap.sh.

k2_notary_submit_nowait() {
    local file="$1" out id a auth=()
    while IFS= read -r a; do auth+=("$a"); done < <(notary_auth_args)
    out="$(xcrun notarytool submit "$file" "${auth[@]}" --output-format json)" \
        || { echo "FATAL: notarytool submit $file failed: $out" >&2; return 1; }
    id="$(printf '%s\n' "$out" | grep -Eo '"id" *: *"[0-9a-fA-F-]{36}"' | grep -Eo '[0-9a-fA-F-]{36}' | head -1)"
    [ -n "$id" ] || { echo "FATAL: no submission id from notarytool submit $file: $out" >&2; return 1; }
    printf '%s\n' "$id"
}

k2_notary_wait_accepted() {
    local id="$1" label="$2" out a auth=()
    while IFS= read -r a; do auth+=("$a"); done < <(notary_auth_args)
    out="$(xcrun notarytool wait "$id" "${auth[@]}" --output-format json)" \
        || { echo "FATAL: notarytool wait $id ($label) failed: $out" >&2; return 1; }
    if ! printf '%s\n' "$out" | grep -Eq '"status" *: *"Accepted"'; then
        echo "FATAL: $label notarization $id did not end Accepted: $out" >&2
        xcrun notarytool log "$id" "${auth[@]}" >&2 || true
        return 1
    fi
    echo "  $label notarization $id: Accepted."
}
