#!/usr/bin/env bash
# Spike (prd-tauri-core-0467-v1 TC-26, vs-live TA-24): sign, notarize and
# updater-sign a Tauri .app UNATTENDED inside one K2 Compute job on the
# owner's own Mac node, as the node's job user (_k2node), inside its
# Seatbelt wall (no /Users, no /Library/Keychains, no login keychain).
#
# The trick: a temporary keychain INSIDE the job folder, with the
# Developer ID .p12 and Apple's intermediate imported into it, used by
# `codesign --keychain` explicitly (the user's keychain search list is never
# touched). Everything is removed on exit, success or not.
#
# Two modes:
#   --selftest          No real key anywhere. Makes a throwaway self-signed
#                       code-signing identity and a dummy .app, then proves
#                       criteria 1, 2 (no timestamp: a self-signed identity
#                       gets none) and 6 inside the wall. Safe on any node.
#   (default, real)     Criteria 1-4 and 6 with the owner's keys, delivered
#                       to this job only as files/env by
#                       `k2 compute run --secret` (never argv, never logs):
#     K2_SIGN_P12_B64         base64 of the Developer ID Application .p12
#     K2_SIGN_P12_PASSWORD    its password
#     K2_SIGN_IDENTITY        e.g. "Developer ID Application: Example Co (TEAMID1234)"
#     K2_NOTARY_KEY_P8_B64    base64 of the App Store Connect API key (.p8)
#     K2_NOTARY_KEY_ID, K2_NOTARY_ISSUER
#     TAURI_SIGNING_PRIVATE_KEY, TAURI_SIGNING_PRIVATE_KEY_PASSWORD   (tauri signer)
#   and: --app <path/to/My.app> [--intermediate <DeveloperIDG2CA.cer>]
#
# Criterion 5 (spctl -a -vv on ANOTHER Mac) and 7 (refuse a node that isn't
# the owner's own) are outside this script: see
# docs/internal/spike-macos-signing-in-a-job.md.
#
# Usage (from a K2 workspace, on one of the owner's Mac nodes):
#   k2 compute run k2n2-m1 --sha HEAD -- bash scripts/spike/macos-sign-in-job.sh --selftest
#   k2 compute run <owner-mac-node> --sha HEAD --secret SIGN_P12_B64=K2_SIGN_P12_B64 … \
#       -- bash scripts/spike/macos-sign-in-job.sh --app target/release/bundle/macos/My.app

set -euo pipefail

MODE=real
APP=""
INTERMEDIATE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --selftest) MODE=selftest; shift ;;
        --app) APP="${2:-}"; shift 2 ;;
        --intermediate) INTERMEDIATE="${2:-}"; shift 2 ;;
        -h|--help) sed -n '2,40p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

[ "$(uname -s)" = Darwin ] || { echo "FAIL: macOS only" >&2; exit 2; }

WORK="$(pwd -P)/.k2-sign-$$"
KC="$WORK/sign.keychain-db"
KC_PW="$(openssl rand -hex 16)"
RESULTS=()

pass() { RESULTS+=("PASS $1"); echo "PASS $1"; }
fail() { RESULTS+=("FAIL $1"); echo "FAIL $1" >&2; }

cleanup() {
    local rc=$?
    security delete-keychain "$KC" >/dev/null 2>&1 || true
    if [ -d "$WORK" ]; then
        # Key material first, then the folder.
        find "$WORK" -type f \( -name '*.p12' -o -name '*.p8' -o -name '*.key' -o -name '*.pem' \) -exec rm -P {} + 2>/dev/null || true
        rm -rf "$WORK"
    fi
    # Criterion 6: nothing left.
    if [ -e "$WORK" ] || [ -e "$KC" ]; then
        echo "FAIL 6 cleanup: $WORK or the keychain is still there" >&2
        rc=1
    else
        echo "PASS 6 cleanup: keychain deleted, key files removed, work folder gone"
    fi
    printf '%s\n' "${RESULTS[@]+"${RESULTS[@]}"}" | sed 's/^/  /'
    exit "$rc"
}
trap cleanup EXIT
umask 077
mkdir -p "$WORK"

echo "== who and where =="
echo "user: $(id -un) ($(id -u)); HOME=${HOME:-}; job folder: $(pwd -P)"
echo "macOS $(sw_vers -productVersion); $(xcode-select -p 2>/dev/null || echo 'no Xcode / CLT')"
if ls /Users >/dev/null 2>&1; then echo "note: /Users is readable (no wall?)"; else echo "/Users is walled off (expected)"; fi

echo "== 1. a temporary keychain inside the job folder =="
security create-keychain -p "$KC_PW" "$KC"
security set-keychain-settings -lut 3600 "$KC"
security unlock-keychain -p "$KC_PW" "$KC"

if [ "$MODE" = selftest ]; then
    # A throwaway identity: never a real key.
    cat >"$WORK/cs.cnf" <<'EOF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = K2 spike throwaway code signing
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
    openssl req -x509 -newkey rsa:2048 -nodes -days 1 -keyout "$WORK/cs.key" -out "$WORK/cs.pem" -config "$WORK/cs.cnf" >/dev/null 2>&1
    P12_PW="spike-$$"
    openssl pkcs12 -export -inkey "$WORK/cs.key" -in "$WORK/cs.pem" -out "$WORK/id.p12" -passout "pass:$P12_PW" \
        -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 >/dev/null 2>&1
    IDENTITY="$(openssl x509 -in "$WORK/cs.pem" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')"
    # A dummy .app: a copy of a system binary, re-signed.
    APP="$WORK/Spike.app"
    mkdir -p "$APP/Contents/MacOS"
    cp /usr/bin/true "$APP/Contents/MacOS/spike"
    cat >"$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>spike</string>
<key>CFBundleIdentifier</key><string>dev.k2.spike.signing</string>
<key>CFBundleName</key><string>Spike</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.0.1</string>
</dict></plist>
EOF
else
    : "${K2_SIGN_P12_B64:?K2_SIGN_P12_B64 (via --secret)}" "${K2_SIGN_P12_PASSWORD:?}" "${K2_SIGN_IDENTITY:?}"
    [ -n "$APP" ] && [ -d "$APP" ] || { echo "FAIL: --app <path/to/My.app> is required" >&2; exit 2; }
    printf '%s' "$K2_SIGN_P12_B64" | base64 -D >"$WORK/id.p12"
    P12_PW="$K2_SIGN_P12_PASSWORD"
    IDENTITY="$K2_SIGN_IDENTITY"
fi

security import "$WORK/id.p12" -k "$KC" -P "$P12_PW" -T /usr/bin/codesign -T /usr/bin/security >/dev/null
# Let codesign use the key without a GUI prompt (the errSecInternalComponent
# trap in a session with no GUI).
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KC_PW" "$KC" >/dev/null
if [ "$MODE" = real ]; then
    # The wall denies /Library/Keychains, so the system's copy of Apple's
    # Developer ID intermediate isn't reachable: import it here.
    if [ -z "$INTERMEDIATE" ]; then
        INTERMEDIATE="$WORK/DeveloperIDG2CA.cer"
        curl -fsS --proto '=https' --tlsv1.2 -o "$INTERMEDIATE" https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer
    fi
    security import "$INTERMEDIATE" -k "$KC" >/dev/null
fi
if [ "$MODE" = selftest ]; then
    # codesign uses only TRUSTED identities. A real Developer ID chains to
    # Apple's root through the imported intermediate; a self-signed one has
    # to be trusted by hand, which needs an authorization the job may not
    # get (no GUI). Try it and say what happened.
    if security add-trusted-cert -r trustRoot -p codeSign -k "$KC" "$WORK/cs.pem" 2>"$WORK/trust.err"; then
        echo "  trusted the throwaway root for code signing (user trust settings)"
    else
        echo "  add-trusted-cert refused: $(head -1 "$WORK/trust.err")"
    fi
fi
security find-identity -p codesigning "$KC" | sed 's/^/  /'
pass "1 keychain: created in the job folder, identity imported$([ "$MODE" = real ] && echo ', intermediate imported')"

if [ "$MODE" = selftest ]; then
    echo "== probes for the real run (no key needed) =="
    # Does trust evaluation work from inside the wall (trustd + the system
    # roots), so a real Developer ID chain can be valid here?
    if curl -fsS --proto '=https' --tlsv1.2 -o "$WORK/DeveloperIDG2CA.cer" https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer; then
        security import "$WORK/DeveloperIDG2CA.cer" -k "$KC" >/dev/null 2>&1 || true
        if security verify-cert -c "$WORK/DeveloperIDG2CA.cer" -p basic -k "$KC" >"$WORK/v.out" 2>&1; then
            echo "  probe: Apple's Developer ID G2 intermediate verifies to Apple's root from inside the wall"
        else
            echo "  probe: the intermediate does NOT verify here: $(head -1 "$WORK/v.out")"
        fi
    else
        echo "  probe: can't fetch Apple's intermediate (network)"
    fi
    if curl -fsS -o /dev/null -w '%{http_code}' http://timestamp.apple.com/ts01 >"$WORK/ts.out" 2>/dev/null || [ -s "$WORK/ts.out" ]; then
        echo "  probe: timestamp.apple.com answers (HTTP $(cat "$WORK/ts.out"))"
    else
        echo "  probe: timestamp.apple.com unreachable"
    fi
    if xcrun --find notarytool >/dev/null 2>&1; then echo "  probe: notarytool at $(xcrun --find notarytool)"; else echo "  probe: no notarytool (install Xcode or the Command Line Tools)"; fi
    if xcrun --find stapler >/dev/null 2>&1; then echo "  probe: stapler at $(xcrun --find stapler)"; else echo "  probe: no stapler"; fi
    if command -v npx >/dev/null 2>&1; then echo "  probe: npx present (tauri signer sign via @tauri-apps/cli)"; else echo "  probe: no npx on this node (tauri signer needs node: --needs node@22)"; fi
fi

echo "== 2. codesign =="
if [ "$MODE" = selftest ]; then
    if codesign --force --timestamp=none --options runtime --keychain "$KC" --sign "$IDENTITY" "$APP" 2>"$WORK/cs.err"; then
        codesign --verify --strict --verbose=2 "$APP" 2>&1 | sed 's/^/  /' || true
        pass "2 codesign (self-signed, no timestamp) inside the wall"
    else
        sed 's/^/  /' "$WORK/cs.err" >&2
        fail "2 codesign: $(head -1 "$WORK/cs.err")"
    fi
else
    if codesign --force --timestamp --options runtime --keychain "$KC" --sign "$IDENTITY" --deep "$APP" 2>"$WORK/cs.err"; then
        codesign --verify --strict --deep --verbose=2 "$APP" 2>&1 | sed 's/^/  /'
        pass "2 codesign --timestamp --options runtime"
    else
        sed 's/^/  /' "$WORK/cs.err" >&2
        fail "2 codesign: $(head -1 "$WORK/cs.err")"
    fi

    echo "== 3. notarize and staple =="
    printf '%s' "${K2_NOTARY_KEY_P8_B64:?}" | base64 -D >"$WORK/notary.p8"
    ditto -c -k --keepParent "$APP" "$WORK/app.zip"
    if xcrun notarytool submit "$WORK/app.zip" --key "$WORK/notary.p8" --key-id "${K2_NOTARY_KEY_ID:?}" --issuer "${K2_NOTARY_ISSUER:?}" --wait 2>&1 | tee "$WORK/notary.log" | sed 's/^/  /'; then
        if grep -q "status: Accepted" "$WORK/notary.log" && xcrun stapler staple "$APP" >/dev/null; then
            pass "3 notarytool Accepted; stapler staple"
        else
            fail "3 notarization not accepted or staple failed"
        fi
    else
        fail "3 notarytool submit failed"
    fi

    echo "== 4. the updater signature =="
    TARBALL="$WORK/$(basename "$APP").tar.gz"
    tar -C "$(dirname "$APP")" -czf "$TARBALL" "$(basename "$APP")"
    if command -v npx >/dev/null && npx --yes @tauri-apps/cli@2 signer sign "$TARBALL" >/dev/null 2>"$WORK/tauri.err"; then
        [ -s "$TARBALL.sig" ] && pass "4 tauri signer sign wrote $(basename "$TARBALL").sig (k2 app updates push checks it against the App's key)"
        mkdir -p "$(pwd -P)/out" && cp "$TARBALL" "$TARBALL.sig" "$(pwd -P)/out/"
    else
        fail "4 tauri signer sign: $(head -1 "$WORK/tauri.err" 2>/dev/null || echo 'no npx')"
    fi
fi

for r in "${RESULTS[@]}"; do
    case "$r" in FAIL*) exit 1 ;; esac
done
exit 0
