#!/usr/bin/env bash
# k2-signer-install.sh — install-k2-signer.sh in test mode (macOS, no root, no
# `_k2sign`, no keys), then the installed wrappers end to end: env -i, the
# binary next to them, pins, a policy approved by sha, refusals with exit 3,
# usage with exit 2 (prd-ci-on-compute-for-clients-v1 C4).
#   cargo build --locked -p k2-signer && bash tests/release/k2-signer-install.sh target/debug/k2-signer
set -uo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:?usage: k2-signer-install.sh <path to the built k2-signer>}"
case "$BIN" in /*) ;; *) BIN="$PWD/$BIN" ;; esac
[ "$(uname -s)" = Darwin ] || { echo "SKIP-REFUSED: macOS only (the installer is macOS's)" >&2; exit 1; }
[ "$(id -u)" != 0 ] || { echo "FAIL: run as a normal user (test mode is refused as root)" >&2; exit 1; }
tmp="$(mktemp -d "${TMPDIR:-/tmp}/k2-signer-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
FAILS=0
pass() { printf 'ok   %s\n' "$1"; }
fail() { printf 'FAIL %s\n%s\n' "$1" "$2"; FAILS=$((FAILS + 1)); }
export K2_SIGNER_TEST_ROOT="$tmp/root"
mkdir -p "$K2_SIGNER_TEST_ROOT/var/k2sign"
chmod 700 "$K2_SIGNER_TEST_ROOT/var/k2sign"
L="$K2_SIGNER_TEST_ROOT/usr/local/libexec/k2-signer"
S="$L/k2-signer"
A="$L/k2-signer-admin"

check() { # <name> <want rc> <want text> <stdin file or -> <cmd…>
    local name="$1" want="$2" text="$3" in="$4" rc=0
    shift 4
    if [ "$in" = - ]; then "$@" </dev/null >"$tmp/out" 2>"$tmp/err" || rc=$?; else "$@" <"$in" >"$tmp/out" 2>"$tmp/err" || rc=$?; fi
    if [ "$rc" = "$want" ] && grep -qF -- "$text" "$tmp/out" "$tmp/err"; then pass "$name"; else fail "$name" "rc=$rc $(cat "$tmp/out" "$tmp/err" | head -5)"; fi
}

I="$REPO/scripts/k2sign/install-k2-signer.sh"
SHA="$(shasum -a 256 "$BIN" | awk '{print $1}')"
echo "== install (test mode) =="
check "the installer refuses a missing binary" 1 "--binary" - bash "$I" --test --binary "$tmp/nope" --expect-sha256 "$SHA"
check "M6: no independent build's sha256: refused" 1 "--expect-sha256" - bash "$I" --test --binary "$BIN"
check "M6: another build's sha256: refused" 1 "not the independent build's" - bash "$I" --test --binary "$BIN" --expect-sha256 "$(printf '0%.0s' $(seq 64))"
mkdir -p "$K2_SIGNER_TEST_ROOT/usr/local"
chmod 777 "$K2_SIGNER_TEST_ROOT/usr/local"
check "L10: a world-writable folder above the signer's: refused" 1 "group/world-writable" - bash "$I" --test --binary "$BIN" --expect-sha256 "$SHA"
chmod 755 "$K2_SIGNER_TEST_ROOT/usr/local"
check "a dry run changes nothing" 0 "dry run" - bash "$I" --test --dry-run --binary "$BIN" --expect-sha256 "$SHA"
[ ! -e "$L/k2-signer-bin" ] && pass "…nothing installed by the dry run" || fail "dry run installed" ""
check "install" 0 "installed. Hashes" - bash "$I" --test --binary "$BIN" --expect-sha256 "$SHA"
for f in k2-signer k2-signer-admin k2-signer-bin; do
    grep -q "  libexec/$f\$" "$K2_SIGNER_TEST_ROOT/usr/local/etc/k2-signer/pins.sha256" && pass "pins.sha256 pins $f" || fail "pin $f" "$(cat "$K2_SIGNER_TEST_ROOT/usr/local/etc/k2-signer/pins.sha256")"
done
grep -q "NOPASSWD: /usr/local/libexec/k2-signer/k2-signer\$" "$K2_SIGNER_TEST_ROOT/etc/sudoers.d/k2-signer" && pass "the sudoers rule names the one wrapper" || fail "sudoers" ""
[ "$(stat -f '%Lp' "$K2_SIGNER_TEST_ROOT/var/k2sign/k2-signer")" = 700 ] && pass "/var/k2sign/k2-signer is 0700" || fail "var mode" ""

echo "== the wrappers =="
# M5: the wrappers refuse a test root as the signing user before anything
# under it runs (here a fake binary that would leave a mark).
FAKE="$tmp/fake"
mkdir -p "$FAKE/usr/local/libexec/k2-signer" "$FAKE/usr/local/libexec/k2sign"
printf '#!/bin/sh\ntouch "%s/ran"\n' "$tmp" > "$FAKE/usr/local/libexec/k2-signer/k2-signer-bin"
chmod 755 "$FAKE/usr/local/libexec/k2-signer/k2-signer-bin"
printf 'open("%s/ran", "w"); print("fake-ran")\n' "$tmp" > "$FAKE/usr/local/libexec/k2sign/k2sign.py"
sed "s#@PYTHON@#/usr/bin/python3#" "$REPO/scripts/k2sign/k2sign" > "$tmp/k2sign-wrapper"
for w in "$REPO/scripts/k2sign/k2-signer" "$REPO/scripts/k2sign/k2-signer-admin"; do
    rm -f "$tmp/ran"
    K2_SIGNER_TEST_ROOT="$FAKE" K2_SIGNER_TEST_SIGNER_USER="$(id -un)" check "M5: $(basename "$w") refuses a test root as the signing user" 3 "is refused as root or the signing user" - bash "$w" pins
    [ ! -e "$tmp/ran" ] && pass "…and ran nothing under it" || fail "$(basename "$w") ran the test root's binary" ""
done
rm -f "$tmp/ran"
K2SIGN_TEST_ROOT="$FAKE" K2SIGN_TEST_SIGNER_USER="$(id -un)" check "M5: k2sign's wrapper refuses the same way" 3 "test mode (K2SIGN_TEST_ROOT) is refused" - bash "$tmp/k2sign-wrapper" status 0.46.6
[ ! -e "$tmp/ran" ] && pass "…and ran no k2sign.py under it" || fail "k2sign wrapper ran the test root's python" ""
K2SIGN_TEST_ROOT="$FAKE" check "(control) the k2sign wrapper runs the test root's file otherwise" 0 "fake-ran" - bash "$tmp/k2sign-wrapper" status 0.46.6
[ -e "$tmp/ran" ] && pass "…it did" || fail "the control did not run" ""
check "pins: nothing trusted yet" 0 '"controllers":[]' - "$S" pins
check "an unknown verb is usage (exit 2)" 2 "verbs:" - "$S" nope
check "an admin verb through the signer wrapper is usage" 2 "verbs:" - "$S" trust-owner
python3 -I - "$REPO/tests/release/fixtures/ci-formats-v1.json" "$tmp/bundle.json" <<'PY'
import json, sys
json.dump(json.load(open(sys.argv[1]))["evidence_bundle"], open(sys.argv[2], "w"))
PY
POL="$REPO/tests/release/fixtures/ci-formats-v1-signer-policy.toml"
check "rehearse before any policy: refused (exit 3)" 3 "no policy for project acme-desktop" "$tmp/bundle.json" "$S" rehearse acme-desktop
check "policy-approve shows the diff and its sha256" 0 "policy-approve acme-desktop $(shasum -a 256 "$POL" | awk '{print $1}')" "$POL" "$A" policy-approve acme-desktop
check "policy-approve with another sha: refused" 3 "not installed" "$POL" "$A" policy-approve acme-desktop "$(printf '0%.0s' $(seq 64))"
check "policy-approve with its sha installs it" 0 "installed" "$POL" "$A" policy-approve acme-desktop "$(shasum -a 256 "$POL" | awk '{print $1}')"
check "pins lists the project" 0 '"projects":["acme-desktop"]' - "$S" pins
check "the golden bundle: refused until the controller is paired" 3 "is not paired with this signer" "$tmp/bundle.json" "$S" rehearse acme-desktop
echo 'version = 1' >> "$K2_SIGNER_TEST_ROOT/usr/local/etc/k2-signer/projects/acme-desktop.toml"
check "a policy edited without the admin's repin: refused" 3 "pins mismatch: etc/projects/acme-desktop.toml" "$tmp/bundle.json" "$S" rehearse acme-desktop
check "root's repin pins the edit (admin only)" 0 "etc/projects/acme-desktop.toml" - "$A" repin
check "pair-begin prints the code for k2 ci signer add" 0 "k2 ci signer add" - "$A" pair-begin z3mbpZ
check "identity: the pairing key" 0 '"fp":"' - "$S" identity

echo "== the Approve sheet's text (M2) =="
if /usr/bin/swiftc -O "$REPO/scripts/k2sign/approve/main.swift" -o "$tmp/approve" 2>"$tmp/swift.err"; then
    BASE="0123456789abcdef0123456789abcdef01234567"
    printf '{"v":1,"kind":"k2-signer-approval","project":"acme-desktop","version":"1.2.3","base_commit":"%s","assets":["acme","acme.AppImage"],"reapproval":false,"max_signatures":8,"expires":"2027-01-01T00:00:00Z","nonce":"00000000000000000000000000000000","issued":"2027-01-01T00:00:00Z"}' "$BASE" > "$tmp/c.json"
    check "M2: the sheet shows the project, the whole base commit and every file" 0 "approve project acme-desktop release 1.2.3 at base commit $BASE: sign acme, acme.AppImage until" - "$tmp/approve" --describe "$tmp/c.json"
    sed "s/$BASE/0123456/" "$tmp/c.json" > "$tmp/c2.json"
    check "M2: a challenge without a whole base commit is not one" 2 "not a K2 release approval" - "$tmp/approve" --describe "$tmp/c2.json"
else
    fail "swiftc could not build K2 Release Approve" "$(head -5 "$tmp/swift.err")"
fi
echo
if [ "$FAILS" = 0 ]; then echo "k2-signer-install: all passed"; else echo "k2-signer-install: $FAILS FAILED"; exit 1; fi
