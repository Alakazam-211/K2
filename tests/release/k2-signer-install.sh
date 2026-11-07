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

echo "== install (test mode) =="
check "the installer refuses a missing binary" 1 "--binary" - bash "$REPO/scripts/k2sign/install-k2-signer.sh" --test --binary "$tmp/nope"
check "a dry run changes nothing" 0 "dry run" - bash "$REPO/scripts/k2sign/install-k2-signer.sh" --test --dry-run --binary "$BIN"
[ ! -e "$L/k2-signer-bin" ] && pass "…nothing installed by the dry run" || fail "dry run installed" ""
check "install" 0 "installed. Hashes" - bash "$REPO/scripts/k2sign/install-k2-signer.sh" --test --binary "$BIN"
for f in k2-signer k2-signer-admin k2-signer-bin; do
    grep -q "  libexec/$f\$" "$K2_SIGNER_TEST_ROOT/usr/local/etc/k2-signer/pins.sha256" && pass "pins.sha256 pins $f" || fail "pin $f" "$(cat "$K2_SIGNER_TEST_ROOT/usr/local/etc/k2-signer/pins.sha256")"
done
grep -q "NOPASSWD: /usr/local/libexec/k2-signer/k2-signer\$" "$K2_SIGNER_TEST_ROOT/etc/sudoers.d/k2-signer" && pass "the sudoers rule names the one wrapper" || fail "sudoers" ""
[ "$(stat -f '%Lp' "$K2_SIGNER_TEST_ROOT/var/k2sign/k2-signer")" = 700 ] && pass "/var/k2sign/k2-signer is 0700" || fail "var mode" ""

echo "== the wrappers =="
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
echo
if [ "$FAILS" = 0 ]; then echo "k2-signer-install: all passed"; else echo "k2-signer-install: $FAILS FAILED"; exit 1; fi
