#!/usr/bin/env bash
# Dry tests for update signing on z3mbpZ (prd-security-sweep-0464-v1 SS1,
# SS2/A7): scripts/lib/sign-updater.sh, scripts/sign-daemon.sh,
# scripts/windows-patch-latest-json.sh and the release.sh / workflow wiring.
#
# Stubs on PATH: minisign (tests/packaging/fixtures/stub-minisign.py, an
# HMAC stand-in that drives the same CLI and file formats), security (a
# temp "keychain" dir), gh (a temp "release" dir), curl. Throwaway keys are
# generated inside the test. Never touches the real keychain, the real key,
# ~/.k2, GitHub or .env; never runs release.sh. No cargo. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export PYTHONDONTWRITEBYTECODE=1
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD K2_RELEASE_REPO || true
# This file tests the keychain (stopgap) backend; tests/release/k2sign-*.sh
# test the signing user, the default backend.
export K2_SIGNER=keychain
unset K2SIGN_TEST_ROOT K2_SIGN_APPROVE_CMD || true

# ── stubs ──
bin="$tmp/bin"; mkdir -p "$bin"
cp "$SCRIPT_DIR/fixtures/stub-minisign.py" "$bin/minisign"; chmod +x "$bin/minisign"
cat > "$bin/security" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$STUB_SECURITY_LOG"
[ "$1" = find-generic-password ] || { echo "stub security: unsupported $1" >&2; exit 2; }
svc="" w=0
shift
while [ $# -gt 0 ]; do
    case "$1" in -s) svc="$2"; shift 2 ;; -w) w=1; shift ;; *) shift ;; esac
done
f="$STUB_KEYCHAIN/$svc"
[ -f "$f" ] || { echo "security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain." >&2; exit 44; }
[ "$w" = 1 ] && cat "$f"
exit 0
EOF
cat > "$bin/gh" <<'EOF'
#!/usr/bin/env bash
# Stub gh: a release is a dir $STUB_REL/<tag>. Logs every call.
printf '%s\n' "$*" >> "$STUB_GH_LOG"
case "$1 $2" in
    "secret list")
        [ "${STUB_SECRETS_FAIL:-0}" = 1 ] && { echo "HTTP 403: Resource not accessible by integration" >&2; exit 1; }
        [ -n "${STUB_SECRETS:-}" ] && printf '%s\tUpdated 2026-10-01\n' $STUB_SECRETS
        exit 0 ;;
    "release view")
        tag="$3"; ls "$STUB_REL/$tag" 2>/dev/null; exit 0 ;;
    "release download")
        tag="$3"; shift 3; dir="."; pats=()
        while [ $# -gt 0 ]; do
            case "$1" in -p|--pattern) pats+=("$2"); shift 2 ;; -D|--dir) dir="$2"; shift 2 ;; -R|--repo) shift 2 ;; *) shift ;; esac
        done
        for p in "${pats[@]}"; do
            [ -f "$STUB_REL/$tag/$p" ] || { echo "no asset $p" >&2; exit 1; }
            cp "$STUB_REL/$tag/$p" "$dir/"
        done
        exit 0 ;;
    "release upload")
        tag="$3"; shift 3
        while [ $# -gt 0 ]; do
            case "$1" in -R|--repo) shift 2 ;; --clobber) shift ;; *) cp "$1" "$STUB_REL/$tag/"; printf 'UPLOAD %s\n' "$(basename "$1")" >> "$STUB_GH_LOG"; shift ;; esac
        done
        # A swap right after the manifest goes up (the post-publish check).
        if [ -n "${STUB_SWAP_AFTER_MANIFEST:-}" ] && [ -f "$STUB_REL/$tag/daemon-latest.json" ]; then
            printf 'swapped' >> "$STUB_REL/$tag/$STUB_SWAP_AFTER_MANIFEST"
        fi
        exit 0 ;;
    "attestation verify")
        if [ "${3:-}" = "--help" ]; then
            printf '      --cert-identity string\n      --signer-workflow string\n'
            [ "${STUB_GH_OLD:-0}" = 1 ] || printf '      --source-digest string\n      --source-ref string\n'
            exit 0
        fi
        # Passes only for bytes listed in $STUB_ATTESTED (sha256 per line) and
        # only for the exact certificate identity + the tag ref and commit.
        f="$3"; shift 3; ident="" digest="" ref=""
        while [ $# -gt 0 ]; do
            case "$1" in --cert-identity) ident="$2"; shift 2 ;; --source-digest) digest="$2"; shift 2 ;; --source-ref) ref="$2"; shift 2 ;; *) shift ;; esac
        done
        sha="$(python3 -I -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$f")"
        [ "$ident" = "https://github.com/Alakazam-211/K2/.github/workflows/daemon-binaries.yml@$ref" ] || { echo "wrong certificate identity $ident" >&2; exit 1; }
        [ "$digest" = "${STUB_ATTESTED_COMMIT:-$STUB_TAG_COMMIT}" ] || { echo "no attestation for commit $digest" >&2; exit 1; }
        case "$ref" in refs/tags/v*) ;; *) echo "wrong ref $ref" >&2; exit 1 ;; esac
        grep -qx "$sha" "${STUB_ATTESTED:-/dev/null}" || { echo "Error: no attestations found for subject" >&2; exit 1; }
        exit 0 ;;
    "api repos/Alakazam-211/K2/commits/"*)
        printf '%s\n' "$STUB_TAG_COMMIT"; exit 0 ;;
    "api repos/Alakazam-211/K2/releases/tags/"*)
        tag="${2##*/}"
        python3 -I - "$STUB_REL/$tag" "${STUB_UPLOADER:-github-actions[bot]}" "${STUB_ASSET_TIME:-2026-10-24T12:00:00Z}" <<'PY'
import json, os, sys
d, who, t = sys.argv[1:4]
print(json.dumps({"created_at": "2026-10-24T11:00:00Z", "assets": [
    {"name": n, "uploader": {"login": who}, "created_at": t} for n in sorted(os.listdir(d))]}))
PY
        exit 0 ;;
esac
echo "stub gh: unsupported: $*" >&2; exit 2
EOF
cat > "$bin/curl" <<'EOF'
#!/usr/bin/env bash
[ "${STUB_CURL_FAIL:-0}" = 1 ] && { echo "curl: (22) The requested URL returned error: 404" >&2; exit 22; }
out="" url=""
while [ $# -gt 0 ]; do case "$1" in -o) out="$2"; shift 2 ;; -*) shift ;; *) url="$1"; shift ;; esac; done
cp "$STUB_CURL_FILE" "$out"
EOF
chmod +x "$bin/security" "$bin/gh" "$bin/curl"
export PATH="$bin:$PATH"
export STUB_MINISIGN_LOG="$tmp/minisign.log" STUB_SECURITY_LOG="$tmp/security.log" STUB_GH_LOG="$tmp/gh.log"
export STUB_KEYCHAIN="$tmp/keychain" STUB_REL="$tmp/rel"
mkdir -p "$STUB_KEYCHAIN" "$STUB_REL"
export HOME="$tmp/home"; mkdir -p "$HOME"
export K2_RELEASE_AUDIT_DIR="$tmp/audit"
export K2_UPDATER_SRC_ROOT="$tmp/src"
: > "$STUB_MINISIGN_LOG"; : > "$STUB_SECURITY_LOG"; : > "$STUB_GH_LOG"

# ── throwaway keys: U1 (the retired key), U2 (new primary), U3 (backup) ──
rand() { python3 -I -c 'import secrets; print(secrets.token_hex(12))'; }
PW="pw1-$(rand)"; PW2="pw2-$(rand)"
mkdir -p "$tmp/gen"
printf '%s\n' "$PW" | minisign -G -p "$tmp/gen/u1.pub" -s "$tmp/gen/u1.key" >/dev/null
printf '%s\n' "$PW2" | minisign -G -p "$tmp/gen/u2.pub" -s "$tmp/gen/u2.key" >/dev/null
PW3="pw3-$(rand)"
printf '%s\n' "$PW3" | minisign -G -p "$tmp/gen/u3.pub" -s "$tmp/gen/u3.key" >/dev/null
mkdir -p "$HOME/.k2-updater"; chmod 700 "$HOME/.k2-updater"
# U1 in Tauri format (base64 of the minisign secret key file, one line), as
# ~/.tauri/k2-updater.key is; U2 as minisign writes it.
python3 -I -c 'import base64,sys; sys.stdout.write(base64.b64encode(open(sys.argv[1],"rb").read()).decode())' "$tmp/gen/u1.key" > "$HOME/.k2-updater/k2-updater-u1.key"
cp "$tmp/gen/u2.key" "$HOME/.k2-updater/k2-updater-u2.key"
chmod 600 "$HOME/.k2-updater/k2-updater-u1.key" "$HOME/.k2-updater/k2-updater-u2.key"
printf '%s' "$PW" > "$STUB_KEYCHAIN/k2-updater-signing-u1"
printf '%s' "$PW2" > "$STUB_KEYCHAIN/k2-updater-signing-u2"
KEY_B64_FILE_CONTENT="$(cat "$HOME/.k2-updater/k2-updater-u1.key")"
KEY_INNER_LINE="$(sed -n 2p "$tmp/gen/u1.key")"
KEY2_INNER_LINE="$(sed -n 2p "$tmp/gen/u2.key")"
U1_PUB_LINE="$(sed -n 2p "$tmp/gen/u1.pub")"; U1_ID="$(sed -n 1p "$tmp/gen/u1.pub" | sed 's/.*: //')"
U2_PUB_LINE="$(sed -n 2p "$tmp/gen/u2.pub")"; U2_ID="$(sed -n 1p "$tmp/gen/u2.pub" | sed 's/.*: //')"
U3_PUB_LINE="$(sed -n 2p "$tmp/gen/u3.pub")"

# A fixture source tree: copies of the four verifier files, U1 swapped for
# the throwaway one, then the ceremony's own `pubkeys` writes U2/U3.
SRC="$K2_UPDATER_SRC_ROOT"
for f in crates/k2-release-keys/src/lib.rs cli/k2 scripts/install-daemon.sh src-tauri/tauri.conf.json; do
    mkdir -p "$SRC/$(dirname "$f")"; cp "$ROOT/$f" "$SRC/$f"
done
python3 -I - "$SRC" "$U1_PUB_LINE" <<'PY'
import base64, re, sys
root, u1 = sys.argv[1:3]
p = root + "/crates/k2-release-keys/src/lib.rs"
s = open(p).read()
s, n = re.subn(r'pub const LEGACY_UPDATER_KEY_B64: &str = "[^"]+";', 'pub const LEGACY_UPDATER_KEY_B64: &str = "%s";' % u1, s)
assert n == 1
open(p, "w").write(s)
PY

echo "== the ceremony writes the public keys into every verifier copy =="
if bash "$ROOT/scripts/updater-key-ceremony.sh" pubkeys --root "$SRC" "$U2_PUB_LINE" "$U3_PUB_LINE" >"$tmp/cer.out" 2>&1; then
    pass "pubkeys: written"
else
    fail "pubkeys" "$(cat "$tmp/cer.out")"
fi
python3 -I - "$SRC" "$U2_PUB_LINE" "$U3_PUB_LINE" "$U2_ID" <<'PY' && pass "the four copies agree (the Rust copies test's rule): primary U2, backup U3, tauri.conf.json = U2" || fail "copies" "see $SRC"
import base64, json, re, sys
root, u2, u3, u2id = sys.argv[1:5]
src = open(root + "/crates/k2-release-keys/src/lib.rs").read()
block = src[src.index("// BEGIN UPDATER_KEYS"):src.index("// END UPDATER_KEYS")]
keys = re.findall(r'role: "([a-z]+)",\s*key_id: "([0-9A-F]{16})",\s*public_key_b64: "([^"]+)"', block)
assert [(k[0], k[2]) for k in keys] == [("primary", u2), ("backup", u3)], keys
assert keys[0][1] == u2id
for f, var in (("cli/k2", "_DAEMON_UPDATE_PUBKEYS"), ("scripts/install-daemon.sh", "K2_DAEMON_PUBKEYS")):
    lines = re.findall(r'^%s="([^"]*)"$' % var, open(root + "/" + f).read(), re.M)
    assert lines == ["%s %s" % (u2, u3)], (f, lines)
pk = json.load(open(root + "/src-tauri/tauri.conf.json"))["plugins"]["updater"]["pubkey"]
assert base64.b64decode(pk).decode().splitlines() == ["untrusted comment: minisign public key: %s" % u2id, u2]
PY
cer_refuses() { # <label> <expect> <args…>
    local label="$1" expect="$2"; shift 2
    if bash "$ROOT/scripts/updater-key-ceremony.sh" pubkeys --root "$SRC" "$@" >"$tmp/cer.out" 2>&1; then
        fail "ceremony $label" "accepted"
    elif grep -q -- "$expect" "$tmp/cer.out"; then
        pass "ceremony refuses $label"
    else
        fail "ceremony $label" "$(cat "$tmp/cer.out")"
    fi
}
cer_refuses "the real U1" "retired key" "RWQFu172RhQR6QBqzmij2E9btDGhDWmpdH+ah1/M4PmuHPINUWvKLf6o" "$U3_PUB_LINE"
cer_refuses "one key twice" "two different keys" "$U2_PUB_LINE" "$U2_PUB_LINE"
cer_refuses "garbage" "not a minisign public key" "not-a-key" "$U3_PUB_LINE"
cer_refuses "a second identical run" "no change" "$U2_PUB_LINE" "$U3_PUB_LINE"

echo "== the U3 drill (restore the backup key, sign, check with UPDATER_KEYS' backup) =="
CER="$ROOT/scripts/updater-key-ceremony.sh"
restored() { cp "$1" "$tmp/restored.key"; printf '%s' "$tmp/restored.key"; }
if printf '%s\n' "$PW3" | bash "$CER" drill "$(restored "$tmp/gen/u3.key")" --root "$SRC" >"$tmp/drill.out" 2>&1 && grep -q "U3 drill ok" "$tmp/drill.out"; then
    pass "drill: U3 restored from its file signs, and the backup key in UPDATER_KEYS accepts it"
else
    fail "drill" "$(cat "$tmp/drill.out")"
fi
[ ! -e "$tmp/restored.key" ] && pass "drill deletes the restored key file when it finishes" || fail "drill cleanup" "restored key left behind"
drill_refuses() { # <label> <expect (ERE)> <key file> <password> [root]
    if printf '%s\n' "$4" | bash "$CER" drill "$(restored "$3")" --root "${5:-$SRC}" >"$tmp/drill.out" 2>&1; then
        fail "drill $1" "passed"
    elif [ -e "$tmp/restored.key" ]; then
        fail "drill $1" "left the restored key behind after a failure"
    elif grep -qE -- "$2" "$tmp/drill.out"; then
        pass "drill refuses $1"
    else
        fail "drill $1" "$(cat "$tmp/drill.out")"
    fi
}
drill_refuses "the U2 key (not the backup)" "NOT the backup key" "$tmp/gen/u2.key" "$PW2"
drill_refuses "a wrong password" "did not sign" "$tmp/gen/u3.key" "wrong"
drill_refuses "a tree whose UPDATER_KEYS don't hold this U3 (pre-ceremony: none)" "no U3 \\(backup\\) key|NOT the backup key" "$tmp/gen/u3.key" "$PW3" "$ROOT"
python3 -I - "$CER" <<'PY' && pass "ceremony: no clipboard; U3 file in a 0700 folder, password in a separate entry, shredded; abort trap removes what the run made; generate needs a terminal" || fail "ceremony static" "see $CER"
import re, sys
t = open(sys.argv[1]).read()
assert "pbcopy" not in t
assert 'mktemp -d "${TMPDIR:-/tmp}/K2-U3-backup.XXXXXX"' in t and 'chmod 700 "$U3DIR"' in t
assert 'tmutil addexclusion "$U3DIR"' in t
assert "SEPARATE entry" in t and "shred_file \"$U3DIR/k2-updater-u3.key\"" in t
assert "trap ceremony_cleanup EXIT" in t and "security delete-generic-password" in t
assert '[ -t 0 ] && [ -t 1 ]' in t
PY
if bash "$CER" generate </dev/null >"$tmp/gen.out" 2>&1; then fail "generate without a tty" "ran"; else grep -qE "macOS|terminal" "$tmp/gen.out" && pass "generate refuses to run without a terminal (or off macOS)" || fail "generate tty" "$(cat "$tmp/gen.out")"; fi

LIB="$ROOT/scripts/lib/sign-updater.sh"
lib() { bash -c 'source "$1"; shift; "$@"' _ "$LIB" "$@"; }

# The leak checks below are about the signing helper: start its logs fresh.
: > "$STUB_MINISIGN_LOG"; : > "$STUB_SECURITY_LOG"
echo "== k2u_sign u1: raw and b64, same format as the tauri signer =="
printf 'daemon bytes\n' > "$tmp/k2-daemon-macos-aarch64"
if lib k2u_sign u1 "$tmp/k2-daemon-macos-aarch64" k2-daemon-macos-aarch64 9.9.9 "$tmp/raw.sig" raw 2>"$tmp/err"; then
    pass "raw signature written"
else
    fail "raw sign" "$(cat "$tmp/err")"
fi
python3 -I - "$tmp/raw.sig" "$U1_ID" <<'PY' && pass "raw: 4 lines, tauri untrusted comment, timestamp+file trusted comment, ED, key id" || fail "raw format" "$(cat "$tmp/raw.sig")"
import base64, re, sys
lines = open(sys.argv[1]).read().split("\n")
assert lines[0] == "untrusted comment: signature from tauri secret key", lines[0]
raw = base64.b64decode(lines[1])
assert raw[:2] == b"ED", raw[:2]
assert raw[2:10][::-1].hex().upper() == sys.argv[2]
assert re.fullmatch(r"trusted comment: timestamp:\d+\tfile:k2-daemon-macos-aarch64", lines[2]), lines[2]
assert lines[4] == "" and len(lines) == 5
PY
out="$(minisign -V -P "$U1_PUB_LINE" -m "$tmp/k2-daemon-macos-aarch64" -x "$tmp/raw.sig" -Q)" \
    && pass "raw verifies with the source tree's key" || fail "raw verify" "$out"
printf 'app bundle\n' > "$tmp/K2.app.tar.gz"
lib k2u_sign u1 "$tmp/K2.app.tar.gz" K2.app.tar.gz 9.9.9 "$tmp/app.sig" b64 2>"$tmp/err" || fail "b64 sign" "$(cat "$tmp/err")"
python3 -I - "$tmp/app.sig" <<'PY' && pass "b64: one base64 line, no newline, decodes to the minisig text" || fail "b64 format" "$(cat "$tmp/app.sig")"
import base64, sys
b = open(sys.argv[1], "rb").read()
assert b"\n" not in b
t = base64.b64decode(b).decode()
assert t.startswith("untrusted comment: signature from tauri secret key\n")
assert "\tfile:K2.app.tar.gz\n" in t
PY
base64_dec() { python3 -I -c 'import base64,sys; sys.stdout.write(base64.b64decode(open(sys.argv[1]).read()).decode())' "$1"; }
base64_dec "$tmp/app.sig" > "$tmp/app.raw"
minisign -V -P "$U1_PUB_LINE" -m "$tmp/K2.app.tar.gz" -x "$tmp/app.raw" -q && pass "b64 verifies once unwrapped" || fail "b64 verify" ""

echo "== k2u_sign u2: version in the signed comment, checked like a 0.46.6 install =="
lib k2u_sign u2 "$tmp/k2-daemon-macos-aarch64" k2-daemon-macos-aarch64 9.9.9 "$tmp/raw.sig2" raw 2>"$tmp/err" \
    && pass "U2 signature written" || fail "u2 sign" "$(cat "$tmp/err")"
python3 -I - "$tmp/raw.sig2" "$U2_ID" <<'PY' && pass "U2: ED, key id U2, trusted comment timestamp + file + version" || fail "u2 format" "$(cat "$tmp/raw.sig2")"
import base64, re, sys
lines = open(sys.argv[1]).read().split("\n")
raw = base64.b64decode(lines[1])
assert raw[:2] == b"ED" and raw[2:10][::-1].hex().upper() == sys.argv[2]
assert re.fullmatch(r"trusted comment: timestamp:\d+\tfile:k2-daemon-macos-aarch64\tversion:9\.9\.9", lines[2]), lines[2]
PY
lib k2u_verify_u2 "$tmp/k2-daemon-macos-aarch64" "$tmp/raw.sig2" k2-daemon-macos-aarch64 9.9.9 2>/dev/null && pass "k2u_verify_u2 accepts it" || fail "verify_u2" ""
if lib k2u_verify_u2 "$tmp/k2-daemon-macos-aarch64" "$tmp/raw.sig2" k2-daemon-macos-aarch64 9.9.8 2>/dev/null; then fail "verify_u2 version" "accepted 9.9.8"; else pass "k2u_verify_u2 refuses another version"; fi
if lib k2u_verify_u2 "$tmp/k2-daemon-macos-aarch64" "$tmp/raw.sig2" k2-daemon-linux-x86_64 9.9.9 2>/dev/null; then fail "verify_u2 file" "accepted"; else pass "k2u_verify_u2 refuses another file"; fi
if lib k2u_verify_u2 "$tmp/k2-daemon-macos-aarch64" "$tmp/raw.sig" k2-daemon-macos-aarch64 9.9.9 2>/dev/null; then fail "verify_u2 U1" "a U1 signature passed"; else pass "k2u_verify_u2 refuses a U1 signature"; fi
python3 -I -c 'import base64,sys; sys.stdout.write(base64.b64encode(open(sys.argv[1],"rb").read()).decode())' "$tmp/raw.sig2" > "$tmp/wrapped.sig2"
lib k2u_verify_u2 "$tmp/k2-daemon-macos-aarch64" "$tmp/wrapped.sig2" k2-daemon-macos-aarch64 9.9.9 2>/dev/null && pass "k2u_verify_u2 reads the base64-wrapped form" || fail "verify_u2 wrapped" ""
[ "$(lib k2u_slot_key_id u2)" = "$U2_ID" ] && pass "k2u_slot_key_id u2 = the source tree's primary" || fail "slot key id" ""

echo "== k2u_verify_u1: the bridge half fails closed too (installs on 0.46.5 or older) =="
lib k2u_verify_u1 "$tmp/k2-daemon-macos-aarch64" "$tmp/raw.sig" k2-daemon-macos-aarch64 2>"$tmp/err" \
    && pass "k2u_verify_u1 accepts the raw U1 signature" || fail "verify_u1 raw" "$(cat "$tmp/err")"
lib k2u_verify_u1 "$tmp/K2.app.tar.gz" "$tmp/app.sig" K2.app.tar.gz 2>"$tmp/err" \
    && pass "k2u_verify_u1 accepts the base64-wrapped app signature (latest.json form)" || fail "verify_u1 b64" "$(cat "$tmp/err")"
u1_refuses() { # <label> <expect> <file> <sig> <asset>
    if lib k2u_verify_u1 "$3" "$4" "$5" 2>"$tmp/err"; then
        fail "verify_u1 $1" "accepted"
    elif grep -q -- "$2" "$tmp/err"; then
        pass "k2u_verify_u1 refuses $1"
    else
        fail "verify_u1 $1" "$(cat "$tmp/err")"
    fi
}
u1_refuses "a missing signature" "no U1 signature" "$tmp/k2-daemon-macos-aarch64" "$tmp/does-not-exist.sig" k2-daemon-macos-aarch64
: > "$tmp/empty.sig"
u1_refuses "an empty signature (the old Step 8 fallback)" "no U1 signature" "$tmp/k2-daemon-macos-aarch64" "$tmp/empty.sig" k2-daemon-macos-aarch64
u1_refuses "a U2 signature in the U1 slot" "does not verify with LEGACY" "$tmp/k2-daemon-macos-aarch64" "$tmp/raw.sig2" k2-daemon-macos-aarch64
u1_refuses "another file's name" "does not name exactly file:" "$tmp/k2-daemon-macos-aarch64" "$tmp/raw.sig" k2-daemon-linux-x86_64
cp "$tmp/k2-daemon-macos-aarch64" "$tmp/changed-daemon"; printf 'x' >> "$tmp/changed-daemon"
u1_refuses "changed bytes" "does not verify with LEGACY" "$tmp/changed-daemon" "$tmp/raw.sig" k2-daemon-macos-aarch64
# The test's own signing calls read the key file directly, so they log apart
# from the leak check below (which audits only the helper's calls).
printf '%s\n' "$PW" | STUB_MINISIGN_LOG="$tmp/test-own-minisign.log" minisign -S -l -s "$tmp/gen/u1.key" -m "$tmp/k2-daemon-macos-aarch64" -x "$tmp/legacy-u1.sig" -t "timestamp:1"$'\t'"file:k2-daemon-macos-aarch64" >/dev/null
u1_refuses "a legacy (non-prehashed) U1 signature (0.46.4 daemons refuse it)" "not prehashed" "$tmp/k2-daemon-macos-aarch64" "$tmp/legacy-u1.sig" k2-daemon-macos-aarch64
printf '%s\n' "$PW" | STUB_MINISIGN_LOG="$tmp/test-own-minisign.log" minisign -S -s "$tmp/gen/u1.key" -m "$tmp/k2-daemon-macos-aarch64" -x "$tmp/ver-u1.sig" -t "timestamp:1"$'\t'"file:k2-daemon-macos-aarch64"$'\t'"version:9.9.1" >/dev/null
u1_refuses "a U1 comment with a version: field (old Tauri apps compare it)" "and no version:" "$tmp/k2-daemon-macos-aarch64" "$tmp/ver-u1.sig" k2-daemon-macos-aarch64

echo "== the audit log has one line per signature =="
tsv="$K2_RELEASE_AUDIT_DIR/9.9.9/signatures.tsv"
python3 -I - "$tsv" "$tmp/k2-daemon-macos-aarch64" "$U1_ID" "${USER:-$(id -un)}" <<'PY' && pass "signatures.tsv: header + asset, sha256, key id, comment (\\t), user" || fail "audit" "$(cat "$tsv")"
import hashlib, sys
rows = [l.split("\t") for l in open(sys.argv[1]).read().splitlines()]
assert rows[0] == ["time", "asset", "sha256", "key_id", "trusted_comment", "user"], rows[0]
assert len(rows) == 4, rows
r = rows[1]
assert r[1] == "k2-daemon-macos-aarch64"
assert r[2] == hashlib.sha256(open(sys.argv[2], "rb").read()).hexdigest()
assert r[3] == sys.argv[3]
assert r[4].startswith("timestamp:") and r[4].endswith("\\tfile:k2-daemon-macos-aarch64"), r[4]
assert r[5] == sys.argv[4]
assert rows[2][1] == "K2.app.tar.gz"
assert rows[3][1] == "k2-daemon-macos-aarch64" and rows[3][4].endswith("\\tversion:9.9.9"), rows[3]
PY

echo "== the secret never reaches argv or an environment (A7) =="
python3 -I - "$STUB_MINISIGN_LOG" "$STUB_SECURITY_LOG" "$PW" "$KEY_B64_FILE_CONTENT" "$KEY_INNER_LINE" "$HOME/.k2-updater/k2-updater-u1.key" "$PW2" "$KEY2_INNER_LINE" "$HOME/.k2-updater/k2-updater-u2.key" <<'PY' && pass "password and key: on no argv, in no env; -s is a /dev/fd pipe, never the key file or a temp copy" || fail "leak" "see logs in the test tmp"
import json, sys
mlog, slog, pw, keyfile_b64, key_line, key_path, pw2, key2_line, key2_path = sys.argv[1:10]
calls = [json.loads(l) for l in open(mlog) if l.strip()]
signs = [c for c in calls if "-S" in c["argv"]]
assert signs, "no minisign -S call logged"
for c in calls:
    blob = json.dumps(c)
    for secret in (pw, keyfile_b64, key_line, pw2, key2_line):
        assert secret not in blob, "secret in minisign argv/env: %s" % c["argv"]
    for k in c["env"]:
        assert not k.startswith("TAURI_SIGNING"), k
for c in signs:
    s = c["argv"][c["argv"].index("-s") + 1]
    assert s.startswith("/dev/fd/"), s
    assert key_path not in json.dumps(c["argv"]) and key2_path not in json.dumps(c["argv"])
sec = open(slog).read()
assert pw not in sec and pw2 not in sec
PY

echo "== refusals: wrong password, wrong key, exported key, old minisign =="
printf 'wrong' > "$STUB_KEYCHAIN/k2-updater-signing-u1"
rm -f "$tmp/bad.sig"
if lib k2u_sign u1 "$tmp/K2.app.tar.gz" K2.app.tar.gz 9.9.8 "$tmp/bad.sig" b64 2>"$tmp/err"; then
    fail "wrong password" "signed anyway"
elif [ -e "$tmp/bad.sig" ] || [ -e "$K2_RELEASE_AUDIT_DIR/9.9.8/signatures.tsv" ]; then
    fail "wrong password" "left a .sig or an audit line"
else
    pass "wrong keychain password: refused, no .sig, no audit line"
fi
printf '%s' "$PW" > "$STUB_KEYCHAIN/k2-updater-signing-u1"
printf '%s\n' other | minisign -G -p "$tmp/gen/other.pub" -s "$tmp/gen/other.key" >/dev/null
cp "$tmp/gen/other.key" "$tmp/other-plain.key"
printf '%s' other > "$STUB_KEYCHAIN/other-svc"
if K2_UPDATER_U1_KEY="$tmp/other-plain.key" K2_UPDATER_U1_KEYCHAIN=other-svc lib k2u_sign u1 "$tmp/K2.app.tar.gz" K2.app.tar.gz 9.9.8 "$tmp/bad.sig" b64 2>"$tmp/err"; then
    fail "wrong key" "signed with a key the source tree doesn't trust"
else
    grep -q "does not verify" "$tmp/err" && [ ! -e "$tmp/bad.sig" ] && pass "a key the source tree doesn't trust: refused (a plain minisign key file is read too)" || fail "wrong key" "$(cat "$tmp/err")"
fi
if TAURI_SIGNING_PRIVATE_KEY=x lib k2u_env_guard 2>"$tmp/err"; then fail "env guard" "passed with the key exported"; else pass "k2u_env_guard refuses TAURI_SIGNING_PRIVATE_KEY"; fi
if TAURI_SIGNING_PRIVATE_KEY_PASSWORD=x lib k2u_env_guard 2>"$tmp/err"; then fail "env guard pw" "passed"; else pass "k2u_env_guard refuses TAURI_SIGNING_PRIVATE_KEY_PASSWORD"; fi
lib k2u_env_guard && pass "k2u_env_guard passes a clean env" || fail "env guard clean" ""

echo "== k2u_preflight =="
lib k2u_preflight 9.9.7 2>"$tmp/err" && pass "preflight passes (U1 and U2 self-test signatures verify)" || fail "preflight" "$(cat "$tmp/err")"
[ "$(grep -c $'\tk2-preflight-self-test\t' "$K2_RELEASE_AUDIT_DIR/9.9.7/signatures.tsv")" = 2 ] && pass "both self-tests are logged" || fail "self-test count" "$(cat "$K2_RELEASE_AUDIT_DIR/9.9.7/signatures.tsv")"
if K2_UPDATER_SRC_ROOT="$ROOT" lib k2u_preflight 9.9.7 2>"$tmp/err"; then
    grep -q 'role: "primary"' "$ROOT/crates/k2-release-keys/src/lib.rs" && pass "(real tree already has keys)" || fail "empty UPDATER_KEYS" "preflight passed"
else
    grep -qE "ceremony|empty|does not verify|primary" "$tmp/err" && pass "a tree whose UPDATER_KEYS don't hold this U2 (pre-ceremony main) is refused" || fail "empty keys" "$(cat "$tmp/err")"
fi
mv "$STUB_KEYCHAIN/k2-updater-signing-u2" "$tmp/kc2.bak"
if lib k2u_preflight 9.9.7 2>"$tmp/err"; then fail "no u2 keychain" "passed"; else grep -q "k2-updater-signing-u2" "$tmp/err" && pass "no U2 keychain item: refused" || fail "u2 keychain" "$(cat "$tmp/err")"; fi
mv "$tmp/kc2.bak" "$STUB_KEYCHAIN/k2-updater-signing-u2"
if STUB_MINISIGN_VERSION=0.9 lib k2u_preflight 9.9.7 2>"$tmp/err"; then fail "old minisign" "passed"; else grep -q "too old" "$tmp/err" && pass "minisign < 0.11 refused" || fail "old minisign" "$(cat "$tmp/err")"; fi
chmod 644 "$HOME/.k2-updater/k2-updater-u1.key"
if lib k2u_preflight 9.9.7 2>"$tmp/err"; then fail "key mode" "passed with 644"; else grep -q "mode 600" "$tmp/err" && pass "key file not 600: refused" || fail "key mode" "$(cat "$tmp/err")"; fi
chmod 600 "$HOME/.k2-updater/k2-updater-u1.key"; chmod 755 "$HOME/.k2-updater"
if lib k2u_preflight 9.9.7 2>"$tmp/err"; then fail "dir mode" "passed with 755"; else grep -q "mode 700" "$tmp/err" && pass "key dir not 700: refused" || fail "dir mode" "$(cat "$tmp/err")"; fi
chmod 700 "$HOME/.k2-updater"
mv "$STUB_KEYCHAIN/k2-updater-signing-u1" "$tmp/kc.bak"
if lib k2u_preflight 9.9.7 2>"$tmp/err"; then fail "no keychain item" "passed"; else grep -q "no keychain item" "$tmp/err" && pass "no keychain item: refused" || fail "keychain" "$(cat "$tmp/err")"; fi
mv "$tmp/kc.bak" "$STUB_KEYCHAIN/k2-updater-signing-u1"
if TAURI_SIGNING_PRIVATE_KEY=x lib k2u_preflight 9.9.7 2>/dev/null; then fail "preflight env" "passed"; else pass "preflight refuses an exported key"; fi

# ── fake Linux daemons ──
export K2_GMAIL_CLIENT_ID="test-client-1234.apps.googleusercontent.com" K2_GMAIL_CLIENT_SECRET="GOCSPX-test-secret-5678" K2_REQUIRE_MICROSOFT_OAUTH=0
unset K2_MICROSOFT_CLIENT_ID || true
mkdaemon() { # <out> <arch> <version> [flags…]: no-marker two-markers no-ptyd testkeys no-oauth
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
if "no-marker" not in flags:
    body += b"\0" + marker
if "two-markers" in flags:
    body += b"\0" + marker
if "testkeys" in flags:
    body += b"\0K2_SEW_TEST_PINS_ENABLED"
if "no-oauth" not in flags:
    body += b"\0" + os.environ["K2_GMAIL_CLIENT_ID"].encode() + b"\0" + os.environ["K2_GMAIL_CLIENT_SECRET"].encode()
open(out, "wb").write(body)
PY
    python3 -I -c 'import hashlib,sys; print("%s  %s" % (hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest(), sys.argv[1].rsplit("/",1)[-1]))' "$1" > "$1.sha256"
}
V=9.9.9
good="$tmp/good"; mkdir -p "$good"
for a in x86_64 aarch64; do
    mkdaemon "$good/k2-daemon-linux-$a" "$a" "$V"
    printf 'engine-%s' "$a" > "$good/k2-app-engine-linux-$a"
done
cat > "$tmp/WHATS_NEW.md" <<'MD'
## 9.9.9 — Test
- a line
MD
ARTS='    "macos-aarch64": {
      "url": "https://github.com/Alakazam-211/K2/releases/download/v9.9.9/k2-daemon-macos-aarch64",
      "sig": "https://github.com/Alakazam-211/K2/releases/download/v9.9.9/k2-daemon-macos-aarch64.sig",
      "sha256": "abc"
    }'
bash -c "source '$ROOT/scripts/daemon-manifest.sh'; k2_daemon_manifest $V 2026-10-24T00:00:00Z '$ARTS' '$tmp/WHATS_NEW.md'" > "$tmp/daemon-latest.json"

echo "== check-daemon-asset.py =="
python3 -I "$ROOT/scripts/lib/check-daemon-asset.py" "$good/k2-daemon-linux-x86_64" x86_64 "$V" >/dev/null && pass "a good x86_64 daemon passes" || fail "good asset" ""

# What compute_release.py writes after checking a job's signed receipt.
mkverdict() { # <dist dir> <state dir> [ok=true]
    mkdir -p "$2"
    python3 -I - "$1" "$2" "${3:-true}" <<'PY'
import hashlib, json, os, sys
d, out, ok = sys.argv[1:4]
files = {"dist/" + n: {"sha256": hashlib.sha256(open(os.path.join(d, n), "rb").read()).hexdigest(), "bytes": 1}
         for n in sorted(os.listdir(d)) if n.startswith("k2-daemon-linux-") and not n.endswith(".sha256")}
json.dump({"label": "L1", "ok": ok == "true", "commit": "c" * 40, "files": files, "problems": [] if ok == "true" else ["x"]},
          open(os.path.join(out, "L1.verdict.json"), "w"))
PY
}
mkverdict "$good" "$tmp/verdict-good"

echo "== sign-daemon.sh --dry: good builds =="
out="$tmp/out-good"
if bash "$ROOT/scripts/sign-daemon.sh" "$V" --dry --from-dir "$good" --compute-verdict "$tmp/verdict-good" --manifest "$tmp/daemon-latest.json" --out "$out" >"$tmp/sd.log" 2>&1; then
    pass "signs both arches and merges the manifest"
else
    fail "sign-daemon dry" "$(tail -20 "$tmp/sd.log")"
fi
for a in x86_64 aarch64; do
    lib k2u_verify_u2 "$good/k2-daemon-linux-$a" "$out/k2-daemon-linux-$a.sig2" "k2-daemon-linux-$a" "$V" 2>/dev/null \
        && pass "linux-$a .sig2: U2, file + version, as 0.46.6 installs check" || fail "linux-$a sig2" ""
    o="$(minisign -V -P "$U1_PUB_LINE" -m "$good/k2-daemon-linux-$a" -x "$out/k2-daemon-linux-$a.sig" -Q 2>&1)" \
        && [[ "$o" == *$'\tfile:k2-daemon-linux-'"$a" ]] && head -1 "$out/k2-daemon-linux-$a.sig" | grep -qx 'untrusted comment: signature from tauri secret key' \
        && pass "linux-$a .sig: raw minisig, verifies, file:k2-daemon-linux-$a" || fail "linux-$a sig" "$o"
done
# Byte-compatible: exactly what CI's old merge job wrote for the same inputs.
cp "$tmp/daemon-latest.json" "$tmp/ci-merge.json"
python3 -I "$ROOT/scripts/merge-daemon-manifest.py" "$tmp/ci-merge.json" "https://github.com/Alakazam-211/K2/releases/download/v$V" "$good" >/dev/null
cmp -s "$tmp/ci-merge.json" "$out/daemon-latest.json" && pass "daemon-latest.json is exactly what merge-daemon-manifest.py writes (the script CI's merge job ran)" || fail "manifest bytes" "$(diff "$tmp/ci-merge.json" "$out/daemon-latest.json" | head)"
python3 -I - "$out/daemon-latest.json" "$V" <<'PY' && pass "linux entries keep url, sig, sha256 (0.46.3's fields, same values) and add sig2" || fail "manifest fields" "$(cat "$out/daemon-latest.json")"
import json, sys
m = json.load(open(sys.argv[1]))
for a in ("x86_64", "aarch64"):
    e = m["artifacts"]["linux-" + a]
    assert list(e) == ["url", "sig", "sha256", "sig2"], e
    base = "https://github.com/Alakazam-211/K2/releases/download/v%s/k2-daemon-linux-%s" % (sys.argv[2], a)
    assert e["url"] == base and e["sig"] == base + ".sig" and e["sig2"] == base + ".sig2"
assert sorted(m) == ["artifacts", "notes", "pub_date", "version"], sorted(m)
PY
sed -n '/^manifest_field() {/,/^}/p' "$SCRIPT_DIR/fixtures/install-daemon-0.46.3-manifest_field.sh" > "$tmp/p0463.sh"
got="$(bash -c "source '$tmp/p0463.sh'; manifest_field \"\$(cat '$out/daemon-latest.json')\" linux-x86_64 sha256")"
want="$(awk '{print $1}' "$good/k2-daemon-linux-x86_64.sha256")"
[ "$got" = "$want" ] && pass "the 0.46.3 installer parser reads the new manifest" || fail "0.46.3 parser" "$got vs $want"

echo "== sign-daemon.sh --dry: refusals (nothing signed) =="
refuse() { # <label> <expect-in-output> <setup cmd…>
    local label="$1" expect="$2"; shift 2
    local d="$tmp/bad-$label" o="$tmp/out-$label"
    rm -rf "$d" "$o" "$d.verdict"; cp -R "$good" "$d"
    "$@" "$d"
    # The verdict follows the (changed) bytes, so the byte checks get a turn.
    mkverdict "$d" "$d.verdict"
    if bash "$ROOT/scripts/sign-daemon.sh" "$V" --dry --from-dir "$d" --compute-verdict "$d.verdict" --manifest "$tmp/daemon-latest.json" --out "$o" >"$tmp/r.log" 2>&1; then
        fail "$label" "signed anyway"
    elif ! grep -q -- "$expect" "$tmp/r.log"; then
        fail "$label" "wrong reason: $(tail -5 "$tmp/r.log")"
    elif ls "$o"/*.sig >/dev/null 2>&1; then
        fail "$label" "left a .sig in --out"
    else
        pass "refused: $label"
    fi
}
t_sha() { printf 'x' >> "$1/k2-daemon-linux-x86_64"; }
t_arch() { mkdaemon "$1/k2-daemon-linux-x86_64" aarch64 "$V"; }
t_two() { mkdaemon "$1/k2-daemon-linux-x86_64" x86_64 "$V" two-markers; }
t_none() { mkdaemon "$1/k2-daemon-linux-x86_64" x86_64 "$V" no-marker; }
t_ver() { mkdaemon "$1/k2-daemon-linux-x86_64" x86_64 9.9.8; }
t_test() { mkdaemon "$1/k2-daemon-linux-aarch64" aarch64 "$V" testkeys; }
t_ptyd() { mkdaemon "$1/k2-daemon-linux-x86_64" x86_64 "$V" no-ptyd; }
t_oauth() { mkdaemon "$1/k2-daemon-linux-x86_64" x86_64 "$V" no-oauth; }
t_missing() { rm "$1/k2-daemon-linux-aarch64.sha256"; }
refuse wrong-sha "sha256 differs" t_sha
refuse wrong-arch "ELF machine 183" t_arch
refuse two-markers "exactly one K2_DAEMON_BUILD marker, found 2" t_two
refuse no-marker "exactly one K2_DAEMON_BUILD marker, found 0" t_none
refuse old-version "built as v9.9.8" t_ver
refuse test-pins "K2_SEW_TEST_PINS_ENABLED" t_test
refuse no-ptyd "does not embed k2-ptyd" t_ptyd
refuse placeholder-oauth "OAuth bake check failed" t_oauth
refuse missing-sha "not in" t_missing
if bash "$ROOT/scripts/sign-daemon.sh" "$V" --dry --from-dir "$good" --compute-verdict "$tmp/verdict-good" --out "$tmp/o2" >/dev/null 2>&1; then fail "dry needs manifest" "ran"; else pass "--dry without --manifest refused (never falls back to GitHub)"; fi

echo "== compute provenance: no ok verdict for these bytes, no signature =="
prov_refuses() { # <label> <expect> <args…>
    local label="$1" expect="$2"; shift 2
    rm -rf "$tmp/o3"
    if bash "$ROOT/scripts/sign-daemon.sh" "$V" --dry --from-dir "$good" --manifest "$tmp/daemon-latest.json" --out "$tmp/o3" "$@" >"$tmp/r.log" 2>&1; then
        fail "provenance $label" "signed anyway"
    elif grep -q -- "$expect" "$tmp/r.log" && ! ls "$tmp/o3"/*.sig >/dev/null 2>&1; then
        pass "refused: $label"
    else
        fail "provenance $label" "$(tail -3 "$tmp/r.log")"
    fi
}
prov_refuses "--from-dir without --compute-verdict" "needs --compute-verdict"
mkverdict "$good" "$tmp/verdict-bad" false
prov_refuses "a verdict that isn't ok" "no ok compute verdict" --compute-verdict "$tmp/verdict-bad"
mkdir -p "$tmp/other-dist"; cp "$good"/k2-daemon-linux-* "$tmp/other-dist/"; printf 'x' >> "$tmp/other-dist/k2-daemon-linux-aarch64"
mkverdict "$tmp/other-dist" "$tmp/verdict-other"
prov_refuses "a verdict for other bytes" "no ok compute verdict" --compute-verdict "$tmp/verdict-other"

echo "== sign-daemon.sh: the GitHub path (stub gh) =="
# The release commit comes from the LOCAL tag (a fixture checkout here).
srcrepo="$tmp/srcrepo"; mkdir -p "$srcrepo"
git -C "$srcrepo" init -q
git -C "$srcrepo" -c user.email=t@example.invalid -c user.name=t commit -q --allow-empty -m release
git -C "$srcrepo" tag "v$V"
export K2_SIGN_SOURCE_DIR="$srcrepo"
export STUB_TAG_COMMIT="$(git -C "$srcrepo" rev-parse "refs/tags/v$V^{commit}")"
export STUB_ATTESTED="$tmp/attested"
for a in x86_64 aarch64; do awk '{print $1}' "$good/k2-daemon-linux-$a.sha256"; done > "$STUB_ATTESTED"
fresh_rel() {
    rm -rf "$STUB_REL/v$V"; mkdir -p "$STUB_REL/v$V"
    cp "$good"/k2-daemon-linux-* "$good"/k2-app-engine-linux-* "$STUB_REL/v$V/"
    cp "$tmp/daemon-latest.json" "$STUB_REL/v$V/daemon-latest.json"
    : > "$STUB_GH_LOG"
}
gh_refuses() { # <label> <expect> [env…]
    local label="$1" expect="$2"; shift 2
    fresh_rel
    if env "$@" K2_SIGN_WAIT_MIN=1 bash "$ROOT/scripts/sign-daemon.sh" "$V" >"$tmp/gh.out" 2>&1; then
        fail "github $label" "signed"
    elif ! grep -q -- "$expect" "$tmp/gh.out"; then
        fail "github $label" "wrong reason: $(tail -4 "$tmp/gh.out")"
    elif grep -q '^UPLOAD .*\.sig' "$STUB_GH_LOG"; then
        fail "github $label" "uploaded a signature"
    else
        pass "refused: $label (no signature uploaded)"
    fi
}
gh_refuses "GitHub's tag moved off the local tag's commit" "the tag moved" STUB_TAG_COMMIT="$(printf 'c%.0s' $(seq 1 40))"
mkdir -p "$tmp/notags"; git -C "$tmp/notags" init -q
gh_refuses "no local release tag" "no local tag v$V" K2_SIGN_SOURCE_DIR="$tmp/notags"
gh_refuses "no build-provenance attestation" "no valid build-provenance attestation" STUB_ATTESTED=/dev/null
gh_refuses "an attestation made at another commit" "no valid build-provenance attestation" STUB_ATTESTED_COMMIT="$(printf 'b%.0s' $(seq 1 40))"
gh_refuses "an asset uploaded by a person" "uploaded by 'mallory', not github-actions" STUB_UPLOADER=mallory
gh_refuses "an asset uploaded before the release existed" "before the release" STUB_ASSET_TIME=2026-10-24T10:00:00Z
fresh_rel
if STUB_SWAP_AFTER_MANIFEST=k2-daemon-linux-x86_64 K2_SIGN_WAIT_MIN=1 bash "$ROOT/scripts/sign-daemon.sh" "$V" >"$tmp/gh.out" 2>&1; then
    fail "post-publish swap" "not caught"
else
    grep -q "is not the binary that was signed" "$tmp/gh.out" && pass "a daemon swapped after signing is caught by the post-publish check" || fail "post-publish swap" "$(tail -4 "$tmp/gh.out")"
fi
fresh_rel
rel="$STUB_REL/v$V"
if K2_SIGN_WAIT_MIN=1 bash "$ROOT/scripts/sign-daemon.sh" "$V" >"$tmp/gh.out" 2>&1; then pass "signs from the release and uploads"; else fail "github path" "$(tail -20 "$tmp/gh.out")"; fi
grep -q "provenance ok (attested by daemon-binaries.yml" "$tmp/gh.out" && pass "both daemons passed the provenance checks" || fail "provenance log" "$(grep provenance "$tmp/gh.out")"
python3 -I - "$STUB_GH_LOG" <<'PY' && pass "uploads the .sig and .sig2 files before daemon-latest.json" || fail "upload order" "$(grep UPLOAD "$STUB_GH_LOG")"
import sys
ups = [l.split()[1] for l in open(sys.argv[1]) if l.startswith("UPLOAD ")]
assert set(ups) == {"k2-daemon-linux-x86_64.sig", "k2-daemon-linux-aarch64.sig", "k2-daemon-linux-x86_64.sig2", "k2-daemon-linux-aarch64.sig2", "daemon-latest.json"}, ups
assert ups[-1] == "daemon-latest.json", ups
PY
python3 -I -c 'import json,sys; a=json.load(open(sys.argv[1]))["artifacts"]; sys.exit(0 if {"macos-aarch64","linux-x86_64","linux-aarch64","k2-app-engine-linux-x86_64","k2-app-engine-linux-aarch64"} == set(a) else 1)' "$rel/daemon-latest.json" \
    && pass "the released manifest lists macOS, both Linux daemons and both engines" || fail "released manifest" "$(cat "$rel/daemon-latest.json")"

echo "== sign-daemon.sh --preflight: the GitHub secrets, and gh can check provenance =="
if STUB_GH_OLD=1 bash "$ROOT/scripts/sign-daemon.sh" --preflight "$V" >"$tmp/p.out" 2>&1; then
    fail "old gh" "preflight passed"
else
    grep -qE "has no --source-(ref|digest)" "$tmp/p.out" && pass "preflight refuses a gh whose attestation verify lacks --source-ref/--source-digest" || fail "old gh" "$(cat "$tmp/p.out")"
fi
grep -qF -- '--cert-identity "https://github.com/$REPO/.github/workflows/daemon-binaries.yml@refs/tags/$TAG"' "$ROOT/scripts/sign-daemon.sh" \
    && ! grep -q -- '--signer-workflow' "$ROOT/scripts/sign-daemon.sh" \
    && pass "sign-daemon.sh pins the exact certificate identity (no prefix-matching --signer-workflow)" || fail "cert identity" ""
STUB_SECRETS="K2_GMAIL_CLIENT_ID" bash "$ROOT/scripts/sign-daemon.sh" --preflight "$V" >/dev/null 2>&1 && pass "preflight passes with no TAURI_SIGNING_* secret on GitHub" || fail "preflight clean" ""
if STUB_SECRETS="TAURI_SIGNING_PRIVATE_KEY K2_GMAIL_CLIENT_ID" bash "$ROOT/scripts/sign-daemon.sh" --preflight "$V" >"$tmp/p.out" 2>&1; then
    fail "secret present" "preflight passed"
else
    grep -q "gh secret delete TAURI_SIGNING_PRIVATE_KEY" "$tmp/p.out" && pass "a leftover TAURI_SIGNING_PRIVATE_KEY secret fails the preflight, with the delete command" || fail "secret present" "$(cat "$tmp/p.out")"
fi
STUB_SECRETS="TAURI_SIGNING_PRIVATE_KEY_PASSWORD" K2_ALLOW_GH_SIGNING_SECRETS=1 bash "$ROOT/scripts/sign-daemon.sh" --preflight "$V" >"$tmp/p.out" 2>&1 \
    && grep -q "WARNING" "$tmp/p.out" && pass "K2_ALLOW_GH_SIGNING_SECRETS=1: warns loudly, passes" || fail "allow secrets" "$(cat "$tmp/p.out")"
STUB_SECRETS_FAIL=1 bash "$ROOT/scripts/sign-daemon.sh" --preflight "$V" >"$tmp/p.out" 2>&1 \
    && grep -q "can't list" "$tmp/p.out" && pass "a token that can't read secrets: warns, passes" || fail "secrets unreadable" "$(cat "$tmp/p.out")"

echo "== windows-patch-latest-json.sh signs through the helper =="
printf 'setup bytes' > "$tmp/K2_${V}_x64-setup.exe"
mkdir -p "$STUB_REL/v$V"
printf '{"version":"%s","platforms":{"darwin-aarch64":{"signature":"x","url":"u"}}}' "$V" > "$tmp/latest.json"
export STUB_CURL_FILE="$tmp/latest.json"
wroot="$tmp/wroot"; mkdir -p "$wroot/scripts/lib"
cp "$ROOT/scripts/windows-patch-latest-json.sh" "$wroot/scripts/"
cp "$LIB" "$wroot/scripts/lib/"
if bash "$wroot/scripts/windows-patch-latest-json.sh" "$V" "$tmp/K2_${V}_x64-setup.exe" >"$tmp/w.out" 2>&1; then
    python3 -I - "$STUB_REL/v$V/latest.json" > "$tmp/w.sig" <<'PY'
import base64, json, sys
print(base64.b64decode(json.load(open(sys.argv[1]))["platforms"]["windows-x86_64"]["signature"]).decode(), end="")
PY
    cp "$tmp/K2_${V}_x64-setup.exe" "$tmp/setup.exe"
    o="$(minisign -V -P "$U1_PUB_LINE" -m "$tmp/setup.exe" -x "$tmp/w.sig" -Q)" && [[ "$o" == *$'\tfile:setup.exe' ]] \
        && pass "latest.json windows-x86_64 signature verifies (b64, file:setup.exe)" || fail "windows sig" "$o"
    python3 -I - "$STUB_REL/v$V/latest.json" "$U2_ID" > "$tmp/w.sig2" <<'PY'
import base64, json, sys
print(base64.b64decode(json.load(open(sys.argv[1]))["platforms"]["windows-x86_64"]["k2_signatures"][sys.argv[2]]).decode(), end="")
PY
    lib k2u_verify_u2 "$tmp/setup.exe" "$tmp/w.sig2" "K2_${V}_x64-setup.exe" "$V" 2>/dev/null \
        && pass "latest.json windows-x86_64 k2_signatures: U2, file = the URL's setup name, version" || fail "windows k2_signatures" "$(cat "$tmp/w.sig2")"
else
    fail "windows patch" "$(tail -10 "$tmp/w.out")"
fi

# The fallback to releases/latest and the version overwrite are gone.
printf '{"version":"9.9.8","platforms":{"darwin-aarch64":{"signature":"x","url":"u"}}}' > "$tmp/latest-old.json"
if STUB_CURL_FILE="$tmp/latest-old.json" bash "$wroot/scripts/windows-patch-latest-json.sh" "$V" "$tmp/K2_${V}_x64-setup.exe" >"$tmp/w.out" 2>&1; then
    fail "windows: another release's latest.json" "patched it"
else
    grep -q "is for '9.9.8', not $V" "$tmp/w.out" && pass "windows: a latest.json for another version is refused, not re-stamped" || fail "windows version" "$(tail -3 "$tmp/w.out")"
fi
if STUB_CURL_FAIL=1 bash "$wroot/scripts/windows-patch-latest-json.sh" "$V" "$tmp/K2_${V}_x64-setup.exe" >"$tmp/w.out" 2>&1; then
    fail "windows: no latest.json" "went on"
else
    grep -q "not falling back" "$tmp/w.out" && pass "windows: no latest.json on the tag is fatal (no releases/latest fallback)" || fail "windows fallback" "$(tail -3 "$tmp/w.out")"
fi
! grep -q 'releases/latest/download/latest.json' "$ROOT/scripts/windows-patch-latest-json.sh" && pass "windows-patch-latest-json.sh never reads the latest alias" || fail "windows alias" ""

echo "== installers: a dual-signed release on old and new installs (SS4) =="
case "$(uname -s)" in Darwin) pos=macos ;; Linux) pos=linux ;; *) pos=unknown ;; esac
case "$(uname -m)" in arm64|aarch64) parch=aarch64 ;; x86_64|amd64) parch=x86_64 ;; *) parch=unknown ;; esac
PLAT="$pos-$parch"; DA="k2-daemon-$PLAT"
inst="$tmp/inst"; mkdir -p "$inst"
printf 'the new daemon for %s\n' "$PLAT" > "$inst/$DA"
lib k2u_sign u1 "$inst/$DA" "$DA" 9.9.9 "$inst/$DA.sig" raw 2>/dev/null
lib k2u_sign u2 "$inst/$DA" "$DA" 9.9.9 "$inst/$DA.sig2" raw 2>/dev/null
mkman() { # <out> <version> <sig2 file, or "" for none>
    python3 -I - "$1" "$2" "$3" "$inst/$DA" "$PLAT" <<'PY'
import hashlib, json, sys
out, version, sig2, binary, plat = sys.argv[1:6]
e = {"url": "file://" + binary, "sig": "file://" + binary + ".sig", "sha256": hashlib.sha256(open(binary, "rb").read()).hexdigest()}
if sig2:
    e["sig2"] = "file://" + sig2
json.dump({"version": version, "pub_date": "", "artifacts": {plat: e}}, open(out, "w"), indent=2)
PY
}
mkman "$inst/dual.json" 9.9.9 "$inst/$DA.sig2"
# 0.46.3's installer: its frozen manifest parser + `minisign -Vm … -p <U1>`.
old_sig="$(bash -c "source '$tmp/p0463.sh'; manifest_field \"\$(cat '$inst/dual.json')\" '$PLAT' sig")"
old_url="$(bash -c "source '$tmp/p0463.sh'; manifest_field \"\$(cat '$inst/dual.json')\" '$PLAT' url")"
if minisign -Vm "${old_url#file://}" -p "$tmp/gen/u1.pub" -x "${old_sig#file://}" >/dev/null 2>&1; then
    pass "an install that trusts only U1 (0.46.3 install-daemon.sh logic) accepts the dual-signed release"
else
    fail "old installer" "sig=$old_sig url=$old_url"
fi
new_install() { # <manifest> [env…]: the fixture tree's install-daemon.sh (trusts U2/U3)
    local m="$1"; shift
    rm -rf "$tmp/ib"
    env K2_MANIFEST_URL="file://$m" K2_BIN_DIR="$tmp/ib" K2_NO_SERVICE=1 K2_APP_ENGINE_PATH="$tmp/no-engine" "$@" \
        sh "$SRC/scripts/install-daemon.sh" >"$tmp/inst.out" 2>&1
}
if new_install "$inst/dual.json" && cmp -s "$tmp/ib/k2-daemon" "$inst/$DA"; then
    pass "an install that trusts only U2 (install-daemon.sh) installs the dual-signed release"
else
    fail "new installer" "$(tail -5 "$tmp/inst.out")"
fi
inst_refuses() { # <label> <expect> <manifest> [env…]
    local label="$1" expect="$2" m="$3"; shift 3
    if new_install "$m" "$@"; then
        fail "install-daemon.sh $label" "installed"
    elif ! grep -q -- "$expect" "$tmp/inst.out"; then
        fail "install-daemon.sh $label" "wrong reason: $(tail -3 "$tmp/inst.out")"
    elif [ -e "$tmp/ib/k2-daemon" ]; then
        fail "install-daemon.sh $label" "left a binary"
    else
        pass "install-daemon.sh refuses $label"
    fi
}
mkman "$inst/u1only.json" 0.46.3 ""
inst_refuses "a U1-only release (no sig2)" "retired update key" "$inst/u1only.json"
mkman "$inst/u1as2.json" 9.9.9 "$inst/$DA.sig"
inst_refuses "a U1 signature in sig2" "VERIFICATION FAILED" "$inst/u1as2.json"
mkman "$inst/ver.json" 9.9.10 "$inst/$DA.sig2"
inst_refuses "a signature for another version" "for version" "$inst/ver.json"
lib k2u_sign u2 "$inst/$DA" k2-root-helper-linux-x86_64 9.9.9 "$inst/other.sig2" raw 2>/dev/null
mkman "$inst/file.json" 9.9.9 "$inst/other.sig2"
inst_refuses "a signature for another file" "is for 'k2-root-helper-linux-x86_64'" "$inst/file.json"
cp "$inst/$DA" "$inst/tampered"; printf 'x' >> "$inst/tampered"
python3 -I - "$inst/dual.json" "$inst/tampered" > "$inst/tamper.json" <<'PY'
import hashlib, json, sys
m = json.load(open(sys.argv[1]))
e = next(iter(m["artifacts"].values()))
e["url"] = "file://" + sys.argv[2]
e["sha256"] = hashlib.sha256(open(sys.argv[2], "rb").read()).hexdigest()
print(json.dumps(m))
PY
inst_refuses "a changed binary" "VERIFICATION FAILED" "$inst/tamper.json"
# A legacy (non-prehashed, "Ed") signature by U2 itself: minisign -V would
# accept it, the installers must not (the daemon doesn't either).
printf '%s\n' "$PW2" | minisign -S -l -s "$tmp/gen/u2.key" -m "$inst/$DA" -x "$inst/legacy.sig2" -t "timestamp:1"$'\t'"file:$DA"$'\t'"version:9.9.9" >/dev/null
minisign -V -P "$U2_PUB_LINE" -m "$inst/$DA" -x "$inst/legacy.sig2" -q && pass "(control: plain minisign -V accepts the legacy signature)" || fail "legacy control" ""
mkman "$inst/legacy.json" 9.9.9 "$inst/legacy.sig2"
inst_refuses "a legacy (non-prehashed) U2 signature" "not a prehashed" "$inst/legacy.json"
rm -rf "$tmp/ib"
if env -u K2_MANIFEST_URL K2_VERSION=0.46.3 K2_BIN_DIR="$tmp/ib" K2_NO_SERVICE=1 sh "$SRC/scripts/install-daemon.sh" >"$tmp/inst.out" 2>&1; then
    fail "install-daemon.sh --version 0.46.3" "ran"
else
    grep -q "Versions before 0.46.6" "$tmp/inst.out" && pass "install-daemon.sh --version 0.46.3: refused before any download" || fail "version refusal" "$(cat "$tmp/inst.out")"
fi
# v0.46.4 shipped signed with U1 only: it is a retired-key release too.
rm -rf "$tmp/ib"
if env -u K2_MANIFEST_URL K2_VERSION=0.46.4 K2_BIN_DIR="$tmp/ib" K2_NO_SERVICE=1 sh "$SRC/scripts/install-daemon.sh" >"$tmp/inst.out" 2>&1; then
    fail "install-daemon.sh --version 0.46.4" "ran"
else
    grep -q "Versions before 0.46.6 are signed with the retired update key" "$tmp/inst.out" && pass "install-daemon.sh --version 0.46.4 (shipped U1-only): refused before any download" || fail "0.46.4 refusal" "$(cat "$tmp/inst.out")"
fi
# v0.46.5 shipped signed with U1 only as well (the cutover is 0.46.6).
rm -rf "$tmp/ib"
if env -u K2_MANIFEST_URL K2_VERSION=0.46.5 K2_BIN_DIR="$tmp/ib" K2_NO_SERVICE=1 sh "$SRC/scripts/install-daemon.sh" >"$tmp/inst.out" 2>&1; then
    fail "install-daemon.sh --version 0.46.5" "ran"
else
    grep -q "Versions before 0.46.6 are signed with the retired update key" "$tmp/inst.out" && pass "install-daemon.sh --version 0.46.5 (shipped U1-only): refused before any download" || fail "0.46.5 refusal" "$(cat "$tmp/inst.out")"
fi
grep -q "(0.46.6 and newer)" <(sh "$SRC/scripts/install-daemon.sh" --help 2>&1) && pass "install-daemon.sh --help names the cutover from K2_FIRST_U2_VERSION" || fail "install help cutover" ""
rm -rf "$tmp/ib"
if env K2_MANIFEST_URL="file://$inst/dual.json" K2_BIN_DIR="$tmp/ib" K2_NO_SERVICE=1 K2_APP_ENGINE_PATH="$tmp/no-engine" sh "$ROOT/scripts/install-daemon.sh" >"$tmp/inst.out" 2>&1; then
    grep -q '^K2_DAEMON_PUBKEYS="RW' "$ROOT/scripts/install-daemon.sh" && pass "(this tree already has keys)" || fail "pre-ceremony installer" "installed with no keys"
else
    grep -qE "trusts no update key|VERIFICATION FAILED" "$tmp/inst.out" && pass "this tree's install-daemon.sh refuses a release its keys didn't sign (pre-ceremony: trusts nothing)" || fail "pre-ceremony installer" "$(tail -3 "$tmp/inst.out")"
fi

cli_help="$(bash "$ROOT/cli/k2" daemon install --help 2>&1 || true)"
grep -qi "an OLDER release than the one installed" "$ROOT/scripts/install-daemon.sh" \
    && [[ "$cli_help" == *"an OLDER release than the one installed"* ]] \
    && pass "install-daemon.sh and k2 daemon install --help say an older 0.46.6+ install is an allowed explicit action" || fail "downgrade help" ""

echo "== k2 daemon install (cli/k2) checks sig2 the same way =="
cli_install() { # <manifest> [extra args…]
    local m="$1"; shift
    rm -rf "$tmp/cb"
    bash "$SRC/cli/k2" daemon install --manifest-url "file://$m" --bin-dir "$tmp/cb" --no-service "$@" >"$tmp/cli.out" 2>&1
}
if cli_install "$inst/dual.json" && cmp -s "$tmp/cb/k2-daemon" "$inst/$DA"; then
    pass "k2 daemon install installs the dual-signed release (trusts only U2/U3)"
else
    fail "cli install" "$(tail -5 "$tmp/cli.out")"
fi
for case_ in "u1only.json:retired update key" "u1as2.json:VERIFICATION FAILED" "ver.json:for version" "file.json:is for 'k2-root-helper" "tamper.json:VERIFICATION FAILED" "legacy.json:not a prehashed"; do
    m="${case_%%:*}"; want="${case_#*:}"
    if cli_install "$inst/$m"; then
        fail "cli $m" "installed"
    elif grep -q -- "$want" "$tmp/cli.out" && [ ! -e "$tmp/cb/k2-daemon" ]; then
        pass "k2 daemon install refuses $m"
    else
        fail "cli $m" "$(tail -3 "$tmp/cli.out")"
    fi
done
rm -rf "$tmp/cb"
if bash "$SRC/cli/k2" daemon install --version 0.46.3 --bin-dir "$tmp/cb" --no-service >"$tmp/cli.out" 2>&1; then
    fail "cli --version 0.46.3" "ran"
else
    grep -q "Versions before 0.46.6" "$tmp/cli.out" && pass "k2 daemon install --version 0.46.3: refused with the retired-key message" || fail "cli version" "$(cat "$tmp/cli.out")"
fi
rm -rf "$tmp/cb"
if bash "$SRC/cli/k2" daemon install --version 0.46.4 --bin-dir "$tmp/cb" --no-service >"$tmp/cli.out" 2>&1; then
    fail "cli --version 0.46.4" "ran"
else
    grep -q "Versions before 0.46.6 are signed with the retired update key" "$tmp/cli.out" && pass "k2 daemon install --version 0.46.4 (shipped U1-only): refused with the retired-key message" || fail "cli 0.46.4" "$(cat "$tmp/cli.out")"
fi
rm -rf "$tmp/cb"
if bash "$SRC/cli/k2" daemon install --version 0.46.5 --bin-dir "$tmp/cb" --no-service >"$tmp/cli.out" 2>&1; then
    fail "cli --version 0.46.5" "ran"
else
    grep -q "Versions before 0.46.6 are signed with the retired update key" "$tmp/cli.out" && pass "k2 daemon install --version 0.46.5 (shipped U1-only): refused with the retired-key message" || fail "cli 0.46.5" "$(cat "$tmp/cli.out")"
fi
[[ "$cli_help" == *"(0.46.6 or newer)"* ]] && pass "k2 daemon install --help names the cutover from _DAEMON_FIRST_U2_VERSION" || fail "cli help cutover" ""

echo "== no signing key outside the helper, no unpinned signer, no key on argv =="
for f in scripts/release.sh scripts/build-local.sh scripts/windows-patch-latest-json.sh scripts/dev-build.sh scripts/build-app.sh scripts/lib/build-binaries.sh scripts/sign-daemon.sh; do
    bad="$(grep -nE 'export[[:space:]]+TAURI_SIGNING|TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)?=|--private-key|--password|bunx @tauri-apps/cli|signer sign|never rotate' "$ROOT/$f" | grep -v '^\s*#' || true)"
    bad="$(printf '%s\n' "$bad" | grep -vE '^[0-9]+:[[:space:]]*#' || true)"
    [ -z "$bad" ] && pass "$f: no exported key, no key/password flag, no bunx signer" || fail "$f" "$bad"
done
bad="$(grep -rnE '\$\{?TAURI_SIGNING|secrets\.TAURI_SIGNING|TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)?[:=]' "$ROOT/.github" "$ROOT/scripts" | grep -v 'scripts/lib/sign-updater.sh:' || true)"
[ -z "$bad" ] && pass "no script or workflow reads, sets or passes TAURI_SIGNING_* (only the helper's guard looks)" || fail "key references" "$bad"
wf="$ROOT/.github/workflows/daemon-binaries.yml"
if grep -qE 'TAURI_SIGNING|signer sign|tauri-cli|merge-daemon-manifest:' "$wf"; then
    fail "workflow" "$(grep -nE 'TAURI_SIGNING|signer sign|tauri-cli|merge-daemon-manifest:' "$wf")"
else
    pass "daemon-binaries.yml: no signing key, no tauri signer, no merge job"
fi
grep -q '"${{ matrix.asset }}.sha256"' "$wf" && ! grep -q '"${{ matrix.asset }}.sig"' "$wf" \
    && pass "daemon-binaries.yml uploads the daemon .sha256, not a .sig" || fail "workflow upload" ""
python3 -I - "$wf" <<'PY' && pass "daemon-binaries.yml: SHA-pinned attest-build-provenance for the daemon, id-token/attestations permissions, daemon upload without --clobber" || fail "workflow provenance" "see $wf"
import re, sys
t = open(sys.argv[1]).read()
assert re.search(r"uses: actions/attest-build-provenance@[0-9a-f]{40} # v", t), "attest action not SHA-pinned"
assert "subject-path: ${{ matrix.asset }}" in t
assert re.search(r"permissions:\n\s+contents: write\n\s+id-token: write\n\s+attestations: write", t)
up = t[t.index("- name: Upload to release"):]
first = up[up.index('gh release upload "$TAG"'):]
first = first[:first.index("\n          gh release upload", 1)]
assert "${{ matrix.asset }}" in first and "--clobber" not in first, first
PY
python3 -I - "$ROOT/.github/workflows" <<'PY' && pass "no workflow splices a \${{ }} value (other than matrix) into a run: script, and no secret is used outside env:" || fail "workflow injection" "see above"
import glob, os, re, sys
bad = []
for f in sorted(glob.glob(os.path.join(sys.argv[1], "*.yml"))):
    lines = open(f).read().split("\n")
    run_indent = None
    for n, l in enumerate(lines, 1):
        ind = len(l) - len(l.lstrip())
        if run_indent is not None and l.strip() and ind <= run_indent:
            run_indent = None
        m = re.match(r"^(\s*)(?:- )?run:\s*(.*)$", l)
        if m:
            if m.group(2).strip() in ("|", ">", "|-", ">-"):
                run_indent = len(m.group(1))
                continue
            text = m.group(2)
        elif run_indent is not None:
            text = l
        else:
            text = None
        if text is not None:
            for e in re.findall(r"\$\{\{[^}]*\}\}", text):
                if not re.match(r"\$\{\{\s*matrix\.", e):
                    bad.append("%s:%d spliced into run: %s" % (os.path.basename(f), n, e))
            if "secrets." in text:
                bad.append("%s:%d secret in a run: script" % (os.path.basename(f), n))
for b in bad:
    print(b)
sys.exit(1 if bad else 0)
PY
R="$ROOT/scripts/release.sh"
env_line=$(grep -n 'source "$PROJECT_DIR/.env"' "$R" | head -1 | cut -d: -f1)
guard_line=$(grep -n '^k2u_env_guard$' "$R" | head -1 | cut -d: -f1)
pre_line=$(grep -n 'scripts/sign-daemon.sh" --preflight "$VERSION"' "$R" | head -1 | cut -d: -f1)
copies_line=$(grep -n '^cargo test -p k2-release-keys$' "$R" | head -1 | cut -d: -f1)
[ -n "$copies_line" ] && [ -n "$pre_line" ] && [ "$copies_line" -lt "$pre_line" ] && [ "$copies_line" -lt "$(grep -n '^if \[ "$SKIP_BUILD" = 0 \]; then$' "$R" | head -1 | cut -d: -f1)" ] \
    && pass "release.sh Step 0.45 runs the key copies test (cargo test -p k2-release-keys) before anything else signs or builds" || fail "copies test in release.sh" "line=$copies_line pre=$pre_line"
first_build=$(grep -nE '^(RUSTFLAGS=.*)?cargo |^bun |^bunx ' "$R" | grep -v ':cargo test -p k2-release-keys$' | head -1 | cut -d: -f1)
[ -n "$env_line" ] && [ -n "$guard_line" ] && [ -n "$pre_line" ] && [ -n "$first_build" ] \
    && [ "$guard_line" -gt "$env_line" ] && [ "$pre_line" -gt "$guard_line" ] && [ "$first_build" -gt "$pre_line" ] \
    && pass "release.sh: .env → k2u_env_guard → signing preflight → first build" || fail "release.sh order" "env=$env_line guard=$guard_line pre=$pre_line build=$first_build"
sd_line=$(grep -n 'bash "$PROJECT_DIR/scripts/sign-daemon.sh" "${VERSION}"' "$R" | head -1 | cut -d: -f1)
rh_line=$(grep -n 'bash "$PROJECT_DIR/scripts/sign-root-helper.sh" "${VERSION}"' "$R" | head -1 | cut -d: -f1)
[ -n "$sd_line" ] && [ -n "$rh_line" ] && [ "$sd_line" -lt "$rh_line" ] && pass "release.sh Step 10.5 signs the Linux daemons before the root helper" || fail "10.5 order" "$sd_line $rh_line"
grep -q '^k2u_sign u1 "target/release/bundle/macos/K2.app.tar.gz" K2.app.tar.gz "$VERSION" \\$' "$R" \
    && grep -q '^k2u_sign u1 "$DIST_DIR/$MAC_ASSET" "$MAC_ASSET" "$VERSION" "$DIST_DIR/$MAC_ASSET.sig" raw$' "$R" \
    && pass "release.sh signs the app bundle (b64) and the macOS daemon (raw) through the helper" || fail "release.sh sign calls" ""

for want in \
    'k2u_sign u2 "target/release/bundle/macos/K2.app.tar.gz" K2.app.tar.gz "$VERSION" \' \
    'k2u_sign u2 "$DIST_DIR/$MAC_ASSET" "$MAC_ASSET" "$VERSION" "$DIST_DIR/$MAC_ASSET.sig2" raw' \
    '"k2_signatures": { "${U2_KEY_ID}": "${APP_SIG2_B64}" }' \
    '"sig2": "${DL_BASE}/${MAC_ASSET}.sig2"' \
    '"$DIST_DIR/${MAC_ASSET}.sig2"' \
    'k2_verify_published_u2 || U2_VERIFY_FAILED=1' \
    '[ -z "$WIN_PATCH_FAILED" ] && [ -z "$U2_VERIFY_FAILED" ] && [ -z "$DAEMON_SIGN_FAILED" ]' \
    'WIN_PATCH_FAILED=1' \
    'Re-run: bash scripts/windows-patch-latest-json.sh' \
    'k2u_verify_u1 "target/release/bundle/macos/K2.app.tar.gz" "$SIG_FILE" K2.app.tar.gz \' \
    '[ -s "$SIG_FILE" ] || { echo "FATAL: $SIG_FILE missing or empty' \
    'k2u_verify_u1 "target/release/bundle/macos/K2.app.tar.gz" "$d/app.sig" K2.app.tar.gz \' \
    'k2u_verify_u1 "$DIST_DIR/$MAC_ASSET" "$d/mac.sig" "$MAC_ASSET" \' \
    'for _k2_seam in K2_UPDATER_SRC_ROOT K2_SIGN_SOURCE_DIR; do'; do
    grep -qF -- "$want" "$R" && pass "release.sh (dual-sign): $want" || fail "release.sh dual-sign" "missing: $want"
done
grep -qF 'k2u_sign u2 "$ASSETS/$A" "$A" "$VERSION" "$ASSETS/$A.sig2" raw' "$ROOT/scripts/sign-daemon.sh" \
    && pass "sign-daemon.sh signs each Linux daemon with U2 too" || fail "sign-daemon u2" ""
grep -qF 'k2u_verify_u1 "$CHECK/$A" "$CHECK/$A.sig" "$A"' "$ROOT/scripts/sign-daemon.sh" \
    && pass "sign-daemon.sh re-checks each published .sig (U1) after the upload" || fail "sign-daemon u1 post-check" ""
# The SIG_CONTENT fallback that let Step 8 publish an empty "signature" is gone.
! grep -qE '^SIG_CONTENT=""$' "$R" && pass "release.sh Step 8 has no empty-signature fallback" || fail "Step 8 fallback" "SIG_CONTENT=\"\" still there"
# The seam guard runs: release.sh exits before anything with K2_UPDATER_SRC_ROOT set.
seam_snip="$(sed -n '/^for _k2_seam in /,/^unset _k2_seam$/p' "$R")"
if K2_UPDATER_SRC_ROOT=/tmp/x bash -c "$seam_snip" >"$tmp/seam.out" 2>&1; then
    fail "seam guard" "passed with K2_UPDATER_SRC_ROOT set"
else
    grep -q "K2_UPDATER_SRC_ROOT is set" "$tmp/seam.out" && pass "release.sh refuses K2_UPDATER_SRC_ROOT (a test seam)" || fail "seam guard" "$(cat "$tmp/seam.out")"
fi
if env -u K2_UPDATER_SRC_ROOT -u K2_SIGN_SOURCE_DIR bash -c "$seam_snip" >"$tmp/seam.out" 2>&1; then
    pass "release.sh's seam guard passes a clean env"
else
    fail "seam guard clean" "$(cat "$tmp/seam.out")"
fi
if env -u K2_UPDATER_SRC_ROOT K2_SIGN_SOURCE_DIR=/tmp/y bash -c "$seam_snip" >"$tmp/seam.out" 2>&1; then
    fail "seam guard 2" "passed with K2_SIGN_SOURCE_DIR set"
else
    pass "release.sh refuses K2_SIGN_SOURCE_DIR (a test seam)"
fi
grep -qF 'k2u_sign u2 "$STAGE/setup.exe" "$WIN_ASSET" "$VERSION" "$STAGE/setup.exe.sig2" raw' "$ROOT/scripts/windows-patch-latest-json.sh" \
    && pass "the Windows patch signs setup.exe with U2 too" || fail "windows u2" ""

echo
echo "updater-signing: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
