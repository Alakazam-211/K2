#!/bin/bash
# Dry test for release.sh's k2-ptyd checks (prd-k2-ptyd-v1 P2/P59):
# k2_daemon_helper_verify_tarball_ptyd passes on an update bundle that
# carries an executable nested k2-ptyd and fails loudly on one without it
# (or with it not executable), and release.sh calls both checks. Runs
# anywhere (codesign and spctl are stubs; no notarytool needed).
set -euo pipefail
here="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../../scripts/macos-daemon-helper-app.sh
. "$here/scripts/macos-daemon-helper-app.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mac="$work/ok/K2.app/Contents/Helpers/$K2_DAEMON_HELPER_APP/Contents/MacOS"
mkdir -p "$mac"
printf 'x' > "$mac/k2-daemon"; printf 'x' > "$mac/$K2_DAEMON_HELPER_PTYD"
chmod 0755 "$mac/k2-daemon" "$mac/$K2_DAEMON_HELPER_PTYD"
tar -czf "$work/ok.tar.gz" -C "$work/ok" K2.app
k2_daemon_helper_verify_tarball_ptyd "$work/ok.tar.gz" >/dev/null || { echo "FAIL: a good bundle was refused"; exit 1; }

chmod 0644 "$mac/$K2_DAEMON_HELPER_PTYD"
tar -czf "$work/noexec.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_ptyd "$work/noexec.tar.gz" 2>/dev/null; then echo "FAIL: a non-executable k2-ptyd passed"; exit 1; fi

rm "$mac/$K2_DAEMON_HELPER_PTYD"
tar -czf "$work/missing.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_ptyd "$work/missing.tar.gz" 2>/dev/null; then echo "FAIL: a bundle without k2-ptyd passed"; exit 1; fi

# The export worker (vs-live §3.1): the same check on the update bundle.
printf 'x' > "$mac/$K2_DAEMON_HELPER_EXPORT"; chmod 0755 "$mac/$K2_DAEMON_HELPER_EXPORT"
tar -czf "$work/export.tar.gz" -C "$work/ok" K2.app
k2_daemon_helper_verify_tarball_export "$work/export.tar.gz" >/dev/null || { echo "FAIL: a bundle with k2-export was refused"; exit 1; }
chmod 0644 "$mac/$K2_DAEMON_HELPER_EXPORT"
tar -czf "$work/export-noexec.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_export "$work/export-noexec.tar.gz" 2>/dev/null; then echo "FAIL: a non-executable k2-export passed"; exit 1; fi
rm "$mac/$K2_DAEMON_HELPER_EXPORT"
tar -czf "$work/export-missing.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_export "$work/export-missing.tar.gz" 2>/dev/null; then echo "FAIL: a bundle without k2-export passed"; exit 1; fi

# The .pptx slide renderer: the same check on the update bundle.
printf 'x' > "$mac/$K2_DAEMON_HELPER_DECKRENDER"; chmod 0755 "$mac/$K2_DAEMON_HELPER_DECKRENDER"
tar -czf "$work/deckrender.tar.gz" -C "$work/ok" K2.app
k2_daemon_helper_verify_tarball_deckrender "$work/deckrender.tar.gz" >/dev/null || { echo "FAIL: a bundle with k2-deckrender was refused"; exit 1; }
chmod 0644 "$mac/$K2_DAEMON_HELPER_DECKRENDER"
tar -czf "$work/deckrender-noexec.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_deckrender "$work/deckrender-noexec.tar.gz" 2>/dev/null; then echo "FAIL: a non-executable k2-deckrender passed"; exit 1; fi
rm "$mac/$K2_DAEMON_HELPER_DECKRENDER"
tar -czf "$work/deckrender-missing.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_deckrender "$work/deckrender-missing.tar.gz" 2>/dev/null; then echo "FAIL: a bundle without k2-deckrender passed"; exit 1; fi

# The 3D stills renderer: the same check on the update bundle.
printf 'x' > "$mac/$K2_DAEMON_HELPER_SCENERENDER"; chmod 0755 "$mac/$K2_DAEMON_HELPER_SCENERENDER"
tar -czf "$work/scenerender.tar.gz" -C "$work/ok" K2.app
k2_daemon_helper_verify_tarball_scenerender "$work/scenerender.tar.gz" >/dev/null || { echo "FAIL: a bundle with k2-scenerender was refused"; exit 1; }
chmod 0644 "$mac/$K2_DAEMON_HELPER_SCENERENDER"
tar -czf "$work/scenerender-noexec.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_scenerender "$work/scenerender-noexec.tar.gz" 2>/dev/null; then echo "FAIL: a non-executable k2-scenerender passed"; exit 1; fi
rm "$mac/$K2_DAEMON_HELPER_SCENERENDER"
tar -czf "$work/scenerender-missing.tar.gz" -C "$work/ok" K2.app
if k2_daemon_helper_verify_tarball_scenerender "$work/scenerender-missing.tar.gz" 2>/dev/null; then echo "FAIL: a bundle without k2-scenerender passed"; exit 1; fi

# The k5 command's program, with a stub codesign: the same signature
# check (k2_k5_cli_check_signature) after Step 3 (k2_k5_cli_verify_signed),
# after notarization (k2_k5_cli_verify_shipped) and in the update bundle
# (k2_k5_cli_verify_tarball): its own identifier, never the daemon's, a
# strict verify, the hardened runtime, the daemon's Team ID and no
# entitlements. verify_signed also runs its --version (cli/k2's), and the
# bundle must carry it at Contents/MacOS/k5-cli, executable.
(
    k5w="$work/k5sig"; mkdir -p "$k5w/stub" "$k5w/cli" "$k5w/K2.app/Contents/MacOS" "$k5w/K2.app/Contents/Helpers/$K2_DAEMON_HELPER_APP/Contents/MacOS"
    printf '#!/bin/bash\nK2_CLI_VERSION="9.8.7"\n' > "$k5w/cli/k2"
    printf '#!/bin/sh\necho "k2 ${STUB_K5_SAYS:-9.8.7}"\n' > "$k5w/K2.app/Contents/MacOS/k5-cli"
    printf 'x' > "$k5w/K2.app/Contents/Helpers/$K2_DAEMON_HELPER_APP/Contents/MacOS/k2-daemon"
    chmod 0755 "$k5w/K2.app/Contents/MacOS/k5-cli"
    # It logs each k5-cli path it is asked about, so a test can see where
    # the tarball check looked.
    cat > "$k5w/stub/codesign" <<'EOF'
#!/bin/bash
for a in "$@"; do last="$a"; done
case "$last" in *k5-cli) [ -z "${STUB_K5_LOG:-}" ] || printf '%s %s\n' "$1" "$last" >>"$STUB_K5_LOG" ;; esac
case "$1" in
    --verify) case "$last" in
                  *k5-cli) [ -z "${STUB_K5_UNSIGNED:-}" ] || { echo "$last: code object is not signed at all" >&2; exit 1; } ;;
              esac; exit 0 ;;
    -dv) case "$2" in
             *k5-cli) [ -z "${STUB_K5_UNSIGNED:-}" ] || { echo "$2: code object is not signed at all" >&2; exit 1; }
                      echo "Identifier=${STUB_K5_ID:-dev.k2.app.k5-cli}" >&2
                      echo "CodeDirectory v=20500 size=1 flags=${STUB_K5_FLAGS:-0x10000(runtime)} hashes=1" >&2
                      echo "TeamIdentifier=${STUB_K5_TEAM:-TEAM}" >&2 ;;
             *) echo "Identifier=dev.k2.app.daemon" >&2; echo "TeamIdentifier=TEAM" >&2 ;;
         esac; exit 0 ;;
    -d) [ -z "${STUB_K5_ENTS:-}" ] || printf '%s\n' "$STUB_K5_ENTS"; exit 0 ;;
esac
exit 1
EOF
    printf '#!/bin/sh\nexit 0\n' > "$k5w/stub/spctl"
    chmod +x "$k5w/stub/codesign" "$k5w/stub/spctl"
    cd "$k5w"
    export PATH="$k5w/stub:$PATH"
    tar -czf "$work/k5.tar.gz" -C "$k5w" K2.app
    k2_k5_cli_verify_signed "$k5w/K2.app" >/dev/null || { echo "FAIL: a well-signed k5-cli was refused"; exit 1; }
    k2_k5_cli_verify_shipped "$k5w/K2.app" >/dev/null || { echo "FAIL: a well-signed notarized k5-cli was refused"; exit 1; }
    export STUB_K5_LOG="$work/k5-codesign.log"
    tmp_before="$(ls -A "${TMPDIR:-/tmp}" | grep -c '^k2-k5-tarball\.' || true)"
    k2_k5_cli_verify_tarball "$work/k5.tar.gz" >/dev/null || { echo "FAIL: a bundle with a well-signed k5-cli was refused"; exit 1; }
    # It checked the k5-cli it unpacked, not the one in the tree it was
    # packed from, and left no folder behind.
    grep -q -- "^--verify .*/k2-k5-tarball\.[^/]*/K2\.app/Contents/MacOS/k5-cli\$" "$STUB_K5_LOG" \
        || { echo "FAIL: the tarball check did not verify the unpacked k5-cli: $(cat "$STUB_K5_LOG")"; exit 1; }
    if grep -qF -- "$k5w/K2.app" "$STUB_K5_LOG"; then echo "FAIL: the tarball check looked at the packed tree, not the bundle"; exit 1; fi
    tmp_after="$(ls -A "${TMPDIR:-/tmp}" | grep -c '^k2-k5-tarball\.' || true)"
    [ "$tmp_after" = "$tmp_before" ] || { echo "FAIL: the tarball check left its unpack folder behind"; exit 1; }
    unset STUB_K5_LOG
    for bad in "STUB_K5_ID=dev.k2.app.daemon" "STUB_K5_ID=k5-cli" "STUB_K5_UNSIGNED=1" "STUB_K5_FLAGS=0x0(none)" "STUB_K5_TEAM=OTHER" \
               "STUB_K5_ENTS=<dict><key>keychain-access-groups</key></dict>"; do
        for step in k2_k5_cli_verify_signed k2_k5_cli_verify_shipped; do
            if env "$bad" bash -c '. "$1/scripts/macos-daemon-helper-app.sh"; '"$step"' "$2"' _ "$here" "$k5w/K2.app" >/dev/null 2>&1; then
                echo "FAIL: $step passed with $bad"; exit 1
            fi
        done
        if env "$bad" bash -c '. "$1/scripts/macos-daemon-helper-app.sh"; k2_k5_cli_verify_tarball "$2"' _ "$here" "$work/k5.tar.gz" >/dev/null 2>&1; then
            echo "FAIL: k2_k5_cli_verify_tarball passed with $bad"; exit 1
        fi
    done
    tmp_after="$(ls -A "${TMPDIR:-/tmp}" | grep -c '^k2-k5-tarball\.' || true)"
    [ "$tmp_after" = "$tmp_before" ] || { echo "FAIL: a refused tarball check left its unpack folder behind"; exit 1; }
    if STUB_K5_SAYS=1.2.3 k2_k5_cli_verify_signed "$k5w/K2.app" >/dev/null 2>&1; then
        echo "FAIL: k2_k5_cli_verify_signed passed with STUB_K5_SAYS=1.2.3"; exit 1
    fi
    k5="$k5w/K2.app/Contents/MacOS/$K2_K5_CLI_EXE"
    chmod 0644 "$k5"
    tar -czf "$work/k5-noexec.tar.gz" -C "$k5w" K2.app
    if k2_k5_cli_verify_tarball "$work/k5-noexec.tar.gz" 2>/dev/null; then echo "FAIL: a non-executable k5-cli passed"; exit 1; fi
    rm "$k5"
    tar -czf "$work/k5-missing.tar.gz" -C "$k5w" K2.app
    if k2_k5_cli_verify_tarball "$work/k5-missing.tar.gz" 2>/dev/null; then echo "FAIL: a bundle without k5-cli passed"; exit 1; fi
)
grep -q '^k2_k5_cli_verify_shipped "K2.app" || exit 1$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the notarized app's k5-cli"; exit 1; }
grep -q '^k2_k5_cli_verify_tarball "target/release/bundle/macos/K2.app.tar.gz" || exit 1$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the update bundle's k5-cli"; exit 1; }

# ── The standalone macOS daemon embeds the signed k2-ptyd (P84) ──
# Stubs on PATH: `codesign` accepts the nested k2-ptyd as dev.k2.app.ptyd
# (or not, with STUB_BAD_SIG=1); `cargo` plays build.rs: it writes a daemon
# that carries the sha256 of $K2_PTYD_EMBED, but only when
# K2_REQUIRE_PTYD_EMBED=1 (or one without it, with STUB_NO_EMBED=1).
stub="$work/stub"; mkdir -p "$stub" "$work/proj/scripts" "$work/proj/target/release"
cp "$here/scripts/linux-ptyd-embed.sh" "$work/proj/scripts/"
cat > "$stub/codesign" <<'EOF'
#!/bin/bash
[ "${STUB_BAD_SIG:-0}" = 1 ] && { echo "invalid signature" >&2; exit 1; }
case "$1" in
    --verify) exit 0 ;;
    -dv) case "$2" in *k2-export) echo "Identifier=dev.k2.app.export" >&2 ;; *) echo "Identifier=dev.k2.app.ptyd" >&2 ;; esac; echo "TeamIdentifier=TEAM" >&2; exit 0 ;;
esac
exit 1
EOF
cat > "$stub/cargo" <<'EOF'
#!/bin/bash
out="${CARGO_TARGET_DIR:-target}/release/k2-daemon"
[ "${K2_REQUIRE_PTYD_EMBED:-}" = 1 ] && [ -f "${K2_PTYD_EMBED:-/nonexistent}" ] || { echo "stub cargo: no embed requested" >&2; exit 1; }
[ "${K2_REQUIRE_EXPORT_EMBED:-}" = 1 ] && [ -f "${K2_EXPORT_EMBED:-/nonexistent}" ] || { echo "stub cargo: no export embed requested" >&2; exit 1; }
printf 'daemon-bytes ' > "$out"
[ "${STUB_NO_EMBED:-0}" = 1 ] || shasum -a 256 "$K2_PTYD_EMBED" | cut -d' ' -f1 >> "$out"
[ "${STUB_NO_EXPORT_EMBED:-0}" = 1 ] || shasum -a 256 "$K2_EXPORT_EMBED" | cut -d' ' -f1 >> "$out"
chmod +x "$out"
EOF
chmod +x "$stub/codesign" "$stub/cargo"
app="$work/app/K2.app"
pdir="$app/Contents/Helpers/$K2_DAEMON_HELPER_APP/Contents/MacOS"
mkdir -p "$pdir"
head -c 8192 /dev/urandom > "$pdir/$K2_DAEMON_HELPER_PTYD"; chmod 0755 "$pdir/$K2_DAEMON_HELPER_PTYD"
sha="$(shasum -a 256 "$pdir/$K2_DAEMON_HELPER_PTYD" | cut -d' ' -f1)"
head -c 8192 /dev/urandom > "$pdir/$K2_DAEMON_HELPER_EXPORT"; chmod 0755 "$pdir/$K2_DAEMON_HELPER_EXPORT"
esha="$(shasum -a 256 "$pdir/$K2_DAEMON_HELPER_EXPORT" | cut -d' ' -f1)"
CARGO_TARGET_DIR="$work/proj/target" PATH="$stub:$PATH" k2_build_standalone_macos_daemon "$app" "$work/standalone" "$work/proj" >/dev/null \
    || { echo "FAIL: the standalone build with a signed k2-ptyd was refused"; exit 1; }
grep -q "$sha" "$work/standalone" || { echo "FAIL: the standalone daemon does not carry the signed k2-ptyd's sha256"; exit 1; }
grep -q "$esha" "$work/standalone" || { echo "FAIL: the standalone daemon does not carry the signed k2-export's sha256"; exit 1; }
rm -f "$work/standalone"
if STUB_NO_EMBED=1 CARGO_TARGET_DIR="$work/proj/target" PATH="$stub:$PATH" k2_build_standalone_macos_daemon "$app" "$work/standalone" "$work/proj" >/dev/null 2>&1; then
    echo "FAIL: a standalone daemon without the embed passed"; exit 1
fi
[ ! -e "$work/standalone" ] || { echo "FAIL: a daemon without the embed was published"; exit 1; }
if STUB_NO_EXPORT_EMBED=1 CARGO_TARGET_DIR="$work/proj/target" PATH="$stub:$PATH" k2_build_standalone_macos_daemon "$app" "$work/standalone" "$work/proj" >/dev/null 2>&1; then
    echo "FAIL: a standalone daemon without the k2-export embed passed"; exit 1
fi
[ ! -e "$work/standalone" ] || { echo "FAIL: a daemon without the k2-export embed was published"; exit 1; }
if STUB_BAD_SIG=1 CARGO_TARGET_DIR="$work/proj/target" PATH="$stub:$PATH" k2_build_standalone_macos_daemon "$app" "$work/standalone" "$work/proj" >/dev/null 2>&1; then
    echo "FAIL: an unsigned k2-ptyd was embedded"; exit 1
fi
grep -q '^k2_build_standalone_macos_daemon "$PROJECT_DIR/target/release/bundle/macos/K2.app" "$DIST_DIR/$MAC_ASSET" "$PROJECT_DIR"$' "$here/scripts/release.sh" \
    || { echo "FAIL: release.sh doesn't build the standalone daemon with the embedded k2-ptyd"; exit 1; }
if grep -q 'cp "$PROJECT_DIR/target/release/k2-daemon" "$DIST_DIR/$MAC_ASSET"' "$here/scripts/release.sh"; then
    echo "FAIL: release.sh still publishes the plain daemon as the standalone asset"; exit 1
fi
# Order: the embed uses the k2-ptyd Step 3 signed, so it comes after it.
sign_line=$(grep -n '^k2_daemon_helper_sign ' "$here/scripts/release.sh" | head -1 | cut -d: -f1)
embed_line=$(grep -n '^k2_build_standalone_macos_daemon ' "$here/scripts/release.sh" | head -1 | cut -d: -f1)
[ -n "$sign_line" ] && [ -n "$embed_line" ] && [ "$embed_line" -gt "$sign_line" ] \
    || { echo "FAIL: release.sh embeds k2-ptyd before Step 3 signs it"; exit 1; }

grep -q '^k2_daemon_helper_verify_shipped_ptyd "K2.app"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the notarized app's k2-ptyd"; exit 1; }
grep -q '^k2_daemon_helper_verify_tarball_ptyd "target/release/bundle/macos/K2.app.tar.gz"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the update bundle's k2-ptyd"; exit 1; }
grep -q '^k2_daemon_helper_verify_shipped_export "K2.app"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the notarized app's k2-export"; exit 1; }
grep -q '^k2_daemon_helper_verify_tarball_export "target/release/bundle/macos/K2.app.tar.gz"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the update bundle's k2-export"; exit 1; }
grep -q '^k2_daemon_helper_verify_shipped_deckrender "K2.app"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the notarized app's k2-deckrender"; exit 1; }
grep -q '^k2_daemon_helper_verify_tarball_deckrender "target/release/bundle/macos/K2.app.tar.gz"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the update bundle's k2-deckrender"; exit 1; }
grep -q '^k2_daemon_helper_verify_shipped_scenerender "K2.app"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the notarized app's k2-scenerender"; exit 1; }
grep -q '^k2_daemon_helper_verify_tarball_scenerender "target/release/bundle/macos/K2.app.tar.gz"$' "$here/scripts/release.sh" || { echo "FAIL: release.sh doesn't check the update bundle's k2-scenerender"; exit 1; }
echo "ok: release k2-ptyd, k2-export, k2-deckrender, k2-scenerender and k5-cli checks"
