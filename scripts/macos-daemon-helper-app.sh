#!/bin/bash
# macOS: wrap k2-daemon in a nested, background-only helper app so Activity
# Monitor shows the K2 icon and "K2 Daemon" instead of a blank executable.
#
#   K2.app/Contents/Helpers/K2 Daemon.app/
#     Contents/Info.plist               generated here (versions from tauri.conf.json)
#     Contents/MacOS/k2-daemon          the daemon binary (CFBundleExecutable)
#     Contents/Resources/K2.icns        src-tauri/icons/icon.icns
#
# Sourced (not executed) by scripts/build-app.sh, scripts/dev-build.sh,
# scripts/release.sh and scripts/build-local.sh. Requires PROJECT_DIR.
#
# The Rust side resolves the same layout: crates/k2-core/src/daemon_lifecycle.rs
# (DAEMON_HELPER_APP_NAME / DAEMON_HELPER_BUNDLE_ID — a k2-daemon test fails
# if the two names below drift). Sidecars (frpc, k2-power-helper, k2-menubar)
# stay in K2.app/Contents/MacOS; the daemon finds them via bundle_sidecar_dir.
#
# Never hand-edit versions: CFBundleShortVersionString/CFBundleVersion come
# from src-tauri/tauri.conf.json, which scripts/release.sh bumps.

K2_DAEMON_HELPER_APP="K2 Daemon.app"
K2_DAEMON_HELPER_BUNDLE_ID="dev.k2.app.daemon"
K2_DAEMON_HELPER_NAME="K2 Daemon"
K2_DAEMON_HELPER_EXE="k2-daemon"
K2_DAEMON_HELPER_ICON="K2.icns"

# Path of the nested helper app inside APP.
k2_daemon_helper_dir() {
    printf '%s/Contents/Helpers/%s' "$1" "$K2_DAEMON_HELPER_APP"
}

# Path of the daemon executable inside APP.
k2_daemon_helper_exe() {
    printf '%s/Contents/MacOS/%s' "$(k2_daemon_helper_dir "$1")" "$K2_DAEMON_HELPER_EXE"
}

_k2_daemon_helper_version() {
    grep -m1 '"version"' "${PROJECT_DIR}/src-tauri/tauri.conf.json" \
        | sed 's/.*: *"\([^"]*\)".*/\1/'
}

# k2_daemon_helper_assemble APP DAEMON_SRC
# Builds K2 Daemon.app inside APP from DAEMON_SRC and removes the pre-0.43.2
# APP/Contents/MacOS/k2-daemon. Exits non-zero on any failure.
k2_daemon_helper_assemble() {
    local app="$1" daemon_src="$2"
    local helper version icon_src
    helper="$(k2_daemon_helper_dir "$app")"
    version="$(_k2_daemon_helper_version)"
    icon_src="${PROJECT_DIR}/src-tauri/icons/icon.icns"

    [ -d "$app/Contents" ] || { echo "FATAL: $app is not an app bundle" >&2; return 1; }
    [ -x "$daemon_src" ] || { echo "FATAL: daemon binary missing at $daemon_src" >&2; return 1; }
    [ -f "$icon_src" ] || { echo "FATAL: K2 icon missing at $icon_src" >&2; return 1; }
    [ -n "$version" ] || { echo "FATAL: no version in src-tauri/tauri.conf.json" >&2; return 1; }

    rm -rf "$helper"
    mkdir -p "$helper/Contents/MacOS" "$helper/Contents/Resources" || return 1
    cp "$daemon_src" "$helper/Contents/MacOS/$K2_DAEMON_HELPER_EXE" || return 1
    chmod 0755 "$helper/Contents/MacOS/$K2_DAEMON_HELPER_EXE" || return 1
    cp "$icon_src" "$helper/Contents/Resources/$K2_DAEMON_HELPER_ICON" || return 1

    cat > "$helper/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>${K2_DAEMON_HELPER_EXE}</string>
    <key>CFBundleIdentifier</key>
    <string>${K2_DAEMON_HELPER_BUNDLE_ID}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>${K2_DAEMON_HELPER_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${K2_DAEMON_HELPER_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${version}</string>
    <key>CFBundleVersion</key>
    <string>${version}</string>
    <key>CFBundleIconFile</key>
    <string>${K2_DAEMON_HELPER_ICON}</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>LSUIElement</key>
    <true/>
</dict>
</plist>
EOF
    plutil -lint "$helper/Contents/Info.plist" >/dev/null \
        || { echo "FATAL: generated $helper/Contents/Info.plist is not a valid plist" >&2; return 1; }

    # Pre-0.43.2 layout. Leaving it would keep a second, stale daemon in the
    # bundle and let an old LaunchAgent plist keep launching it.
    rm -f "$app/Contents/MacOS/$K2_DAEMON_HELPER_EXE"
    echo "  K2 Daemon.app assembled (v${version}, ${K2_DAEMON_HELPER_BUNDLE_ID})."
}

_k2_plist_get() {
    # Never fails: callers compare the value and print their own FATAL
    # (a bare failing $(…) assignment would exit silently under set -e).
    /usr/libexec/PlistBuddy -c "Print :$2" "$1" 2>/dev/null || true
}

# k2_daemon_helper_verify APP
# Fails loudly if the helper app is missing, has no icon, or is malformed.
k2_daemon_helper_verify() {
    local app="$1"
    local helper plist icon exe version want_version
    helper="$(k2_daemon_helper_dir "$app")"
    plist="$helper/Contents/Info.plist"
    want_version="$(_k2_daemon_helper_version)"

    [ -f "$plist" ] || { echo "FATAL: missing $plist" >&2; return 1; }
    plutil -lint "$plist" >/dev/null || { echo "FATAL: $plist is not a valid plist" >&2; return 1; }

    icon="$(_k2_plist_get "$plist" CFBundleIconFile)"
    [ -n "$icon" ] || { echo "FATAL: $plist has no CFBundleIconFile" >&2; return 1; }
    case "$icon" in *.icns) ;; *) icon="${icon}.icns" ;; esac
    [ -s "$helper/Contents/Resources/$icon" ] \
        || { echo "FATAL: helper icon missing at $helper/Contents/Resources/$icon" >&2; return 1; }

    exe="$(_k2_plist_get "$plist" CFBundleExecutable)"
    [ "$exe" = "$K2_DAEMON_HELPER_EXE" ] \
        || { echo "FATAL: CFBundleExecutable is '$exe', want '$K2_DAEMON_HELPER_EXE'" >&2; return 1; }
    [ -x "$helper/Contents/MacOS/$exe" ] \
        || { echo "FATAL: helper executable missing at $helper/Contents/MacOS/$exe" >&2; return 1; }

    [ "$(_k2_plist_get "$plist" CFBundleIdentifier)" = "$K2_DAEMON_HELPER_BUNDLE_ID" ] \
        || { echo "FATAL: CFBundleIdentifier is not $K2_DAEMON_HELPER_BUNDLE_ID" >&2; return 1; }
    [ "$(_k2_plist_get "$plist" CFBundleName)" = "$K2_DAEMON_HELPER_NAME" ] \
        || { echo "FATAL: CFBundleName is not '$K2_DAEMON_HELPER_NAME'" >&2; return 1; }
    [ "$(_k2_plist_get "$plist" CFBundleDisplayName)" = "$K2_DAEMON_HELPER_NAME" ] \
        || { echo "FATAL: CFBundleDisplayName is not '$K2_DAEMON_HELPER_NAME'" >&2; return 1; }
    [ "$(_k2_plist_get "$plist" LSUIElement)" = "true" ] \
        || { echo "FATAL: LSUIElement is not true (helper would show in the Dock)" >&2; return 1; }
    [ "$(_k2_plist_get "$plist" CFBundleShortVersionString)" = "$want_version" ] \
        || { echo "FATAL: helper version does not match tauri.conf.json ($want_version)" >&2; return 1; }
    [ "$(_k2_plist_get "$plist" CFBundleVersion)" = "$want_version" ] \
        || { echo "FATAL: helper CFBundleVersion does not match tauri.conf.json ($want_version)" >&2; return 1; }

    if [ -e "$app/Contents/MacOS/$K2_DAEMON_HELPER_EXE" ]; then
        echo "FATAL: legacy $app/Contents/MacOS/$K2_DAEMON_HELPER_EXE still present" >&2
        return 1
    fi
    echo "  ✓ K2 Daemon.app present: Info.plist, icon $icon, ${K2_DAEMON_HELPER_BUNDLE_ID}, v${want_version}."
}

# k2_daemon_helper_sign APP IDENTITY ENTITLEMENTS
# Signs the nested helper app (its main executable gets the daemon's
# entitlements + hardened runtime). Call BEFORE signing the outer APP.
k2_daemon_helper_sign() {
    local app="$1" identity="$2" entitlements="$3"
    local helper
    helper="$(k2_daemon_helper_dir "$app")"
    codesign --force --options runtime --timestamp \
        --entitlements "$entitlements" \
        --sign "$identity" \
        "$helper" || { echo "FATAL: codesign of $helper failed" >&2; return 1; }
    codesign --verify --strict "$helper" \
        || { echo "FATAL: $helper signature does not verify" >&2; return 1; }
    echo "  Signed K2 Daemon.app."
}

# k2_daemon_helper_verify_signed APP
# After the outer APP is signed: deep-verify, check the helper kept its
# identifier + entitlements, and exec the daemon past AMFI (`--version`
# exits 0 without starting the server; rc 137 = AMFI SIGKILL).
k2_daemon_helper_verify_signed() {
    local app="$1"
    local helper exe out rc info ents
    helper="$(k2_daemon_helper_dir "$app")"
    exe="$(k2_daemon_helper_exe "$app")"

    codesign --verify --deep --strict "$app" \
        || { echo "FATAL: $app does not deep-verify after signing" >&2; return 1; }
    # Capture first, then grep: `codesign | grep -q` can SIGPIPE under pipefail.
    info="$(codesign -dv "$helper" 2>&1)" \
        || { echo "FATAL: codesign -dv $helper failed: $info" >&2; return 1; }
    ents="$(codesign -d --entitlements - "$helper" 2>/dev/null)" \
        || { echo "FATAL: cannot read helper entitlements" >&2; return 1; }
    grep -q "^Identifier=${K2_DAEMON_HELPER_BUNDLE_ID}\$" <<<"$info" \
        || { echo "FATAL: helper code identifier is not ${K2_DAEMON_HELPER_BUNDLE_ID}" >&2; return 1; }
    grep -q "flags=.*runtime" <<<"$info" \
        || { echo "FATAL: helper is not signed with the hardened runtime" >&2; return 1; }
    grep -q "com.apple.security.cs.disable-library-validation" <<<"$ents" \
        || { echo "FATAL: helper lost the daemon entitlements" >&2; return 1; }

    rc=0
    out="$("$exe" --version 2>&1)" || rc=$?
    if [ "$rc" -eq 137 ]; then
        echo "FATAL: signed k2-daemon SIGKILL'd at exec (137 = AMFI). Output: ${out:0:400}" >&2
        return 1
    fi
    [ "$rc" -eq 0 ] || { echo "FATAL: signed k2-daemon --version exited rc=$rc: ${out:0:400}" >&2; return 1; }
    echo "  ✓ K2 Daemon.app signed (runtime + entitlements); daemon execs past AMFI: $out"
}
