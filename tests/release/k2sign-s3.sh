#!/usr/bin/env bash
# k2sign-s3.sh — prd-signing-user-v1 S3 acceptance: the per-file checks the
# signer runs itself, in test mode (macOS, throwaway keys, a fake GitHub API
# on 127.0.0.1, a gh stub standing in for Sigstore's crypto, ad hoc-signed
# Mac fixtures). No K2SIGN_TEST_EVIDENCE here: every signature below passed
# the real evidence code.
#   bash tests/release/k2sign-s3.sh
source "$(dirname "$0")/k2sign_fixture.sh"
ready
command -v cc >/dev/null && command -v codesign >/dev/null || { echo "FAIL: cc and codesign needed (Command Line Tools)" >&2; exit 1; }
V=0.46.20
REPOP="/repos/Alakazam-211/K2"
T="$(printf 'c%.0s' $(seq 40))"
IDENT="https://github.com/Alakazam-211/K2/.github/workflows/daemon-binaries.yml@refs/tags/v$V"
ASSETS=(K2.app.tar.gz k2-daemon-linux-aarch64 k2-daemon-linux-x86_64 k2-daemon-macos-aarch64)

open_session() { # <version> <base> [--windows] assets…
    local v="$1" b="$2"; shift 2
    "$K" challenge "$v" "$b" "$@" > "$tmp/c.json" 2>"$tmp/c.err" || { fail "challenge $v" "$(cat "$tmp/c.err")"; return 1; }
    approve_with "$tmp/keys/approver.key" "$tmp/c.json" "$tmp/c.der"
    "$K" approve "$(nonce_of "$tmp/c.json")" < "$tmp/c.der" >/dev/null 2>"$tmp/c.err" || fail "approve $v" "$(cat "$tmp/c.err")"
}
release_routes() { # <version> <base> <tag commit> [changed files…]: GitHub as after Step 8.9
    local v="$1" b="$2" t="$3"; shift 3
    local files='[{"filename": "package.json"}, {"filename": "Cargo.lock"}]'
    if [ $# -gt 0 ]; then files="$(python3 -I -c 'import json,sys; print(json.dumps([{"filename": f} for f in sys.argv[1:]]))' "$@")"; fi
    route "$REPOP/git/ref/tags/v$v" "{\"object\": {\"type\": \"commit\", \"sha\": \"$t\"}}"
    route "$REPOP/commits/$t" "{\"sha\": \"$t\", \"parents\": [{\"sha\": \"$b\"}]}"
    route "$REPOP/compare/$b...$t" "{\"status\": \"ahead\", \"ahead_by\": 1, \"files\": $files}"
    route "$REPOP/compare/$b...main" '{"status": "ahead"}'
}
attest() { # <file> <identity> <ref> <commit>: an attestation gh would show for these bytes
    python3 -I - "$FIX/attest" "$@" <<'PY'
import hashlib, json, os, sys
d, f, ident, ref, commit = sys.argv[1:6]
sha = hashlib.sha256(open(f, "rb").read()).hexdigest()
json.dump([{"verificationResult": {
    "statement": {"subject": [{"name": os.path.basename(f), "digest": {"sha256": sha}}]},
    "signature": {"certificate": {"subjectAlternativeName": ident, "sourceRepositoryRef": ref,
                                  "sourceRepositoryDigest": commit}}}}], open(os.path.join(d, sha + ".json"), "w"))
PY
}
mkdaemon() { # <out> <arch> <version> [two-markers|no-ptyd]
    python3 -I - "$@" <<'PY'
import hashlib, os, struct, sys
out, arch, version, flags = sys.argv[1], sys.argv[2], sys.argv[3], set(sys.argv[4:])
mach = {"x86_64": 62, "aarch64": 183}[arch]
def elf(machine, size):
    h = bytearray(64)
    h[0:4] = b"\x7fELF"; h[4] = 2; h[5] = 1; h[6] = 1
    struct.pack_into("<H", h, 18, machine)
    struct.pack_into("<Q", h, 0x28, size - 2 * 64)
    struct.pack_into("<HH", h, 0x3A, 64, 2)
    return bytes(h) + os.urandom(size - 64)
ptyd = elf(mach, 8192)
body = elf(mach, 4096)
if "no-ptyd" not in flags:
    body += ptyd + hashlib.sha256(ptyd).hexdigest().encode()
marker = ("K2_DAEMON_BUILD v=%s os=linux arch=%s;" % (version, arch)).encode()
body += b"\0" + marker
if "two-markers" in flags:
    body += b"\0" + marker
open(out, "wb").write(body)
PY
}
sign() { "$K" sign "$@" < "$SIGN_IN" > "$tmp/sig.out" 2> "$tmp/sig.err"; }
# signs <name> <file> <slot> <asset> <version>: must sign, and verify with the pinned key.
signs() {
    local name="$1" f="$2" slot="$3" asset="$4" v="$5" pub="$U2_LINE"
    [ "$slot" = u1 ] && pub="$U1_LINE"
    if SIGN_IN="$f" sign "$slot" "$asset" "$v" raw && "$MINISIGN" -V -P "$pub" -m "$f" -x "$tmp/sig.out" -q; then
        pass "$name"
    else
        fail "$name" "$(cat "$tmp/sig.err")"
    fi
}
# refuses <name> <file> <slot> <asset> <version> <text>
refuses() {
    local name="$1" f="$2" rc=0
    SIGN_IN="$f" sign "$3" "$4" "$5" raw || rc=$?
    if [ "$rc" = 3 ] && grep -qF -- "$6" "$tmp/sig.err" && [ ! -s "$tmp/sig.out" ]; then pass "$name"; else fail "$name" "rc=$rc $(cat "$tmp/sig.err")"; fi
}

echo "== Linux daemons: GitHub's tag, the base, main, the bytes, the attestation =="
open_session "$V" "$B1" "${ASSETS[@]}"
mkdaemon "$tmp/lx" x86_64 "$V"; mkdaemon "$tmp/la" aarch64 "$V"
refuses "no tag on GitHub yet" "$tmp/lx" u2 k2-daemon-linux-x86_64 "$V" "no tag v$V on GitHub yet"
release_routes "$V" "$B1" "$T"
refuses "S3-1 no attestation for the bytes" "$tmp/lx" u2 k2-daemon-linux-x86_64 "$V" "no build attestation for $(shasum -a 256 "$tmp/lx" | awk '{print $1}')"
attest "$tmp/lx" "https://github.com/Alakazam-211/K2/.github/workflows/daemon-binaries.yml@refs/heads/main" refs/heads/main "$T"
refuses "S3-2 an attestation from main, not the tag" "$tmp/lx" u2 k2-daemon-linux-x86_64 "$V" "attestation identity is https://github.com/Alakazam-211/K2/.github/workflows/daemon-binaries.yml@refs/heads/main"
attest "$tmp/lx" "$IDENT" "refs/tags/v$V" "$B2"
refuses "an attestation at another commit" "$tmp/lx" u2 k2-daemon-linux-x86_64 "$V" "attestation source is refs/tags/v$V@$B2"
attest "$tmp/lx" "$IDENT" "refs/tags/v$V" "$T"
attest "$tmp/la" "$IDENT" "refs/tags/v$V" "$T"
: > "$FIX/gh.log"
signs "a Linux daemon with all its evidence signs (u2)" "$tmp/lx" u2 k2-daemon-linux-x86_64 "$V"
grep -qF -- "--cert-identity $IDENT --source-ref refs/tags/v$V --source-digest $T" "$FIX/gh.log" \
    && pass "gh ran the strict check: exact identity, tag ref and tag commit" || fail "strict gh call" "$(cat "$FIX/gh.log")"
signs "…and u1 (the bridge)" "$tmp/lx" u1 k2-daemon-linux-x86_64 "$V"
signs "aarch64 too" "$tmp/la" u2 k2-daemon-linux-aarch64 "$V"
grep -q "^/repos/Alakazam-211/K2/compare/$B1...main$" "$FIX/api.log" && pass "the signer asked GitHub itself whether the base is on main" || fail "main check call"
mkdaemon "$tmp/lx-arch" aarch64 "$V"; attest "$tmp/lx-arch" "$IDENT" "refs/tags/v$V" "$T"
V2=0.46.21; open_session "$V2" "$B1" "${ASSETS[@]}"; T2="$(printf 'd%.0s' $(seq 40))"
release_routes "$V2" "$B1" "$T2"
IDENT2="https://github.com/Alakazam-211/K2/.github/workflows/daemon-binaries.yml@refs/tags/v$V2"
mkdaemon "$tmp/w-arch" aarch64 "$V2"; attest "$tmp/w-arch" "$IDENT2" "refs/tags/v$V2" "$T2"
refuses "an aarch64 build offered as x86_64 (check-daemon-asset.py, pinned copy)" "$tmp/w-arch" u2 k2-daemon-linux-x86_64 "$V2" "k2-daemon-linux-x86_64:"
mkdaemon "$tmp/w-two" x86_64 "$V2" two-markers; attest "$tmp/w-two" "$IDENT2" "refs/tags/v$V2" "$T2"
refuses "two build markers" "$tmp/w-two" u2 k2-daemon-linux-x86_64 "$V2" "marker"
mkdaemon "$tmp/w-old" x86_64 0.46.19; attest "$tmp/w-old" "$IDENT2" "refs/tags/v$V2" "$T2"
refuses "an older build's bytes under the new tag" "$tmp/w-old" u2 k2-daemon-linux-x86_64 "$V2" "built as v0.46.19, release is v$V2"
route "$REPOP/commits/$T2" "{\"sha\": \"$T2\", \"parents\": [{\"sha\": \"$B2\"}]}"
mkdaemon "$tmp/g2" x86_64 "$V2"; attest "$tmp/g2" "$IDENT2" "refs/tags/v$V2" "$T2"
refuses "S3-3 the tag commit's parent is not the approved base" "$tmp/g2" u2 k2-daemon-linux-x86_64 "$V2" "release commit is not base+bump"
release_routes "$V2" "$B1" "$T2" package.json crates/k2-core/src/lib.rs
refuses "S3-4 the release commit touches a non-release file" "$tmp/g2" u2 k2-daemon-linux-x86_64 "$V2" "release commit changes non-release files: crates/k2-core/src/lib.rs"
release_routes "$V2" "$B1" "$T2"
route "$REPOP/compare/$B1...main" '{"status": "diverged"}'
refuses "S3-5 the base is not on GitHub main" "$tmp/g2" u2 k2-daemon-linux-x86_64 "$V2" "base commit is not on GitHub main"
route "$REPOP/compare/$B1...main" '{"status": "identical"}'
signs "base = main's tip (identical) is fine" "$tmp/g2" u2 k2-daemon-linux-x86_64 "$V2"
grep -q $'\tREFUSED-EVIDENCE\tu2\tk2-daemon-linux-x86_64\t' "$AUDIT/$V2/signatures.tsv" && pass "evidence refusals are in the signer's audit log" || fail "evidence audit"
# The tag commit IS the base (no bump commit): allowed.
V3=0.46.22; open_session "$V3" "$B1" "${ASSETS[@]}"
route "$REPOP/git/ref/tags/v$V3" "{\"object\": {\"type\": \"tag\", \"sha\": \"$(printf 'e%.0s' $(seq 40))\"}}"
route "$REPOP/git/tags/$(printf 'e%.0s' $(seq 40))" "{\"object\": {\"type\": \"commit\", \"sha\": \"$B1\"}}"
route "$REPOP/commits/$B1" '{"parents": [{"sha": "0000000000000000000000000000000000000000"}]}'
mkdaemon "$tmp/g3" x86_64 "$V3"; attest "$tmp/g3" "https://github.com/Alakazam-211/K2/.github/workflows/daemon-binaries.yml@refs/tags/v$V3" "refs/tags/v$V3" "$B1"
signs "an annotated tag on the base commit itself" "$tmp/g3" u2 k2-daemon-linux-x86_64 "$V3"
# The token: a wrong one is refused by GitHub, so nothing signs.
mv "$FIX/token" "$FIX/token.bak"; printf 'other\n' > "$FIX/token"
mkdaemon "$tmp/g4" aarch64 "$V3"; attest "$tmp/g4" "https://github.com/Alakazam-211/K2/.github/workflows/daemon-binaries.yml@refs/tags/v$V3" "refs/tags/v$V3" "$B1"
refuses "GitHub refusing the signer's token stops the signature" "$tmp/g4" u2 k2-daemon-linux-aarch64 "$V3" "returned error: 401"
mv "$FIX/token.bak" "$FIX/token"

echo "== the macOS daemon: arm64, one marker for this version, k2-ptyd signed by the team =="
printf 'int main(void){return 0;}\n' > "$tmp/p.c"
cc -arch arm64 -o "$tmp/ptyd" "$tmp/p.c"
codesign -f -s - -i dev.k2.app.ptyd "$tmp/ptyd" 2>/dev/null
cc -arch arm64 -o "$tmp/outer" "$tmp/p.c"
mkmac() { # <out> <marker version|none> [no-ptyd]
    python3 -I - "$tmp/outer" "$tmp/ptyd" "$@" <<'PY'
import hashlib, sys
outer, ptyd, out, mv = sys.argv[1:5]
b = open(outer, "rb").read()
if "no-ptyd" not in sys.argv[5:]:
    p = open(ptyd, "rb").read()
    b += b"\0" * (16 - len(b) % 16) + p + hashlib.sha256(p).hexdigest().encode()
if mv != "none":
    b += b"\0K2_DAEMON_BUILD v=%s os=macos arch=aarch64;" % mv.encode()
open(out, "wb").write(b)
PY
}
mkmac "$tmp/md" "$V"
refuses "an ad hoc k2-ptyd inside: not our team (the real anchor)" "$tmp/md" u2 k2-daemon-macos-aarch64 "$V" "is not signed by team 36B8R93HXV"
export K2SIGN_TEST_ANCHOR='always'
signs "the macOS daemon with its evidence (test anchor for the ad hoc ptyd)" "$tmp/md" u2 k2-daemon-macos-aarch64 "$V"
mkmac "$tmp/md-none" none; refuses "no build marker" "$tmp/md-none" u2 k2-daemon-macos-aarch64 "$V3" "must carry exactly one build marker"
mkmac "$tmp/md-old" 0.46.19; refuses "a marker for another version" "$tmp/md-old" u2 k2-daemon-macos-aarch64 "$V3" "found 0.46.19/macos/aarch64"
mkmac "$tmp/md-np" "$V3" no-ptyd; refuses "no embedded k2-ptyd" "$tmp/md-np" u2 k2-daemon-macos-aarch64 "$V3" "embeds no k2-ptyd"
cp "$tmp/lx" "$tmp/elf-as-mac"; refuses "an ELF offered as the macOS daemon" "$tmp/elf-as-mac" u2 k2-daemon-macos-aarch64 "$V3" "is not an arm64 Mach-O"

echo "== K2.app.tar.gz: team, notarized, version, nested helper and k2-ptyd =="
mkapp() { # <out tgz> <version> [no-ptyd]
    local d="$tmp/appsrc" a
    rm -rf "$d"; a="$d/K2.app"
    mkdir -p "$a/Contents/MacOS" "$a/Contents/Helpers/K2 Daemon.app/Contents/MacOS"
    cp "$tmp/outer" "$a/Contents/MacOS/k2"
    cp "$tmp/outer" "$a/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon"
    [ "${3:-}" = no-ptyd ] || cp "$tmp/ptyd" "$a/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-ptyd"
    printf '<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>dev.k2.app.daemon</string><key>CFBundleExecutable</key><string>k2-daemon</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>\n' \
        > "$a/Contents/Helpers/K2 Daemon.app/Contents/Info.plist"
    printf '<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>dev.k2.app</string><key>CFBundleExecutable</key><string>k2</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>%s</string></dict></plist>\n' "$2" \
        > "$a/Contents/Info.plist"
    [ "${3:-}" = no-ptyd ] || codesign -f -s - -i dev.k2.app.ptyd "$a/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-ptyd" 2>/dev/null
    codesign -f -s - "$a/Contents/Helpers/K2 Daemon.app" 2>/dev/null
    codesign -f -s - "$a" 2>/dev/null
    COPYFILE_DISABLE=1 tar -czf "$1" -C "$d" K2.app
}
V4=0.46.23; open_session "$V4" "$B1" "${ASSETS[@]}"
mkapp "$tmp/app.tgz" "$V4"
unset K2SIGN_TEST_ANCHOR
refuses "S3-6 an ad hoc K2.app: not our team" "$tmp/app.tgz" u2 K2.app.tar.gz "$V4" "K2.app is not signed by team 36B8R93HXV"
export K2SIGN_TEST_ANCHOR='always'
refuses "S3-6 not stapled / notarized" "$tmp/app.tgz" u2 K2.app.tar.gz "$V4" "K2.app is not notarized"
export K2SIGN_TEST_NOTARIZED=1
V5=0.46.24; open_session "$V5" "$B1" "${ASSETS[@]}"
refuses "S3-6 a $V4 app in a $V5 session" "$tmp/app.tgz" u2 K2.app.tar.gz "$V5" "K2.app is $V4, session is $V5"
mkapp "$tmp/app-np.tgz" "$V5" no-ptyd
refuses "no k2-ptyd in the daemon helper" "$tmp/app-np.tgz" u2 K2.app.tar.gz "$V5" "K2.app has no k2-ptyd"
mkapp "$tmp/app5.tgz" "$V5"
signs "K2.app.tar.gz with its evidence" "$tmp/app5.tgz" u2 K2.app.tar.gz "$V5"
printf 'not a tarball' > "$tmp/junk"; refuses "a file that isn't a tarball" "$tmp/junk" u1 K2.app.tar.gz 0.46.25 "no approved signing session"
unset K2SIGN_TEST_NOTARIZED K2SIGN_TEST_ANCHOR

echo "== the Windows setup: only when approved, and a PE32+ for x64 =="
V6=0.46.26; open_session "$V6" "$B1" --windows K2.app.tar.gz setup.exe "K2_${V6}_x64-setup.exe"
python3 -I -c '
import struct, sys
b = bytearray(512); b[0:2] = b"MZ"; struct.pack_into("<I", b, 0x3C, 0x80); b[0x80:0x84] = b"PE\0\0"
struct.pack_into("<H", b, 0x84, int(sys.argv[2], 16)); open(sys.argv[1], "wb").write(bytes(b))' "$tmp/setup.exe" 8664
python3 -I -c '
import struct, sys
b = bytearray(512); b[0:2] = b"MZ"; struct.pack_into("<I", b, 0x3C, 0x80); b[0x80:0x84] = b"PE\0\0"
struct.pack_into("<H", b, 0x84, int(sys.argv[2], 16)); open(sys.argv[1], "wb").write(bytes(b))' "$tmp/setup86.exe" 14c
refuses "a 32-bit setup" "$tmp/setup86.exe" u1 setup.exe "$V6" "is not an x64 Windows executable"
signs "the approved Windows setup (u1 name)" "$tmp/setup.exe" u1 setup.exe "$V6"
signs "…and the u2 name" "$tmp/setup.exe" u2 "K2_${V6}_x64-setup.exe" "$V6"
refuses "S3-7 the same setup name, other bytes" "$tmp/setup86.exe" u2 "K2_${V6}_x64-setup.exe" "$V6" "REFUSED-SWAP"
refuses "S3-8 a Windows setup outside a windows session" "$tmp/setup.exe" u1 setup.exe "$V5" "is not in $V5's approved asset list"

echo "== gh-selfcheck: the pinned gh, the token and gh's JSON fields (cli/cli's attestation of the gh zip) =="
check "gh-selfcheck with no attestation" 3 "gh could not verify cli/cli's attestation" "$K" gh-selfcheck
attest "$TROOT/usr/local/libexec/k2sign/gh-release.zip" "https://github.com/cli/cli/.github/workflows/deployment.yml@refs/heads/trunk" refs/heads/trunk "$T"
check "gh-selfcheck ok" 0 '"sourceRepositoryRef": "refs/heads/trunk"' "$K" gh-selfcheck
python3 -I - "$FIX/attest" "$TROOT/usr/local/libexec/k2sign/gh-release.zip" <<'PY'
import hashlib, json, os, sys
sha = hashlib.sha256(open(sys.argv[2], "rb").read()).hexdigest()
p = os.path.join(sys.argv[1], sha + ".json")
r = json.load(open(p)); del r[0]["verificationResult"]["signature"]["certificate"]["sourceRepositoryDigest"]
json.dump(r, open(p, "w"))
PY
check "gh-selfcheck: a JSON shape without the fields the signer reads" 3 "lacks the fields the signer reads" "$K" gh-selfcheck

echo "== policy drift =="
python3 -I - "$REPO/scripts/release.sh" "$REPO/scripts/k2sign/policy" <<'PY' && pass "policy release_commit_files == release.sh RELEASE_COMMIT_FILES" || fail "release_commit_files drift"
import re, sys
r = open(sys.argv[1]).read()
block = re.search(r"RELEASE_COMMIT_FILES=\(\n(.*?)\n\)", r, re.S).group(1)
want = set(block.split())
pol = [l for l in open(sys.argv[2]).read().splitlines() if l.startswith("release_commit_files")][0]
got = set(pol.split("=", 1)[1].split())
sys.exit(0 if want == got else 1)
PY

finish
