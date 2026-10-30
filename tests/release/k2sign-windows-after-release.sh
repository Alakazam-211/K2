#!/usr/bin/env bash
# k2sign-windows-after-release.sh — the signer side of
# scripts/windows-sign-after-release.sh, in test mode (macOS, throwaway
# keys, an openssl P-256 key for the approval): a release that shipped
# without Windows gets a RE-approval for the two Windows names only, on
# the same base commit, and that session signs the setup and nothing else.
#   bash tests/release/k2sign-windows-after-release.sh
source "$(dirname "$0")/k2sign_fixture.sh"
ready
NOW="$(date +%s)"
export K2SIGN_TEST_NOW="$NOW"
V=0.46.11
MAC=(K2.app.tar.gz k2-daemon-linux-aarch64 k2-daemon-linux-x86_64 k2-daemon-macos-aarch64)
WIN=(setup.exe "K2_${V}_x64-setup.exe")

# session <args for challenge...>: challenge, approve with the enrolled key.
session() {
    "$K" challenge "$@" > "$tmp/c.json" 2>"$tmp/c.err" || { echo "challenge: $(cat "$tmp/c.err")"; return 1; }
    approve_with "$tmp/keys/approver.key" "$tmp/c.json" "$tmp/c.der"
    "$K" approve "$(nonce_of "$tmp/c.json")" < "$tmp/c.der" > "$tmp/a.out" 2>&1 || { echo "approve: $(cat "$tmp/a.out")"; return 1; }
}

# A minimal NSIS-shaped file: an i386 PE stub with the NSIS first header
# 4 bytes past a 512-byte boundary (what pe_evidence looks for).
python3 -I - "$tmp/setup.exe" <<'PY'
import struct, sys
b = bytearray(4096)
b[0:2] = b"MZ"
struct.pack_into("<I", b, 0x3C, 0x80)
b[0x80:0x84] = b"PE\0\0"
struct.pack_into("<H", b, 0x84, 0x014C)
b[1024 + 4:1024 + 4 + 16] = b"\xef\xbe\xad\xdeNullsoftInst"
open(sys.argv[1], "wb").write(bytes(b))
PY

echo "== the release: a session without Windows =="
if r="$(session "$V" "$B1" "${MAC[@]}")"; then pass "release session (no Windows)"; else fail "release session" "$r"; fi

echo "== adding the Windows setup afterwards =="
check "a second session needs --reapprove" 3 "already has an approved session" "$K" challenge "$V" "$B1" "${WIN[@]}" --windows
check "a re-approval on another base is refused" 3 "base commit differs" "$K" challenge "$V" "$B2" "${WIN[@]}" --windows --reapprove
check "the setup names without --windows are refused" 3 "Windows setup not approved" "$K" challenge "$V" "$B1" "${WIN[@]}" --reapprove
if r="$(session "$V" "$B1" "${WIN[@]}" --windows --reapprove)"; then pass "Windows-only re-approval on the release's base"; else fail "Windows re-approval" "$r"; fi
python3 -I - "$tmp/c.json" "$V" <<'PY' && pass "the challenge names windows, reapproval and only the two setup names" || fail "challenge fields" "$(cat "$tmp/c.json")"
import json, sys
c = json.load(open(sys.argv[1]))
assert c["windows"] is True and c["reapproval"] is True, c
assert c["assets"] == sorted(["setup.exe", "K2_%s_x64-setup.exe" % sys.argv[2]]), c["assets"]
PY

echo "== that session signs the setup, under both names, and nothing else =="
printf 'not a PE' > "$tmp/notpe"
check_in "a file that isn't an NSIS PE is refused (before anything is signed)" "$tmp/notpe" 3 "is not a Windows executable" "$K" sign u2 "K2_${V}_x64-setup.exe" "$V" raw
check_in "u1 signs setup.exe (b64, the latest.json signature)" "$tmp/setup.exe" 0 "" "$K" sign u1 setup.exe "$V" b64
check_in "u2 signs the URL name (raw, k2_signatures)" "$tmp/setup.exe" 0 "" "$K" sign u2 "K2_${V}_x64-setup.exe" "$V" raw
check_in "a macOS asset is outside the re-approved list" "$tmp/setup.exe" 3 "is not in ${V}'s approved asset list" "$K" sign u2 K2.app.tar.gz "$V" raw
cp "$tmp/setup.exe" "$tmp/setup2.exe"; printf 'x' >> "$tmp/setup2.exe"
check_in "REFUSED-SWAP: other setup bytes under a signed name" "$tmp/setup2.exe" 3 "REFUSED-SWAP" "$K" sign u1 setup.exe "$V" b64

finish
