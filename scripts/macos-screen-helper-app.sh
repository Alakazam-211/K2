#!/bin/bash
# macOS: K2 Screen.app, the remote desktop helper (prd-remote-desktop-tab-v1
# §3.2, D3), nested as a background-only app so System Settings lists its
# Screen Recording and Accessibility grants as "K2 Screen" with the K2 icon.
#
#   K2.app/Contents/Helpers/K2 Screen.app/
#     Contents/Info.plist               generated here (versions from tauri.conf.json)
#     Contents/MacOS/k2-deskd           the helper (CFBundleExecutable)
#     Contents/Resources/K2.icns        src-tauri/icons/icon.icns
#
# The daemon installs a LaunchAgent (dev.k2.deskd) that starts this
# executable, so launchd makes it its own responsible process: TCC credits
# its grants to K2 Screen, never to the daemon (crates/k2-daemon/src/desktop).
# The Rust side resolves the same path: launch_agent::screen_exe_for (beside
# K2 Daemon.app, both under Contents/Helpers).
#
# Signing (the normal setup for a signed helper, vs-live R4 as Rosson kept
# it): hardened runtime, identifier dev.k2.app.screen, NO entitlements (it
# is Rust plus system frameworks, so library validation stays on). When
# Apple grants com.apple.developer.persistent-content-capture (§11), add
# src-tauri/entitlements-screen.plist and an embedded provisioning profile;
# k2_screen_helper_sign picks the plist up and the AMFI exec check below
# catches a missing profile. TCC grants follow the designated requirement
# (bundle id + team), so never change K2_SCREEN_HELPER_BUNDLE_ID.
#
# Sourced by scripts/macos-daemon-helper-app.sh (so every caller of that
# file has these too). Requires PROJECT_DIR. Never hand-edit versions.

K2_SCREEN_HELPER_APP="K2 Screen.app"
K2_SCREEN_HELPER_BUNDLE_ID="dev.k2.app.screen"
K2_SCREEN_HELPER_NAME="K2 Screen"
K2_SCREEN_HELPER_EXE="k2-deskd"
K2_SCREEN_HELPER_ICON="K2.icns"
# A debug k2-deskd also serves dev-signed daemons; it must never ship.
K2_SCREEN_DEV_MARKER="K2_DESKD_DEV_PEER_REQUIREMENT"
# Entitlements K2 Screen must never carry (it holds Screen Recording and
# Accessibility; nothing may attach to it or load code into it).
K2_SCREEN_FORBIDDEN_ENTITLEMENTS="get-task-allow|allow-dyld-environment-variables|disable-library-validation|allow-jit|allow-unsigned-executable-memory"

k2_screen_helper_dir() {
    printf '%s/Contents/Helpers/%s' "$1" "$K2_SCREEN_HELPER_APP"
}

k2_screen_helper_exe() {
    printf '%s/Contents/MacOS/%s' "$(k2_screen_helper_dir "$1")" "$K2_SCREEN_HELPER_EXE"
}

_k2_screen_helper_version() {
    grep -m1 '"version"' "${PROJECT_DIR}/src-tauri/tauri.conf.json" \
        | sed 's/.*: *"\([^"]*\)".*/\1/'
}

_k2_screen_plist_get() {
    /usr/libexec/PlistBuddy -c "Print :$2" "$1" 2>/dev/null || true
}

# k2_screen_helper_assemble APP DESKD_SRC
k2_screen_helper_assemble() {
    local app="$1" deskd_src="$2"
    local helper version icon_src
    helper="$(k2_screen_helper_dir "$app")"
    version="$(_k2_screen_helper_version)"
    icon_src="${PROJECT_DIR}/src-tauri/icons/icon.icns"

    [ -d "$app/Contents" ] || { echo "FATAL: $app is not an app bundle" >&2; return 1; }
    [ -n "$deskd_src" ] && [ -x "$deskd_src" ] \
        || { echo "FATAL: k2-deskd binary missing at '${deskd_src}' (cargo build --release -p k2-deskd --bin k2-deskd)" >&2; return 1; }
    [ -f "$icon_src" ] || { echo "FATAL: K2 icon missing at $icon_src" >&2; return 1; }
    [ -n "$version" ] || { echo "FATAL: no version in src-tauri/tauri.conf.json" >&2; return 1; }

    rm -rf "$helper"
    mkdir -p "$helper/Contents/MacOS" "$helper/Contents/Resources" || return 1
    cp "$deskd_src" "$helper/Contents/MacOS/$K2_SCREEN_HELPER_EXE" || return 1
    chmod 0755 "$helper/Contents/MacOS/$K2_SCREEN_HELPER_EXE" || return 1
    cp "$icon_src" "$helper/Contents/Resources/$K2_SCREEN_HELPER_ICON" || return 1

    cat > "$helper/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>${K2_SCREEN_HELPER_EXE}</string>
    <key>CFBundleIdentifier</key>
    <string>${K2_SCREEN_HELPER_BUNDLE_ID}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>${K2_SCREEN_HELPER_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${K2_SCREEN_HELPER_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${version}</string>
    <key>CFBundleVersion</key>
    <string>${version}</string>
    <key>CFBundleIconFile</key>
    <string>${K2_SCREEN_HELPER_ICON}</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
</dict>
</plist>
EOF
    plutil -lint "$helper/Contents/Info.plist" >/dev/null \
        || { echo "FATAL: generated $helper/Contents/Info.plist is not a valid plist" >&2; return 1; }
    echo "  K2 Screen.app assembled (v${version}, ${K2_SCREEN_HELPER_BUNDLE_ID})."
}

# k2_screen_helper_verify APP: the unsigned layout.
k2_screen_helper_verify() {
    local app="$1"
    local helper plist icon exe want_version kind
    helper="$(k2_screen_helper_dir "$app")"
    plist="$helper/Contents/Info.plist"
    want_version="$(_k2_screen_helper_version)"

    [ -f "$plist" ] || { echo "FATAL: missing $plist" >&2; return 1; }
    plutil -lint "$plist" >/dev/null || { echo "FATAL: $plist is not a valid plist" >&2; return 1; }
    icon="$(_k2_screen_plist_get "$plist" CFBundleIconFile)"
    [ -s "$helper/Contents/Resources/$icon" ] \
        || { echo "FATAL: K2 Screen icon missing at $helper/Contents/Resources/$icon" >&2; return 1; }
    exe="$(_k2_screen_plist_get "$plist" CFBundleExecutable)"
    [ "$exe" = "$K2_SCREEN_HELPER_EXE" ] \
        || { echo "FATAL: K2 Screen CFBundleExecutable is '$exe', want '$K2_SCREEN_HELPER_EXE'" >&2; return 1; }
    [ -x "$helper/Contents/MacOS/$exe" ] \
        || { echo "FATAL: K2 Screen executable missing at $helper/Contents/MacOS/$exe" >&2; return 1; }
    kind="$(file -b "$helper/Contents/MacOS/$exe")"
    case "$kind" in *Mach-O*) ;; *) echo "FATAL: $helper/Contents/MacOS/$exe is not a Mach-O executable: $kind" >&2; return 1 ;; esac
    [ "$(_k2_screen_plist_get "$plist" CFBundleIdentifier)" = "$K2_SCREEN_HELPER_BUNDLE_ID" ] \
        || { echo "FATAL: K2 Screen CFBundleIdentifier is not $K2_SCREEN_HELPER_BUNDLE_ID" >&2; return 1; }
    [ "$(_k2_screen_plist_get "$plist" CFBundleName)" = "$K2_SCREEN_HELPER_NAME" ] \
        || { echo "FATAL: K2 Screen CFBundleName is not '$K2_SCREEN_HELPER_NAME'" >&2; return 1; }
    [ "$(_k2_screen_plist_get "$plist" LSUIElement)" = "true" ] \
        || { echo "FATAL: K2 Screen LSUIElement is not true (it would show in the Dock)" >&2; return 1; }
    [ "$(_k2_screen_plist_get "$plist" CFBundleShortVersionString)" = "$want_version" ] \
        || { echo "FATAL: K2 Screen version does not match tauri.conf.json ($want_version)" >&2; return 1; }
    if grep -a -q "$K2_SCREEN_DEV_MARKER" "$helper/Contents/MacOS/$exe"; then
        echo "FATAL: $helper/Contents/MacOS/$exe is a debug k2-deskd (serves dev-signed daemons); build it --release" >&2
        return 1
    fi
    if [ -e "$app/Contents/MacOS/$K2_SCREEN_HELPER_EXE" ]; then
        echo "FATAL: stray $app/Contents/MacOS/$K2_SCREEN_HELPER_EXE (it belongs in $K2_SCREEN_HELPER_APP)" >&2
        return 1
    fi
    echo "  ✓ K2 Screen.app present: Info.plist, icon, ${K2_SCREEN_HELPER_BUNDLE_ID}, LSUIElement, v${want_version}."
}

# k2_screen_helper_sign APP IDENTITY
# Hardened runtime, no entitlements (unless src-tauri/entitlements-screen.plist
# exists: the persistent-content-capture entitlement, once Apple grants it).
# Call BEFORE signing the outer APP (its seal covers this bundle).
k2_screen_helper_sign() {
    local app="$1" identity="$2" helper ents_file
    helper="$(k2_screen_helper_dir "$app")"
    [ -d "$helper" ] || { echo "FATAL: $helper missing (assemble first)" >&2; return 1; }
    ents_file="${PROJECT_DIR}/src-tauri/entitlements-screen.plist"
    if [ -f "$ents_file" ]; then
        codesign --force --options runtime --timestamp \
            --identifier "$K2_SCREEN_HELPER_BUNDLE_ID" \
            --entitlements "$ents_file" \
            --sign "$identity" \
            "$helper" || { echo "FATAL: codesign of $helper failed" >&2; return 1; }
    else
        codesign --force --options runtime --timestamp \
            --identifier "$K2_SCREEN_HELPER_BUNDLE_ID" \
            --sign "$identity" \
            "$helper" || { echo "FATAL: codesign of $helper failed" >&2; return 1; }
    fi
    codesign --verify --strict "$helper" \
        || { echo "FATAL: $helper signature does not verify" >&2; return 1; }
    echo "  Signed K2 Screen.app."
}

# k2_screen_helper_verify_signed APP
# After the outer APP is signed: identifier, hardened runtime, the same team
# as the daemon, no forbidden entitlement, and it execs past AMFI
# (`--version` exits 0 without starting; rc 137 = AMFI SIGKILL).
k2_screen_helper_verify_signed() {
    local app="$1" helper exe info ents team dteam out rc dinfo
    helper="$(k2_screen_helper_dir "$app")"
    exe="$(k2_screen_helper_exe "$app")"
    codesign --verify --strict "$helper" \
        || { echo "FATAL: K2 Screen.app does not verify after signing" >&2; return 1; }
    info="$(codesign -dv "$helper" 2>&1)" \
        || { echo "FATAL: codesign -dv $helper failed: $info" >&2; return 1; }
    grep -q "^Identifier=${K2_SCREEN_HELPER_BUNDLE_ID}\$" <<<"$info" \
        || { echo "FATAL: K2 Screen code identifier is not ${K2_SCREEN_HELPER_BUNDLE_ID}" >&2; return 1; }
    grep -q "flags=.*runtime" <<<"$info" \
        || { echo "FATAL: K2 Screen is not signed with the hardened runtime" >&2; return 1; }
    team="$(sed -n 's/^TeamIdentifier=//p' <<<"$info")"
    if declare -F k2_daemon_helper_dir >/dev/null; then
        dinfo="$(codesign -dv "$(k2_daemon_helper_dir "$app")" 2>&1)" || dinfo=""
        dteam="$(sed -n 's/^TeamIdentifier=//p' <<<"$dinfo")"
        { [ -n "$team" ] && [ "$team" != "not set" ] && [ "$team" = "$dteam" ]; } \
            || { echo "FATAL: K2 Screen Team ID '$team' differs from the daemon's '$dteam' (deskd only serves K2's team)" >&2; return 1; }
    fi
    ents="$(codesign -d --entitlements - "$helper" 2>/dev/null)" \
        || { echo "FATAL: cannot read K2 Screen entitlements" >&2; return 1; }
    if grep -qE "$K2_SCREEN_FORBIDDEN_ENTITLEMENTS" <<<"$ents"; then
        echo "FATAL: K2 Screen carries an entitlement it must not have ($K2_SCREEN_FORBIDDEN_ENTITLEMENTS)" >&2
        return 1
    fi
    rc=0
    out="$("$exe" --version 2>&1)" || rc=$?
    if [ "$rc" -eq 137 ]; then
        echo "FATAL: signed k2-deskd SIGKILL'd at exec (137 = AMFI; a restricted entitlement without its profile?). Output: ${out:0:400}" >&2
        return 1
    fi
    [ "$rc" -eq 0 ] || { echo "FATAL: signed k2-deskd --version exited rc=$rc: ${out:0:400}" >&2; return 1; }
    echo "  ✓ K2 Screen.app signed (runtime, no forbidden entitlement, team $team); execs past AMFI: $out"
}

# k2_screen_helper_verify_shipped APP: after notarization + staple.
k2_screen_helper_verify_shipped() {
    local app="$1" helper info
    helper="$(k2_screen_helper_dir "$app")"
    [ -x "$(k2_screen_helper_exe "$app")" ] || { echo "FATAL: the notarized app has no K2 Screen at $helper" >&2; return 1; }
    codesign --verify --strict "$helper" \
        || { echo "FATAL: K2 Screen in the notarized app does not verify" >&2; return 1; }
    info="$(codesign -dv "$helper" 2>&1)" \
        || { echo "FATAL: codesign -dv $helper failed: $info" >&2; return 1; }
    grep -q "^Identifier=${K2_SCREEN_HELPER_BUNDLE_ID}\$" <<<"$info" \
        || { echo "FATAL: K2 Screen in the notarized app is not ${K2_SCREEN_HELPER_BUNDLE_ID}" >&2; return 1; }
    spctl --assess --type execute "$app" \
        || { echo "FATAL: Gatekeeper rejects the notarized app (with K2 Screen)" >&2; return 1; }
    echo "  ✓ Notarized app ships a signed K2 Screen.app (Gatekeeper accepts it)."
}

# k2_screen_helper_verify_tarball TARBALL: the update bundle carries it.
k2_screen_helper_verify_tarball() {
    local tgz="$1" want listing
    want="K2.app/Contents/Helpers/${K2_SCREEN_HELPER_APP}/Contents/MacOS/${K2_SCREEN_HELPER_EXE}"
    listing="$(tar -tvzf "$tgz")" \
        || { echo "FATAL: cannot list $tgz" >&2; return 1; }
    grep -F -- "$want" <<<"$listing" | grep -q '^-rwx' \
        || { echo "FATAL: $tgz has no executable $want" >&2; return 1; }
    echo "  ✓ Update bundle carries ${want}."
}
