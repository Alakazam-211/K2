#!/usr/bin/env bash
# Arch/Omarchy whole-bundle packaging lock (O1–O27).
# Fail loud on this Mac — no Omarchy, no skip-if-missing.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ARCH="$ROOT/packaging/arch"

pass=0
fail=0

pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

need_file() {
    local path="$1" label="${2:-$1}"
    if [[ -f "$path" ]]; then
        pass "exists $label"
    else
        fail "exists $label" "missing $path"
    fi
}

contains() {
    local label="$1" file="$2" needle="$3"
    if [[ ! -f "$file" ]]; then
        fail "$label" "missing $file"
        return
    fi
    if grep -Fq -- "$needle" "$file"; then
        pass "$label"
    else
        fail "$label" "missing $(printf %q "$needle") in $file"
    fi
}

absent() {
    local label="$1" file="$2" needle="$3"
    if [[ ! -f "$file" ]]; then
        fail "$label" "missing $file"
        return
    fi
    if grep -Fq -- "$needle" "$file"; then
        fail "$label" "unexpected $(printf %q "$needle") in $file"
    else
        pass "$label"
    fi
}

# Comments may document a lock ("never --features airgap"). Code lines must not.
absent_code() {
    local label="$1" file="$2" needle="$3"
    if [[ ! -f "$file" ]]; then
        fail "$label" "missing $file"
        return
    fi
    if grep -vE '^\s*(#|$)' "$file" | grep -Fq -- "$needle"; then
        fail "$label" "unexpected $(printf %q "$needle") in non-comment lines of $file"
    else
        pass "$label"
    fi
}

matches() {
    local label="$1" file="$2" regex="$3"
    if [[ ! -f "$file" ]]; then
        fail "$label" "missing $file"
        return
    fi
    if grep -Eq -- "$regex" "$file"; then
        pass "$label"
    else
        fail "$label" "no match /$regex/ in $file"
    fi
}

echo "== Arch/Omarchy packaging lock =="

need_file "$ARCH/PKGBUILD" "packaging/arch/PKGBUILD"
need_file "$ARCH/k2-daemon.service" "packaging/arch/k2-daemon.service"
need_file "$ARCH/k2.desktop" "packaging/arch/k2.desktop"
need_file "$ARCH/k2.install" "packaging/arch/k2.install"
need_file "$ARCH/README.md" "packaging/arch/README.md"

# O4 / O24 — pkgname k2 (k2-bin is a later AUR rename, not this file)
matches "PKGBUILD pkgname=k2" "$ARCH/PKGBUILD" '^pkgname=k2$'
matches "PKGBUILD license FSL-1.1-Apache-2.0" "$ARCH/PKGBUILD" "license=\\('FSL-1.1-Apache-2.0'\\)"
matches "PKGBUILD arch x86_64 first" "$ARCH/PKGBUILD" "^arch=\\('x86_64'\\)"

# pkgver tracks the live tree (this feat must not bump versions)
conf_ver="$(grep -m1 '"version"' "$ROOT/src-tauri/tauri.conf.json" | sed -E 's/.*"version": *"([^"]+)".*/\1/')"
pkg_ver="$(grep -E '^pkgver=' "$ARCH/PKGBUILD" | head -1 | cut -d= -f2)"
if [[ -n "$conf_ver" && "$pkg_ver" == "$conf_ver" ]]; then
    pass "PKGBUILD pkgver=$pkg_ver matches tauri.conf.json"
else
    fail "PKGBUILD pkgver matches tauri.conf.json" "pkgver=$pkg_ver tauri=$conf_ver"
fi

# O19 depends until ldd — gtk3 not gtk4
for dep in webkit2gtk-4.1 gtk3 libayatana-appindicator librsvg libsecret openssl; do
    contains "PKGBUILD depends $dep" "$ARCH/PKGBUILD" "'$dep'"
done
absent "PKGBUILD does not depend gtk4" "$ARCH/PKGBUILD" "gtk4"

# O14 / O26 — PATH k2 is the CLI; GUI is k2-gui
matches "PKGBUILD installs cli/k2 as /usr/bin/k2" "$ARCH/PKGBUILD" 'cli/k2" "\$\{pkgdir\}/usr/bin/k2"'
matches "PKGBUILD installs /usr/bin/k2-gui" "$ARCH/PKGBUILD" '\$\{pkgdir\}/usr/bin/k2-gui"'
matches "PKGBUILD installs /usr/lib/k2/k2-gui" "$ARCH/PKGBUILD" '\$\{pkgdir\}/usr/lib/k2/k2-gui"'
matches "desktop Exec=k2-gui" "$ARCH/k2.desktop" '^Exec=k2-gui$'
if [[ -f "$ARCH/k2.desktop" ]] && grep -Eq '^Exec=k2$' "$ARCH/k2.desktop"; then
    fail "desktop does not Exec=k2 as GUI" "Exec=k2 would steal the CLI name"
else
    pass "desktop does not Exec=k2 as GUI"
fi
matches "desktop is an Application" "$ARCH/k2.desktop" '^Type=Application$'

# O15 — user unit, exact Debian text, not k2so-daemon, not a system unit
contains "unit ExecStart=/usr/bin/k2-daemon" "$ARCH/k2-daemon.service" "ExecStart=/usr/bin/k2-daemon"
contains "unit WantedBy=default.target" "$ARCH/k2-daemon.service" "WantedBy=default.target"
contains "unit Restart=always" "$ARCH/k2-daemon.service" "Restart=always"
contains "PKGBUILD installs user unit path" "$ARCH/PKGBUILD" '/usr/lib/systemd/user/k2-daemon.service'
absent "unit is not k2so-daemon" "$ARCH/k2-daemon.service" "k2so-daemon"
absent "PKGBUILD does not install a system unit" "$ARCH/PKGBUILD" "/usr/lib/systemd/system"
absent "PKGBUILD does not mention k2so-daemon as a unit" "$ARCH/PKGBUILD" "k2so-daemon.service"

if [[ -f "$ARCH/k2-daemon.service" && -f "$ROOT/scripts/package-daemon-deb.sh" ]]; then
    deb_unit="$(awk '
        /^cat > .* <<'\''UNIT'\''/ {p=1; next}
        /^UNIT$/ {p=0}
        p {print}
    ' "$ROOT/scripts/package-daemon-deb.sh")"
    arch_unit="$(cat "$ARCH/k2-daemon.service")"
    if [[ "$deb_unit" == "$arch_unit" ]]; then
        pass "unit text matches scripts/package-daemon-deb.sh"
    else
        fail "unit text matches scripts/package-daemon-deb.sh" "packaging/arch/k2-daemon.service diverged from the Debian UNIT heredoc"
    fi
fi

# O12 — not the air-gap SKU
absent_code "PKGBUILD does not enable airgap" "$ARCH/PKGBUILD" "--features airgap"
contains "PKGBUILD comments airgap out" "$ARCH/PKGBUILD" "airgap"
contains "README air-gap SKU is not this package" "$ARCH/README.md" "Air-gap"

# O17 — no debtap
absent_code "PKGBUILD does not debtap" "$ARCH/PKGBUILD" "debtap"
contains "README does not debtap GH deb" "$ARCH/README.md" "debtap"

# O9/O27 — release.sh hands-off
absent_code "PKGBUILD does not call release.sh" "$ARCH/PKGBUILD" "release.sh"
contains "README release.sh hands-off until O9" "$ARCH/README.md" "release.sh"

# O18 — pacman owns bins; no GH swap / k2 update overwrite
contains "README upgrade via pacman/AUR" "$ARCH/README.md" "k2 update"
contains ".install warns against k2 update" "$ARCH/k2.install" "k2 update"

# O22 — packaged install is the install; pacman -R does not wipe ~/.k2
contains ".install says do not run k2 daemon install" "$ARCH/k2.install" "k2 daemon install"
absent ".install does not rm ~/.k2" "$ARCH/k2.install" "rm -rf"
contains ".install leaves ~/.k2 on remove" "$ARCH/k2.install" "~/.k2"
contains "README pacman -R leaves ~/.k2" "$ARCH/README.md" "pacman -R"

# O25 — OAuth bake, no secrets in git
contains "PKGBUILD sources require-mail-oauth-build-env.sh" "$ARCH/PKGBUILD" "require-mail-oauth-build-env.sh"
contains "PKGBUILD comments k2-bin pre-baked daemon" "$ARCH/PKGBUILD" "k2-bin"
if grep -Eiq 'K2_GMAIL_CLIENT_(ID|SECRET)=[A-Za-z0-9._-]{8,}' "$ARCH/PKGBUILD" "$ARCH/README.md" "$ARCH/k2.install" "$ARCH/k2.desktop" "$ARCH/k2-daemon.service"; then
    fail "no Gmail OAuth secrets in packaging files" "looks like a real client id/secret assignment"
else
    pass "no Gmail OAuth secrets in packaging files"
fi
if grep -Fq 'REPLACE_ME.apps.googleusercontent.com' "$ARCH/PKGBUILD"; then
    fail "PKGBUILD does not embed REPLACE_ME client" "placeholder client id in PKGBUILD"
else
    pass "PKGBUILD does not embed REPLACE_ME client"
fi

# O16 — smoke must not require k2 daemon status
contains "README does not smoke k2 daemon status" "$ARCH/README.md" "k2 daemon status"

# O1 — whole bundle
contains "README whole bundle GUI" "$ARCH/README.md" "GUI"
contains "PKGBUILD installs k2-daemon" "$ARCH/PKGBUILD" 'k2-daemon'

# O23 — no WEBKIT env hacks
absent "PKGBUILD no WEBKIT_ env" "$ARCH/PKGBUILD" "WEBKIT_"
absent "desktop no WEBKIT_ env" "$ARCH/k2.desktop" "WEBKIT_"

echo
if [[ "$fail" -ne 0 ]]; then
    echo "FAIL: $fail  PASS: $pass" >&2
    exit 1
fi
echo "PASS: $pass"
exit 0
