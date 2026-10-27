#!/usr/bin/env bash
# Update signing against the REAL minisign (jedisct1, 0.11+) and a U1 key
# made by the REAL Tauri signer, with throwaway keys only.
#
#   bash tests/packaging/updater-signing-real-minisign.sh <tauri-cli> [<fixture-out-dir>]
#
#   <tauri-cli>  a command that runs `tauri` (e.g. "node_modules/.bin/tauri").
#                Required: U1 must be Tauri-format, the way ~/.tauri/k2-updater.key
#                is, because the bridge signatures come from that key.
#   <fixture-out-dir>  optional: also write the public keys, data and
#                signatures for crates/k2-release-keys/tests/fixtures/real-minisign
#                (public material of throwaway keys only).
#
# What tests/packaging/updater-signing.sh (stub minisign) can't prove:
#   - minisign 0.12 reads the Tauri-format U1 key through a /dev/fd pipe and
#     its password from stdin (k2u_sign), and signs prehashed (ED);
#   - those U1 signatures are what a 0.46.4 install checks (old app plugin
#     form, old daemon form), and the U2 signatures what a 0.46.5 install
#     checks (file: + version:), with real minisign -V in the shell
#     installers (cli/k2, install-daemon.sh).
# The `security` tool is a stub over a temp folder: no real keychain. No
# skips: a missing tool is a failure.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TAURI_CLI="${1:?usage: updater-signing-real-minisign.sh <tauri-cli> [<fixture-out-dir>]}"
FIX_OUT="${2:-}"

pass=0 fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD K2_RELEASE_REPO K2_SIGN_SOURCE_DIR || true

echo "== tools =="
mv_line="$(minisign -v 2>&1 | head -1 || true)"
[[ "$mv_line" =~ ^minisign\ 0\.(1[1-9]|[2-9][0-9]) ]] || { echo "FAIL: real minisign 0.11+ required on PATH (got: '$mv_line')" >&2; exit 1; }
pass "real $mv_line"
$TAURI_CLI signer --help >/dev/null 2>&1 || { echo "FAIL: '$TAURI_CLI signer' does not run" >&2; exit 1; }
pass "tauri signer runs ($($TAURI_CLI --version 2>&1 | head -1))"

# Stub keychain only (never the real one).
bin="$tmp/bin"; mkdir -p "$bin"
cat > "$bin/security" <<'EOF'
#!/usr/bin/env bash
[ "$1" = find-generic-password ] || { echo "stub security: unsupported $1" >&2; exit 2; }
svc="" w=0; shift
while [ $# -gt 0 ]; do case "$1" in -s) svc="$2"; shift 2 ;; -w) w=1; shift ;; *) shift ;; esac; done
f="$STUB_KEYCHAIN/$svc"
[ -f "$f" ] || { echo "security: The specified item could not be found in the keychain." >&2; exit 44; }
# The real tool prints the password and a newline.
[ "$w" = 1 ] && { cat "$f"; echo; }
exit 0
EOF
chmod +x "$bin/security"
export PATH="$bin:$PATH" STUB_KEYCHAIN="$tmp/keychain"
mkdir -p "$STUB_KEYCHAIN"
export HOME="$tmp/home"; mkdir -p "$HOME"
export K2_RELEASE_AUDIT_DIR="$tmp/audit" K2_UPDATER_SRC_ROOT="$tmp/src"

echo "== throwaway keys: U1 from the Tauri signer, U2/U3 from minisign -G =="
rand() { python3 -I -c 'import secrets; print(secrets.token_hex(16))'; }
PW1="u1-$(rand)" PW2="u2-$(rand)" PW3="u3-$(rand)"
# Throwaway password on argv is fine here (no real key, no real secret).
$TAURI_CLI signer generate --ci -p "$PW1" -w "$tmp/u1.tauri.key" -f >/dev/null 2>&1 \
    || { echo "FAIL: tauri signer generate" >&2; exit 1; }
[ -s "$tmp/u1.tauri.key" ] && [ -s "$tmp/u1.tauri.key.pub" ] && pass "tauri signer made a U1 key pair" || fail "tauri key" "$(ls -la "$tmp")"
# Tauri's .pub is base64 of the two-line minisign public key file.
U1_TAURI_PUB="$(tr -d '\n\r' < "$tmp/u1.tauri.key.pub")"
U1_LINE="$(python3 -I -c 'import base64,sys; print(base64.b64decode(sys.argv[1]).decode().splitlines()[1])' "$U1_TAURI_PUB")"
printf '%s\n%s\n' "$PW2" "$PW2" | minisign -G -p "$tmp/u2.pub" -s "$tmp/u2.key" >/dev/null 2>&1 || { echo "FAIL: minisign -G U2" >&2; exit 1; }
printf '%s\n%s\n' "$PW3" "$PW3" | minisign -G -p "$tmp/u3.pub" -s "$tmp/u3.key" >/dev/null 2>&1 || { echo "FAIL: minisign -G U3" >&2; exit 1; }
U2_LINE="$(sed -n 2p "$tmp/u2.pub")" U3_LINE="$(sed -n 2p "$tmp/u3.pub")"
U2_ID="$(sed -n 1p "$tmp/u2.pub" | awk '{print $NF}')"
# The ceremony prints the id this way; it must be the key line's own id
# (minisign 0.12 writes "minisign public key <ID>", no colon).
want_id="$(python3 -I -c 'import base64,sys; print(base64.b64decode(sys.argv[1])[2:10][::-1].hex().upper())' "$U2_LINE")"
[ "$U2_ID" = "$want_id" ] && pass "the .pub comment's last field is the key id ($U2_ID), as the ceremony prints it" || fail "key id" "'$U2_ID' vs $want_id"

mkdir -p "$HOME/.k2-updater"; chmod 700 "$HOME/.k2-updater"
cp "$tmp/u1.tauri.key" "$HOME/.k2-updater/k2-updater-u1.key"   # Tauri format, as on z3mbpZ
cp "$tmp/u2.key" "$HOME/.k2-updater/k2-updater-u2.key"
chmod 600 "$HOME/.k2-updater/"*.key
printf '%s' "$PW1" > "$STUB_KEYCHAIN/k2-updater-signing-u1"
printf '%s' "$PW2" > "$STUB_KEYCHAIN/k2-updater-signing-u2"

# Fixture tree: the four verifier files, U1 swapped for the throwaway one,
# then the ceremony's own `pubkeys` writes U2/U3.
SRC="$K2_UPDATER_SRC_ROOT"
for f in crates/k2-release-keys/src/lib.rs cli/k2 scripts/install-daemon.sh src-tauri/tauri.conf.json; do
    mkdir -p "$SRC/$(dirname "$f")"; cp "$ROOT/$f" "$SRC/$f"
done
python3 -I - "$SRC/crates/k2-release-keys/src/lib.rs" "$U1_LINE" <<'PY'
import re, sys
p, u1 = sys.argv[1:3]
s = open(p).read()
s, n = re.subn(r'pub const LEGACY_UPDATER_KEY_B64: &str = "[^"]+";', 'pub const LEGACY_UPDATER_KEY_B64: &str = "%s";' % u1, s)
assert n == 1
open(p, "w").write(s)
PY
bash "$ROOT/scripts/updater-key-ceremony.sh" pubkeys --root "$SRC" "$U2_LINE" "$U3_LINE" >/dev/null \
    && pass "ceremony pubkeys wrote U2/U3 into the fixture tree" || fail "pubkeys" ""

LIB="$ROOT/scripts/lib/sign-updater.sh"
lib() { bash -c 'source "$1"; shift; "$@"' _ "$LIB" "$@"; }

echo "== preflight: both slots sign a self-test with real minisign =="
lib k2u_preflight 9.9.9 2>"$tmp/err" && pass "k2u_preflight (U1 Tauri-format key via /dev/fd, U2, passwords on stdin)" || fail "preflight" "$(cat "$tmp/err")"

echo "== sign with both keys =="
DA="k2-daemon-linux-x86_64"
head -c 3000 /dev/urandom > "$tmp/$DA"
head -c 5000 /dev/urandom > "$tmp/K2.app.tar.gz"
lib k2u_sign u1 "$tmp/$DA" "$DA" 9.9.9 "$tmp/$DA.sig" raw 2>"$tmp/err" && pass "U1 raw (daemon .sig)" || fail "u1 raw" "$(cat "$tmp/err")"
lib k2u_sign u2 "$tmp/$DA" "$DA" 9.9.9 "$tmp/$DA.sig2" raw 2>"$tmp/err" && pass "U2 raw (daemon .sig2)" || fail "u2 raw" "$(cat "$tmp/err")"
lib k2u_sign u1 "$tmp/K2.app.tar.gz" K2.app.tar.gz 9.9.9 "$tmp/K2.app.tar.gz.sig" b64 2>"$tmp/err" && pass "U1 b64 (latest.json signature)" || fail "u1 b64" "$(cat "$tmp/err")"
lib k2u_sign u2 "$tmp/K2.app.tar.gz" K2.app.tar.gz 9.9.9 "$tmp/K2.app.tar.gz.sig2" raw 2>"$tmp/err" && pass "U2 raw (k2_signatures)" || fail "u2 app" "$(cat "$tmp/err")"

python3 -I - "$tmp/$DA.sig" "$tmp/$DA.sig2" <<'PY' && pass "both are prehashed (ED); U1 comment timestamp+file, U2 adds version" || fail "formats" "$(cat "$tmp/$DA.sig" "$tmp/$DA.sig2")"
import base64, re, sys
u1 = open(sys.argv[1]).read().split("\n")
u2 = open(sys.argv[2]).read().split("\n")
for l in (u1, u2):
    assert base64.b64decode(l[1])[:2] == b"ED"
assert u1[0] == "untrusted comment: signature from tauri secret key", u1[0]
assert re.fullmatch(r"trusted comment: timestamp:\d+\tfile:k2-daemon-linux-x86_64", u1[2]), u1[2]
assert re.fullmatch(r"trusted comment: timestamp:\d+\tfile:k2-daemon-linux-x86_64\tversion:9\.9\.9", u2[2]), u2[2]
PY

echo "== the verifiers (real minisign -V) =="
lib k2u_verify_u1 "$tmp/$DA" "$tmp/$DA.sig" "$DA" 2>"$tmp/err" && pass "k2u_verify_u1 daemon" || fail "verify_u1" "$(cat "$tmp/err")"
lib k2u_verify_u1 "$tmp/K2.app.tar.gz" "$tmp/K2.app.tar.gz.sig" K2.app.tar.gz 2>"$tmp/err" && pass "k2u_verify_u1 app (b64)" || fail "verify_u1 app" "$(cat "$tmp/err")"
lib k2u_verify_u2 "$tmp/$DA" "$tmp/$DA.sig2" "$DA" 9.9.9 2>"$tmp/err" && pass "k2u_verify_u2 daemon" || fail "verify_u2" "$(cat "$tmp/err")"
if lib k2u_verify_u2 "$tmp/$DA" "$tmp/$DA.sig" "$DA" 9.9.9 2>/dev/null; then fail "u1 as u2" "accepted"; else pass "k2u_verify_u2 refuses the U1 signature"; fi
if lib k2u_verify_u2 "$tmp/$DA" "$tmp/$DA.sig2" "$DA" 9.9.8 2>/dev/null; then fail "u2 version" "accepted"; else pass "k2u_verify_u2 refuses another version"; fi
if lib k2u_verify_u1 "$tmp/$DA" "$tmp/$DA.sig2" "$DA" 2>/dev/null; then fail "u2 as u1" "accepted"; else pass "k2u_verify_u1 refuses the U2 signature"; fi
# A 0.46.4 shell installer: `minisign -Vm <bin> -p <U1 pub file>`.
python3 -I -c 'import base64,sys; open(sys.argv[2],"wb").write(base64.b64decode(sys.argv[1]))' "$U1_TAURI_PUB" "$tmp/u1.pub"
minisign -Vm "$tmp/$DA" -p "$tmp/u1.pub" -x "$tmp/$DA.sig" >/dev/null 2>&1 && pass "0.46.4 install-daemon.sh logic (minisign -Vm -p U1) accepts the bridge .sig" || fail "old installer" ""

echo "== the shell installers with real minisign =="
case "$(uname -s)" in Darwin) pos=macos ;; Linux) pos=linux ;; *) pos=unknown ;; esac
case "$(uname -m)" in arm64|aarch64) parch=aarch64 ;; x86_64|amd64) parch=x86_64 ;; *) parch=unknown ;; esac
PLAT="$pos-$parch"; NA="k2-daemon-$PLAT"
inst="$tmp/inst"; mkdir -p "$inst"
printf 'the new daemon for %s\n' "$PLAT" > "$inst/$NA"
lib k2u_sign u1 "$inst/$NA" "$NA" 9.9.9 "$inst/$NA.sig" raw 2>/dev/null
lib k2u_sign u2 "$inst/$NA" "$NA" 9.9.9 "$inst/$NA.sig2" raw 2>/dev/null
mkman() { # <out> <version> <sig2 file or "">
    python3 -I - "$1" "$2" "$3" "$inst/$NA" "$PLAT" <<'PY'
import hashlib, json, sys
out, version, sig2, binary, plat = sys.argv[1:6]
e = {"url": "file://" + binary, "sig": "file://" + binary + ".sig", "sha256": hashlib.sha256(open(binary, "rb").read()).hexdigest()}
if sig2:
    e["sig2"] = "file://" + sig2
json.dump({"version": version, "pub_date": "", "artifacts": {plat: e}}, open(out, "w"))
PY
}
mkman "$inst/dual.json" 9.9.9 "$inst/$NA.sig2"
mkman "$inst/u1as2.json" 9.9.9 "$inst/$NA.sig"
mkman "$inst/ver.json" 9.9.10 "$inst/$NA.sig2"
rm -rf "$tmp/ib"
env K2_MANIFEST_URL="file://$inst/dual.json" K2_BIN_DIR="$tmp/ib" K2_NO_SERVICE=1 K2_APP_ENGINE_PATH="$tmp/no-engine" \
    sh "$SRC/scripts/install-daemon.sh" >"$tmp/i.out" 2>&1 && cmp -s "$tmp/ib/k2-daemon" "$inst/$NA" \
    && pass "install-daemon.sh installs the dual-signed release (real minisign, U2)" || fail "install-daemon" "$(tail -5 "$tmp/i.out")"
for m in u1as2 ver; do
    rm -rf "$tmp/ib"
    if env K2_MANIFEST_URL="file://$inst/$m.json" K2_BIN_DIR="$tmp/ib" K2_NO_SERVICE=1 K2_APP_ENGINE_PATH="$tmp/no-engine" \
        sh "$SRC/scripts/install-daemon.sh" >"$tmp/i.out" 2>&1; then
        fail "install-daemon $m" "installed"
    else
        [ ! -e "$tmp/ib/k2-daemon" ] && pass "install-daemon.sh refuses $m (real minisign)" || fail "install-daemon $m" "left a binary"
    fi
done
rm -rf "$tmp/cb"
bash "$SRC/cli/k2" daemon install --manifest-url "file://$inst/dual.json" --bin-dir "$tmp/cb" --no-service >"$tmp/c.out" 2>&1 \
    && cmp -s "$tmp/cb/k2-daemon" "$inst/$NA" \
    && pass "k2 daemon install installs the dual-signed release (real minisign, U2)" || fail "cli install" "$(tail -5 "$tmp/c.out")"
for m in u1as2 ver; do
    rm -rf "$tmp/cb"
    if bash "$SRC/cli/k2" daemon install --manifest-url "file://$inst/$m.json" --bin-dir "$tmp/cb" --no-service >"$tmp/c.out" 2>&1; then
        fail "cli $m" "installed"
    else
        [ ! -e "$tmp/cb/k2-daemon" ] && pass "k2 daemon install refuses $m (real minisign)" || fail "cli $m" "left a binary"
    fi
done

if [ -n "$FIX_OUT" ]; then
    mkdir -p "$FIX_OUT"
    printf '%s\n' "$U1_TAURI_PUB" > "$FIX_OUT/u1.tauri.pub"
    printf '%s\n' "$U2_LINE" > "$FIX_OUT/u2.pub.line"
    printf '%s\n' "$U3_LINE" > "$FIX_OUT/u3.pub.line"
    printf '%s\n' "$U2_ID" > "$FIX_OUT/u2.id"
    cp "$tmp/$DA" "$tmp/$DA.sig" "$tmp/$DA.sig2" "$tmp/K2.app.tar.gz" "$tmp/K2.app.tar.gz.sig" "$tmp/K2.app.tar.gz.sig2" "$FIX_OUT/"
    printf '%s\n' "$mv_line" > "$FIX_OUT/MADE_WITH"
    $TAURI_CLI --version >> "$FIX_OUT/MADE_WITH" 2>&1
    pass "fixtures written to $FIX_OUT (public keys, data, signatures; no secret key)"
    python3 -I - "$FIX_OUT" "$tmp/u1.tauri.key" "$tmp/u2.key" "$tmp/u3.key" "$PW1" "$PW2" "$PW3" <<'PY' \
        && pass "no secret key (raw or base64) or password in the fixtures" || fail "fixtures" "secret material in $FIX_OUT"
import base64, os, sys
d, k1, k2, k3 = sys.argv[1:5]
needles = [p.encode() for p in sys.argv[5:8]] + [b"encrypted secret key"]
for k in (k1, k2, k3):
    raw = open(k, "rb").read().strip()
    needles.append(raw)
    for line in raw.splitlines():
        if len(line) > 40:
            needles.append(line)
    if not raw.startswith(b"untrusted comment:"):
        dec = base64.b64decode(raw)
        needles += [l for l in dec.splitlines() if len(l) > 40]
for f in os.listdir(d):
    blob = open(os.path.join(d, f), "rb").read()
    for n in needles:
        assert n not in blob, (f, n[:20])
PY
fi

echo
echo "updater-signing-real-minisign: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
