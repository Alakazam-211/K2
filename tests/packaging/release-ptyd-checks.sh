#!/bin/bash
# Dry test for release.sh's k2-ptyd checks (prd-k2-ptyd-v1 P2/P59):
# k2_daemon_helper_verify_tarball_ptyd passes on an update bundle that
# carries an executable nested k2-ptyd and fails loudly on one without it
# (or with it not executable), and release.sh calls both checks. Runs
# anywhere (no codesign/notarytool needed).
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
    -dv) echo "Identifier=dev.k2.app.ptyd" >&2; echo "TeamIdentifier=TEAM" >&2; exit 0 ;;
esac
exit 1
EOF
cat > "$stub/cargo" <<'EOF'
#!/bin/bash
out="${CARGO_TARGET_DIR:-target}/release/k2-daemon"
[ "${K2_REQUIRE_PTYD_EMBED:-}" = 1 ] && [ -f "${K2_PTYD_EMBED:-/nonexistent}" ] || { echo "stub cargo: no embed requested" >&2; exit 1; }
printf 'daemon-bytes ' > "$out"
[ "${STUB_NO_EMBED:-0}" = 1 ] || shasum -a 256 "$K2_PTYD_EMBED" | cut -d' ' -f1 >> "$out"
chmod +x "$out"
EOF
chmod +x "$stub/codesign" "$stub/cargo"
app="$work/app/K2.app"
pdir="$app/Contents/Helpers/$K2_DAEMON_HELPER_APP/Contents/MacOS"
mkdir -p "$pdir"
head -c 8192 /dev/urandom > "$pdir/$K2_DAEMON_HELPER_PTYD"; chmod 0755 "$pdir/$K2_DAEMON_HELPER_PTYD"
sha="$(shasum -a 256 "$pdir/$K2_DAEMON_HELPER_PTYD" | cut -d' ' -f1)"
PATH="$stub:$PATH" k2_build_standalone_macos_daemon "$app" "$work/standalone" "$work/proj" >/dev/null \
    || { echo "FAIL: the standalone build with a signed k2-ptyd was refused"; exit 1; }
grep -q "$sha" "$work/standalone" || { echo "FAIL: the standalone daemon does not carry the signed k2-ptyd's sha256"; exit 1; }
rm -f "$work/standalone"
if STUB_NO_EMBED=1 PATH="$stub:$PATH" k2_build_standalone_macos_daemon "$app" "$work/standalone" "$work/proj" >/dev/null 2>&1; then
    echo "FAIL: a standalone daemon without the embed passed"; exit 1
fi
[ ! -e "$work/standalone" ] || { echo "FAIL: a daemon without the embed was published"; exit 1; }
if STUB_BAD_SIG=1 PATH="$stub:$PATH" k2_build_standalone_macos_daemon "$app" "$work/standalone" "$work/proj" >/dev/null 2>&1; then
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
echo "ok: release k2-ptyd checks"
