#!/bin/bash
# macOS: wrap k2-daemon in a nested, background-only helper app so Activity
# Monitor shows the K2 icon and "K2 Daemon" instead of a blank executable.
#
#   K2.app/Contents/Helpers/K2 Daemon.app/
#     Contents/Info.plist               generated here (versions from tauri.conf.json)
#     Contents/MacOS/k2-daemon          the daemon binary (CFBundleExecutable)
#     Contents/MacOS/k2-ptyd            the PTY host (k2-ptyd PRD P84/§7.1): the
#                                       daemon finds it beside its own exe
#                                       (k2_core terminal::ptyd::client::shipped_binary)
#     Contents/MacOS/k2-export          the export worker (prd-k2-core-exports-v1,
#                                       vs-live §3.1): beside the daemon, like k2-ptyd
#                                       (k2-daemon docs::export_worker::binary)
#     Contents/Resources/K2.icns        src-tauri/icons/icon.icns
#
# Sourced (not executed) by scripts/build-app.sh, scripts/dev-build.sh,
# scripts/release.sh and scripts/build-local.sh. Requires PROJECT_DIR.
#
# The Rust side resolves the same layout: crates/k2-client/src/daemon_lifecycle.rs
# (DAEMON_HELPER_APP_NAME / DAEMON_HELPER_BUNDLE_ID — a k2-daemon test fails
# if the two names below drift). Sidecars (frpc, k2-power-helper, k2-menubar)
# stay in K2.app/Contents/MacOS; the daemon finds them via bundle_sidecar_dir.
#
# Never hand-edit versions: CFBundleShortVersionString/CFBundleVersion come
# from src-tauri/tauri.conf.json, which scripts/release.sh bumps.

# K2 Screen.app (the remote desktop helper) sits beside K2 Daemon.app;
# every caller of this file gets its functions too.
# shellcheck source=scripts/macos-screen-helper-app.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/macos-screen-helper-app.sh"

K2_DAEMON_HELPER_APP="K2 Daemon.app"
K2_DAEMON_HELPER_BUNDLE_ID="dev.k2.app.daemon"
K2_DAEMON_HELPER_NAME="K2 Daemon"
K2_DAEMON_HELPER_EXE="k2-daemon"
K2_DAEMON_HELPER_ICON="K2.icns"
K2_DAEMON_HELPER_PTYD="k2-ptyd"
K2_DAEMON_HELPER_EXPORT="k2-export"
# A test-hostile k2-export build carries this; nothing that ships may.
K2_EXPORT_HOSTILE_MARKER="K2_EXPORT_TEST_HOSTILE_ENABLED"

# The ONE cargo call that builds every Rust sidecar K2.app ships on macOS
# (scripts/lib/build-binaries.sh, Step 2 of release.sh) and the standalone
# daemon rebuild (k2_build_standalone_macos_daemon, Step 8.5). One package
# set means one feature resolution: k2-core and the ~500 crates under it
# compile once, and the Step 8.5 rebuild reuses all of them (only
# k2-daemon relinks). A daemon built with a different package set (e.g.
# `-p k2-daemon` alone) resolves objc2-foundation (under trash, so under
# k2-core) and tungstenite differently and recompiles k2-core. Keep the two
# callers on this array; tests/packaging/build-once.sh fails if either
# grows its own list. Release profile and --release come from the callers.
K2_MAC_SIDECAR_CARGO_ARGS=(
    -p k2-daemon --bin k2-daemon --bin k2-power-helper
    -p k2-ptyd --bin k2-ptyd
    -p k2-export --bin k2-export
    -p k2-node --bin k2-node
    -p k2-menubar --bin k2-menubar
    -p k2-deskd --bin k2-deskd
)

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

# Path of the nested k2-ptyd inside APP.
k2_daemon_helper_ptyd() {
    printf '%s/Contents/MacOS/%s' "$(k2_daemon_helper_dir "$1")" "$K2_DAEMON_HELPER_PTYD"
}

# Path of the nested k2-export inside APP.
k2_daemon_helper_export() {
    printf '%s/Contents/MacOS/%s' "$(k2_daemon_helper_dir "$1")" "$K2_DAEMON_HELPER_EXPORT"
}

# k2_daemon_helper_assemble APP DAEMON_SRC PTYD_SRC EXPORT_SRC
# Builds K2 Daemon.app inside APP from DAEMON_SRC (+ k2-ptyd from PTYD_SRC
# and k2-export from EXPORT_SRC, nested beside the daemon) and removes the
# pre-0.43.2 APP/Contents/MacOS/k2-daemon. Exits non-zero on any failure.
k2_daemon_helper_assemble() {
    local app="$1" daemon_src="$2" ptyd_src="$3" export_src="$4"
    local helper version icon_src
    helper="$(k2_daemon_helper_dir "$app")"
    version="$(_k2_daemon_helper_version)"
    icon_src="${PROJECT_DIR}/src-tauri/icons/icon.icns"

    [ -d "$app/Contents" ] || { echo "FATAL: $app is not an app bundle" >&2; return 1; }
    [ -x "$daemon_src" ] || { echo "FATAL: daemon binary missing at $daemon_src" >&2; return 1; }
    [ -n "$ptyd_src" ] && [ -x "$ptyd_src" ] \
        || { echo "FATAL: k2-ptyd binary missing at '${ptyd_src}' (cargo build --release -p k2-ptyd --bin k2-ptyd)" >&2; return 1; }
    [ -n "$export_src" ] && [ -x "$export_src" ] \
        || { echo "FATAL: k2-export binary missing at '${export_src}' (cargo build --release -p k2-export --bin k2-export)" >&2; return 1; }
    [ -f "$icon_src" ] || { echo "FATAL: K2 icon missing at $icon_src" >&2; return 1; }
    [ -n "$version" ] || { echo "FATAL: no version in src-tauri/tauri.conf.json" >&2; return 1; }

    rm -rf "$helper"
    mkdir -p "$helper/Contents/MacOS" "$helper/Contents/Resources" || return 1
    cp "$daemon_src" "$helper/Contents/MacOS/$K2_DAEMON_HELPER_EXE" || return 1
    chmod 0755 "$helper/Contents/MacOS/$K2_DAEMON_HELPER_EXE" || return 1
    cp "$ptyd_src" "$helper/Contents/MacOS/$K2_DAEMON_HELPER_PTYD" || return 1
    chmod 0755 "$helper/Contents/MacOS/$K2_DAEMON_HELPER_PTYD" || return 1
    cp "$export_src" "$helper/Contents/MacOS/$K2_DAEMON_HELPER_EXPORT" || return 1
    chmod 0755 "$helper/Contents/MacOS/$K2_DAEMON_HELPER_EXPORT" || return 1
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
    <string>11.0</string>
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
    echo "  K2 Daemon.app assembled (v${version}, ${K2_DAEMON_HELPER_BUNDLE_ID}, + ${K2_DAEMON_HELPER_PTYD}, ${K2_DAEMON_HELPER_EXPORT})."
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
    # k2-ptyd rides beside the daemon (never in K2.app/Contents/MacOS: the
    # daemon only looks next to its own executable).
    # Not exec'd here: an unsigned Mach-O inside an app bundle can sit in
    # dyld start-up waiting on the first-launch policy check. The signed
    # check (k2_daemon_helper_verify_signed) execs it.
    local ptyd ptyd_kind
    ptyd="$(k2_daemon_helper_ptyd "$app")"
    [ -x "$ptyd" ] || { echo "FATAL: k2-ptyd missing at $ptyd" >&2; return 1; }
    ptyd_kind="$(file -b "$ptyd")"
    case "$ptyd_kind" in *Mach-O*) ;; *) echo "FATAL: $ptyd is not a Mach-O executable: $ptyd_kind" >&2; return 1 ;; esac
    if [ -e "$app/Contents/MacOS/$K2_DAEMON_HELPER_PTYD" ]; then
        echo "FATAL: stray $app/Contents/MacOS/$K2_DAEMON_HELPER_PTYD (it belongs in $K2_DAEMON_HELPER_APP)" >&2
        return 1
    fi
    # k2-export rides beside the daemon too, and is never a test-hostile build.
    local exp exp_kind
    exp="$(k2_daemon_helper_export "$app")"
    [ -x "$exp" ] || { echo "FATAL: k2-export missing at $exp" >&2; return 1; }
    exp_kind="$(file -b "$exp")"
    case "$exp_kind" in *Mach-O*) ;; *) echo "FATAL: $exp is not a Mach-O executable: $exp_kind" >&2; return 1 ;; esac
    if grep -a -q "$K2_EXPORT_HOSTILE_MARKER" "$exp"; then
        echo "FATAL: $exp is a test-hostile build (cargo --features test-hostile); it must never ship" >&2
        return 1
    fi
    if [ -e "$app/Contents/MacOS/$K2_DAEMON_HELPER_EXPORT" ]; then
        echo "FATAL: stray $app/Contents/MacOS/$K2_DAEMON_HELPER_EXPORT (it belongs in $K2_DAEMON_HELPER_APP)" >&2
        return 1
    fi
    echo "  ✓ K2 Daemon.app present: Info.plist, icon $icon, ${K2_DAEMON_HELPER_BUNDLE_ID}, v${want_version}, ${K2_DAEMON_HELPER_PTYD}, ${K2_DAEMON_HELPER_EXPORT}."
}

# k2_daemon_helper_sign APP IDENTITY ENTITLEMENTS
# Signs the nested k2-ptyd and k2-export (hardened runtime, NO
# entitlements: ptyd only forks/execs agents, the export worker only reads
# stdin and writes stdout; both link system libraries only), then the
# helper app (its main executable gets the daemon's entitlements + hardened
# runtime). The helper's seal covers both, so they must be signed first.
# Call BEFORE signing the outer APP.
k2_daemon_helper_sign() {
    local app="$1" identity="$2" entitlements="$3"
    local helper ptyd exp
    helper="$(k2_daemon_helper_dir "$app")"
    ptyd="$(k2_daemon_helper_ptyd "$app")"
    exp="$(k2_daemon_helper_export "$app")"
    [ -x "$ptyd" ] || { echo "FATAL: k2-ptyd missing at $ptyd (assemble first)" >&2; return 1; }
    [ -x "$exp" ] || { echo "FATAL: k2-export missing at $exp (assemble first)" >&2; return 1; }
    codesign --force --options runtime --timestamp \
        --identifier "dev.k2.app.ptyd" \
        --sign "$identity" \
        "$ptyd" || { echo "FATAL: codesign of $ptyd failed" >&2; return 1; }
    codesign --verify --strict "$ptyd" \
        || { echo "FATAL: $ptyd signature does not verify" >&2; return 1; }
    codesign --force --options runtime --timestamp \
        --identifier "dev.k2.app.export" \
        --sign "$identity" \
        "$exp" || { echo "FATAL: codesign of $exp failed" >&2; return 1; }
    codesign --verify --strict "$exp" \
        || { echo "FATAL: $exp signature does not verify" >&2; return 1; }
    codesign --force --options runtime --timestamp \
        --entitlements "$entitlements" \
        --sign "$identity" \
        "$helper" || { echo "FATAL: codesign of $helper failed" >&2; return 1; }
    codesign --verify --strict "$helper" \
        || { echo "FATAL: $helper signature does not verify" >&2; return 1; }
    echo "  Signed K2 Daemon.app (+ k2-ptyd, k2-export)."
}

# k2_build_standalone_macos_daemon APP OUT [PROJECT_DIR]
# k2-ptyd (prd-k2-ptyd-v1 P84) for the STANDALONE macOS daemon download
# (`k2-daemon-macos-aarch64`: install-daemon.sh, `k2 daemon install`, the
# daemon self-update). Those fetch only `k2-daemon`, so — as on Linux — the
# daemon carries k2-ptyd inside it: build.rs embeds the file K2_PTYD_EMBED
# names, and at run time the daemon writes it to
# ~/.k2/bin/ptyd/<ver>-<sha12>/k2-ptyd and launches that.
#
# The embedded bytes are the k2-ptyd ALREADY SIGNED in APP (Step 3:
# Developer ID, hardened runtime, no entitlements, dev.k2.app.ptyd), so the
# copy the daemon writes keeps that signature. Call it after Step 3. The
# app-bundled daemon is untouched: it was copied into K2 Daemon.app and
# signed before this rebuild, embeds nothing, and keeps using the nested
# k2-ptyd beside it.
#
# What this rebuild costs: every other crate is reused (only k2-daemon
# compiles), but it is NOT a cheap relink. The embed is include_bytes! in
# the k2-daemon LIBRARY (ptyd_status.rs, docs/export_worker.rs) and build.rs
# reruns on the K2_PTYD_EMBED change, so the whole k2-daemon crate
# recompiles, and with `lto = true` + `codegen-units = 1` its link is a
# whole-program, single-threaded LLVM run: the 15–18 min on z3mbpZ.
# release.sh therefore runs it in the background during the notarization
# waits (k2_standalone_macos_daemon_start after Step 3,
# k2_standalone_macos_daemon_finish at Step 8.5); a resumed run
# (K2_RELEASE_RESUME) still calls this one in the foreground.
k2_build_standalone_macos_daemon() {
    local app="$1" out="$2" project="${3:-$PWD}"
    _k2_standalone_embeds_signed "$app" || return 1
    _k2_standalone_cargo "$(k2_daemon_helper_ptyd "$app")" "$(k2_daemon_helper_export "$app")" "$out" "$project" || return 1
    echo "  ✓ Standalone macOS daemon embeds the signed k2-ptyd and k2-export: $out"
}

# The nested k2-ptyd and k2-export in APP are validly signed with their
# identifiers; nothing else is ever embedded.
_k2_standalone_embeds_signed() {
    local app="$1" ptyd pinfo exp einfo
    ptyd="$(k2_daemon_helper_ptyd "$app")"
    exp="$(k2_daemon_helper_export "$app")"
    [ -x "$ptyd" ] || { echo "FATAL: no signed k2-ptyd at $ptyd (sign the app first)" >&2; return 1; }
    codesign --verify --strict "$ptyd" \
        || { echo "FATAL: $ptyd is not validly signed; refusing to embed it" >&2; return 1; }
    pinfo="$(codesign -dv "$ptyd" 2>&1)" \
        || { echo "FATAL: codesign -dv $ptyd failed: $pinfo" >&2; return 1; }
    grep -q "^Identifier=dev.k2.app.ptyd\$" <<<"$pinfo" \
        || { echo "FATAL: $ptyd is not dev.k2.app.ptyd; refusing to embed it" >&2; return 1; }
    # The export worker rides inside it the same way (vs-live §3.1).
    [ -x "$exp" ] || { echo "FATAL: no signed k2-export at $exp (sign the app first)" >&2; return 1; }
    codesign --verify --strict "$exp" \
        || { echo "FATAL: $exp is not validly signed; refusing to embed it" >&2; return 1; }
    einfo="$(codesign -dv "$exp" 2>&1)" \
        || { echo "FATAL: codesign -dv $exp failed: $einfo" >&2; return 1; }
    grep -q "^Identifier=dev.k2.app.export\$" <<<"$einfo" \
        || { echo "FATAL: $exp is not dev.k2.app.export; refusing to embed it" >&2; return 1; }
}

# _k2_standalone_cargo PTYD EXPORT OUT PROJECT: the embed build itself.
_k2_standalone_cargo() {
    local ptyd="$1" exp="$2" out="$3" project="$4"
    (
        cd "$project" || exit 1
        # shellcheck source=linux-ptyd-embed.sh
        . "$project/scripts/linux-ptyd-embed.sh"
        # The same environment and package set as Step 2's build
        # (K2_MAC_SIDECAR_CARGO_ARGS), so only the embed differs: k2-core
        # and every other crate are reused; k2-daemon recompiles and links.
        if declare -F k2_mac_build_env >/dev/null; then k2_mac_build_env; fi
        K2_PTYD_EMBED="$ptyd"
        K2_REQUIRE_PTYD_EMBED=1
        K2_EXPORT_EMBED="$exp"
        K2_REQUIRE_EXPORT_EMBED=1
        export K2_PTYD_EMBED K2_REQUIRE_PTYD_EMBED K2_EXPORT_EMBED K2_REQUIRE_EXPORT_EMBED
        cargo build --release "${K2_MAC_SIDECAR_CARGO_ARGS[@]}" \
            || { echo "FATAL: standalone k2-daemon build (embedding k2-ptyd) failed" >&2; exit 1; }
        assert_daemon_embeds_ptyd "${CARGO_TARGET_DIR:-target}/release/k2-daemon" || exit 1
        assert_daemon_embeds_export "${CARGO_TARGET_DIR:-target}/release/k2-daemon" || exit 1
        if declare -F assert_daemon_oauth_not_placeholder >/dev/null; then
            assert_daemon_oauth_not_placeholder "${CARGO_TARGET_DIR:-target}/release/k2-daemon" || exit 1
        fi
        cp "${CARGO_TARGET_DIR:-target}/release/k2-daemon" "$out" || exit 1
    )
}

# ── The standalone daemon in the background (release.sh, 0.46.13) ──
# k2_standalone_macos_daemon_start APP STAGE_DIR [PROJECT_DIR]
#   Right after Step 3: the same signature checks as the foreground build,
#   then COPIES the signed k2-ptyd and k2-export to STAGE_DIR/embed/ (so
#   nothing that happens to the app later can change what gets embedded)
#   and starts the embed build in the background: the daemon lands at
#   STAGE_DIR/k2-daemon, the build's output in STAGE_DIR/build.log. Sets
#   K2_STANDALONE_PID. STAGE_DIR must be absolute.
# k2_standalone_macos_daemon_poll STAGE_DIR
#   Between steps: 0 while it runs or after it succeeded; 1 (with the log's
#   tail) once it has failed, so the release stops before the next wait.
# k2_standalone_macos_daemon_finish APP STAGE_DIR OUT
#   Step 8.5: waits for it, then checks the daemon against the k2-ptyd and
#   k2-export that SHIP (the stapled app's: the embedded copies must be the
#   same bytes) and copies it to OUT. Fatal on any mismatch.
# k2_standalone_macos_daemon_kill
#   For the release's EXIT trap: stop a build nobody will collect.
# The background build never touches K2.app or anything Steps 3.5–8 read:
# it writes target/release/{k2-daemon,k2-power-helper,deps,…} and
# STAGE_DIR. No other cargo runs in those steps, so the target dir is its.
_K2_HELPER_SCRIPTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Stop a background job and everything it started (cargo → rustc; bun →
# tauri → cargo). bash 3.2-safe: walks pgrep -P, children first. Also used
# by scripts/lib/build-binaries.sh.
k2_kill_tree() {
    local pid="$1" kid
    for kid in $(pgrep -P "$pid" 2>/dev/null); do k2_kill_tree "$kid"; done
    kill "$pid" 2>/dev/null || true
}

K2_STANDALONE_PID=""
K2_STANDALONE_RC=""
k2_standalone_macos_daemon_start() {
    local app="$1" stage="$2" project="${3:-$PWD}"
    case "$stage" in /*) ;; *) echo "FATAL: k2_standalone_macos_daemon_start: STAGE_DIR must be absolute ($stage)" >&2; return 1 ;; esac
    _k2_standalone_embeds_signed "$app" || return 1
    rm -rf "$stage"
    mkdir -p "$stage/embed" || return 1
    cp -p "$(k2_daemon_helper_ptyd "$app")" "$stage/embed/k2-ptyd" || return 1
    cp -p "$(k2_daemon_helper_export "$app")" "$stage/embed/k2-export" || return 1
    K2_STANDALONE_RC=""
    _k2_standalone_cargo "$stage/embed/k2-ptyd" "$stage/embed/k2-export" "$stage/k2-daemon" "$project" \
        > "$stage/build.log" 2>&1 &
    K2_STANDALONE_PID=$!
    echo "  Standalone macOS daemon (Step 8.5's) now builds in the background (pid ${K2_STANDALONE_PID}; log $stage/build.log)."
}

_k2_standalone_reap() {
    local stage="$1" rc=0
    [ -n "$K2_STANDALONE_PID" ] || return 0
    wait "$K2_STANDALONE_PID" || rc=$?
    K2_STANDALONE_PID=""
    K2_STANDALONE_RC="$rc"
    if [ "$rc" != 0 ]; then
        echo "FATAL: the background standalone daemon build failed (exit $rc). Last lines of $stage/build.log:" >&2
        tail -40 "$stage/build.log" >&2
    fi
    return 0
}

k2_standalone_macos_daemon_poll() {
    local stage="$1"
    if [ -n "$K2_STANDALONE_PID" ] && ! kill -0 "$K2_STANDALONE_PID" 2>/dev/null; then
        _k2_standalone_reap "$stage"
    fi
    [ -z "$K2_STANDALONE_RC" ] || [ "$K2_STANDALONE_RC" = 0 ]
}

# k2_standalone_macos_daemon_wait STAGE_DIR: block until it ends; 0 when it
# succeeded (or none ran), 1 (with the log's tail) when it failed.
k2_standalone_macos_daemon_wait() {
    local stage="$1" t0
    if [ -n "$K2_STANDALONE_PID" ]; then
        t0="$(date +%s)"
        _k2_standalone_reap "$stage"
        echo "  background standalone daemon build ended (waited $(( $(date +%s) - t0 ))s)."
    fi
    [ -z "$K2_STANDALONE_RC" ] || [ "$K2_STANDALONE_RC" = 0 ]
}

k2_standalone_macos_daemon_finish() {
    local app="$1" stage="$2" out="$3" t0
    t0="$(date +%s)"
    if [ -n "$K2_STANDALONE_PID" ]; then
        echo "  waiting for the background standalone daemon build..."
        _k2_standalone_reap "$stage"
        echo "  (waited $(( $(date +%s) - t0 ))s at Step 8.5)"
    fi
    [ "$K2_STANDALONE_RC" = 0 ] \
        || { echo "FATAL: no standalone daemon (background build exit: ${K2_STANDALONE_RC:-never started})" >&2; return 1; }
    grep -E '^ *(Compiling|Finished) ' "$stage/build.log" | sed 's/^ */  build: /' || true
    # What is embedded must be what ships: the stapled app's signed files.
    cmp -s "$stage/embed/k2-ptyd" "$(k2_daemon_helper_ptyd "$app")" \
        || { echo "FATAL: the k2-ptyd embedded in the standalone daemon is not the one in the shipped app" >&2; return 1; }
    cmp -s "$stage/embed/k2-export" "$(k2_daemon_helper_export "$app")" \
        || { echo "FATAL: the k2-export embedded in the standalone daemon is not the one in the shipped app" >&2; return 1; }
    _k2_standalone_embeds_signed "$app" || return 1
    (
        # shellcheck source=linux-ptyd-embed.sh
        . "$_K2_HELPER_SCRIPTS_DIR/linux-ptyd-embed.sh"
        K2_PTYD_EMBED="$(k2_daemon_helper_ptyd "$app")"
        K2_EXPORT_EMBED="$(k2_daemon_helper_export "$app")"
        assert_daemon_embeds_ptyd "$stage/k2-daemon" || exit 1
        assert_daemon_embeds_export "$stage/k2-daemon" || exit 1
        if declare -F assert_daemon_oauth_not_placeholder >/dev/null; then
            assert_daemon_oauth_not_placeholder "$stage/k2-daemon" || exit 1
        fi
    ) || return 1
    cp "$stage/k2-daemon" "$out" || return 1
    echo "  ✓ Standalone macOS daemon embeds the shipped, signed k2-ptyd and k2-export: $out"
}

k2_standalone_macos_daemon_kill() {
    [ -n "$K2_STANDALONE_PID" ] || return 0
    k2_kill_tree "$K2_STANDALONE_PID"
    wait "$K2_STANDALONE_PID" 2>/dev/null || true
    K2_STANDALONE_PID=""
}

# k2_daemon_helper_verify_shipped_ptyd APP
# k2-ptyd (prd-k2-ptyd-v1 P2): after notarization + staple, the app users
# install still carries the nested k2-ptyd intact — same identifier, a
# signature that verifies strictly inside the stapled bundle, and Gatekeeper
# accepts the bundle that contains it. Agents survive updates only if the
# daemon finds this binary beside it.
k2_daemon_helper_verify_shipped_ptyd() {
    local app="$1" ptyd pinfo
    ptyd="$(k2_daemon_helper_ptyd "$app")"
    [ -x "$ptyd" ] || { echo "FATAL: the notarized app has no k2-ptyd at $ptyd" >&2; return 1; }
    codesign --verify --strict "$ptyd" \
        || { echo "FATAL: k2-ptyd in the notarized app does not verify" >&2; return 1; }
    pinfo="$(codesign -dv "$ptyd" 2>&1)" \
        || { echo "FATAL: codesign -dv $ptyd failed: $pinfo" >&2; return 1; }
    grep -q "^Identifier=dev.k2.app.ptyd\$" <<<"$pinfo" \
        || { echo "FATAL: k2-ptyd in the notarized app is not dev.k2.app.ptyd" >&2; return 1; }
    spctl --assess --type execute "$app" \
        || { echo "FATAL: Gatekeeper rejects the notarized app (with its k2-ptyd)" >&2; return 1; }
    echo "  ✓ Notarized app ships a signed k2-ptyd (Gatekeeper accepts it)."
}

# k2_daemon_helper_verify_shipped_export APP
# The export worker, as k2_daemon_helper_verify_shipped_ptyd checks k2-ptyd:
# after notarization + staple the nested k2-export is intact, signed as
# dev.k2.app.export, and Gatekeeper accepts the bundle.
k2_daemon_helper_verify_shipped_export() {
    local app="$1" exp einfo
    exp="$(k2_daemon_helper_export "$app")"
    [ -x "$exp" ] || { echo "FATAL: the notarized app has no k2-export at $exp" >&2; return 1; }
    codesign --verify --strict "$exp" \
        || { echo "FATAL: k2-export in the notarized app does not verify" >&2; return 1; }
    einfo="$(codesign -dv "$exp" 2>&1)" \
        || { echo "FATAL: codesign -dv $exp failed: $einfo" >&2; return 1; }
    grep -q "^Identifier=dev.k2.app.export\$" <<<"$einfo" \
        || { echo "FATAL: k2-export in the notarized app is not dev.k2.app.export" >&2; return 1; }
    spctl --assess --type execute "$app" \
        || { echo "FATAL: Gatekeeper rejects the notarized app (with its k2-export)" >&2; return 1; }
    echo "  ✓ Notarized app ships a signed k2-export (Gatekeeper accepts it)."
}

# k2_daemon_helper_verify_tarball_export TARBALL
# The update bundle holds the nested k2-export beside the daemon, executable.
k2_daemon_helper_verify_tarball_export() {
    local tgz="$1" want listing
    want="K2.app/Contents/Helpers/${K2_DAEMON_HELPER_APP}/Contents/MacOS/${K2_DAEMON_HELPER_EXPORT}"
    listing="$(tar -tvzf "$tgz")" \
        || { echo "FATAL: cannot list $tgz" >&2; return 1; }
    grep -F -- "$want" <<<"$listing" | grep -q '^-rwx' \
        || { echo "FATAL: $tgz has no executable $want" >&2; return 1; }
    echo "  ✓ Update bundle carries ${want}."
}

# k2_daemon_helper_verify_tarball_ptyd TARBALL
# The update bundle (K2.app.tar.gz, what the desktop updater installs) holds
# the nested k2-ptyd at the path the daemon looks for, executable.
k2_daemon_helper_verify_tarball_ptyd() {
    local tgz="$1" want listing
    want="K2.app/Contents/Helpers/${K2_DAEMON_HELPER_APP}/Contents/MacOS/${K2_DAEMON_HELPER_PTYD}"
    listing="$(tar -tvzf "$tgz")" \
        || { echo "FATAL: cannot list $tgz" >&2; return 1; }
    grep -F -- "$want" <<<"$listing" | grep -q '^-rwx' \
        || { echo "FATAL: $tgz has no executable $want" >&2; return 1; }
    echo "  ✓ Update bundle carries ${want}."
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
    # K2 vault (VS14): the daemon holds the vault key; nothing may attach to
    # or inject into it.
    if grep -qE "get-task-allow|allow-dyld-environment-variables|cs.allow-jit|allow-unsigned-executable-memory" <<<"$ents"; then
        echo "FATAL: K2 Daemon.app carries an entitlement it must not have (debugger/dyld/JIT)" >&2
        return 1
    fi

    rc=0
    out="$("$exe" --version 2>&1)" || rc=$?
    if [ "$rc" -eq 137 ]; then
        echo "FATAL: signed k2-daemon SIGKILL'd at exec (137 = AMFI). Output: ${out:0:400}" >&2
        return 1
    fi
    [ "$rc" -eq 0 ] || { echo "FATAL: signed k2-daemon --version exited rc=$rc: ${out:0:400}" >&2; return 1; }

    # k2-ptyd: same Team ID as the daemon, hardened runtime, no
    # entitlements at all (it holds agent PTYs; nothing may attach to it),
    # and it execs past AMFI.
    local ptyd pinfo pents team pteam pout prc
    ptyd="$(k2_daemon_helper_ptyd "$app")"
    pinfo="$(codesign -dv "$ptyd" 2>&1)" \
        || { echo "FATAL: codesign -dv $ptyd failed: $pinfo" >&2; return 1; }
    grep -q "^Identifier=dev.k2.app.ptyd\$" <<<"$pinfo" \
        || { echo "FATAL: k2-ptyd code identifier is not dev.k2.app.ptyd" >&2; return 1; }
    grep -q "flags=.*runtime" <<<"$pinfo" \
        || { echo "FATAL: k2-ptyd is not signed with the hardened runtime" >&2; return 1; }
    team="$(sed -n 's/^TeamIdentifier=//p' <<<"$info")"
    pteam="$(sed -n 's/^TeamIdentifier=//p' <<<"$pinfo")"
    { [ -n "$team" ] && [ "$team" != "not set" ] && [ "$team" = "$pteam" ]; } \
        || { echo "FATAL: k2-ptyd Team ID '$pteam' differs from the daemon's '$team'" >&2; return 1; }
    pents="$(codesign -d --entitlements - "$ptyd" 2>/dev/null)" \
        || { echo "FATAL: cannot read k2-ptyd entitlements" >&2; return 1; }
    if grep -q "<key>" <<<"$pents"; then
        echo "FATAL: k2-ptyd carries entitlements; it must have none" >&2
        return 1
    fi
    prc=0
    pout="$("$ptyd" --version 2>&1)" || prc=$?
    if [ "$prc" -eq 137 ]; then
        echo "FATAL: signed k2-ptyd SIGKILL'd at exec (137 = AMFI). Output: ${pout:0:400}" >&2
        return 1
    fi
    [ "$prc" -eq 0 ] || { echo "FATAL: signed k2-ptyd --version exited rc=$prc: ${pout:0:400}" >&2; return 1; }

    # k2-export: as k2-ptyd (it parses hostile bytes; nothing may attach to
    # it, and it holds no entitlement).
    local exp einfo eents eteam eout erc
    exp="$(k2_daemon_helper_export "$app")"
    einfo="$(codesign -dv "$exp" 2>&1)" \
        || { echo "FATAL: codesign -dv $exp failed: $einfo" >&2; return 1; }
    grep -q "^Identifier=dev.k2.app.export\$" <<<"$einfo" \
        || { echo "FATAL: k2-export code identifier is not dev.k2.app.export" >&2; return 1; }
    grep -q "flags=.*runtime" <<<"$einfo" \
        || { echo "FATAL: k2-export is not signed with the hardened runtime" >&2; return 1; }
    eteam="$(sed -n 's/^TeamIdentifier=//p' <<<"$einfo")"
    [ "$team" = "$eteam" ] \
        || { echo "FATAL: k2-export Team ID '$eteam' differs from the daemon's '$team'" >&2; return 1; }
    eents="$(codesign -d --entitlements - "$exp" 2>/dev/null)" \
        || { echo "FATAL: cannot read k2-export entitlements" >&2; return 1; }
    if grep -q "<key>" <<<"$eents"; then
        echo "FATAL: k2-export carries entitlements; it must have none" >&2
        return 1
    fi
    erc=0
    eout="$("$exp" --version 2>&1)" || erc=$?
    if [ "$erc" -eq 137 ]; then
        echo "FATAL: signed k2-export SIGKILL'd at exec (137 = AMFI). Output: ${eout:0:400}" >&2
        return 1
    fi
    [ "$erc" -eq 0 ] || { echo "FATAL: signed k2-export --version exited rc=$erc: ${eout:0:400}" >&2; return 1; }
    echo "  ✓ K2 Daemon.app signed (runtime + entitlements); daemon execs past AMFI: $out; $pout; $eout (runtime, no entitlements, team $pteam)"
}
