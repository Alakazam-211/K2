# k2sign_fixture.sh — sourced by tests/release/k2sign-*.sh (not a test itself).
# A test install of the signing user under a temp K2SIGN_TEST_ROOT, from a
# fixture copy of this tree whose U1/U2/U3 are THROWAWAY keys made here.
# Needs macOS (the signer is z3mbpZ tooling), real minisign >= 0.11 on PATH
# (it is copied in as the "pinned" one) and /usr/bin/openssl. Missing tools
# fail the test; nothing is skipped.
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD K2_SIGN_APPROVE_CMD K2SIGN_TEST_NOW \
    K2SIGN_TEST_SIGNER_USER K2SIGN_TEST_EVIDENCE K2SIGN_TEST_ROOT K2_SIGNER K2SIGN_TEST_GITHUB_API \
    K2SIGN_TEST_ANCHOR K2SIGN_TEST_NOTARIZED || true

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
npass=0 nfail=0
pass() { echo "  ok: $1"; npass=$((npass + 1)); }
fail() { echo "  FAIL: $1" >&2; [ -z "${2:-}" ] || echo "        $2" >&2; nfail=$((nfail + 1)); }
finish() {
    echo
    echo "$(basename "$0"): $npass passed, $nfail failed"
    [ "$nfail" -eq 0 ]
}

[ "$(uname -s)" = Darwin ] || { echo "FAIL: the k2sign tests run on macOS (the signer is z3mbpZ tooling)" >&2; exit 1; }
MINISIGN="$(command -v minisign)" || { echo "FAIL: minisign is not installed (brew install minisign)" >&2; exit 1; }
"$MINISIGN" -v 2>&1 | grep -q '^minisign 0\.1[1-9]' || { echo "FAIL: minisign >= 0.11 needed" >&2; exit 1; }
[ -x /usr/bin/openssl ] || { echo "FAIL: /usr/bin/openssl missing" >&2; exit 1; }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/k2sign-test.XXXXXX")"
tmp="$(cd "$tmp" && pwd -P)"
GH_PID=""
trap '[ -z "$GH_PID" ] || kill "$GH_PID" 2>/dev/null; rm -rf "$tmp"' EXIT
TREE="$tmp/tree"
TROOT="$tmp/root"
mkdir -p "$TREE/scripts" "$TREE/crates/k2-release-keys/src" "$TROOT" "$tmp/keys" "$tmp/home"
cp -R "$REPO/scripts/k2sign" "$TREE/scripts/"
mkdir -p "$TREE/scripts/lib"
cp "$REPO/scripts/lib/sign-updater.sh" "$REPO/scripts/lib/sign-approve.sh" "$REPO/scripts/lib/check-daemon-asset.py" "$TREE/scripts/lib/"
cp "$REPO/crates/k2-release-keys/src/lib.rs" "$TREE/crates/k2-release-keys/src/lib.rs"

# Throwaway U1 (Tauri form: base64 of the minisign secret key file), U3.
U1_PW="u1-throwaway-$$"
printf '%s\n%s\n' "$U1_PW" "$U1_PW" | "$MINISIGN" -G -p "$tmp/keys/u1.pub" -s "$tmp/keys/u1.key" >/dev/null 2>&1
base64 < "$tmp/keys/u1.key" | tr -d '\n' > "$tmp/keys/u1.tauri.key"
U1_LINE="$(sed -n 2p "$tmp/keys/u1.pub")"
printf 'u3pw\nu3pw\n' | "$MINISIGN" -G -p "$tmp/keys/u3.pub" -s "$tmp/keys/u3.key" >/dev/null 2>&1
U3_LINE="$(sed -n 2p "$tmp/keys/u3.pub")"

# fixture_keys <U1 line> [<U2 line> <U3 line> [<other Mac's primary>…]]: rewrite
# the fixture lib.rs. Other Macs' primaries are listed BEFORE U2 (so "this
# Mac's key" is never simply the first one).
fixture_keys() {
    python3 -I - "$TREE/crates/k2-release-keys/src/lib.rs" "$@" <<'PY'
import base64, re, sys
p, u1 = sys.argv[1], sys.argv[2]
s = open(p).read()
s, n = re.subn(r'pub const LEGACY_UPDATER_KEY_B64: &str = "[^"]+";', 'pub const LEGACY_UPDATER_KEY_B64: &str = "%s";' % u1, s)
assert n == 1
def kid(line):
    return base64.b64decode(line)[2:10][::-1].hex().upper()
entries = ""
if len(sys.argv) > 3:
    u2, u3, others = sys.argv[3], sys.argv[4], sys.argv[5:]
    entries = "\n".join('    TrustedKey { role: "%s", key_id: "%s", public_key_b64: "%s" },' % (r, kid(k), k)
                        for r, k in [("primary", o) for o in others] + [("primary", u2), ("backup", u3)])
    entries = "\n" + entries + "\n"
block = '// BEGIN UPDATER_KEYS (fixture)\npub const UPDATER_KEYS: &[TrustedKey] = &[%s];\n' % entries
s = re.sub(r'// BEGIN UPDATER_KEYS.*?(?=// END UPDATER_KEYS)', block, s, flags=re.S)
open(p, "w").write(s)
PY
}
fixture_keys "$U1_LINE"

install_signer() {
    K2SIGN_TEST_ROOT="$TROOT" bash "$TREE/scripts/k2sign/install.sh" --test --minisign "$MINISIGN" \
        --gh "$REPO/tests/release/fixtures/gh-stub.sh" > "$tmp/install.out" 2>&1
}

# A fake GitHub API (the S3 evidence) on 127.0.0.1; routes in $ROUTES.
GH_TOKEN_VALUE="github_pat_k2signTEST$(printf '%040d' 0)"
FIX="$TROOT/fixtures"
mkdir -p "$FIX/attest"
ROUTES="$FIX/routes.json"
printf '{"/repos/Alakazam-211/K2": {"full_name": "Alakazam-211/K2"}}\n' > "$ROUTES"
printf '%s\n' "$GH_TOKEN_VALUE" > "$FIX/token"
python3 -I "$REPO/tests/release/fixtures/fake_github.py" "$ROUTES" "$FIX/token" "$FIX/port" "$FIX/api.log" &
GH_PID=$!
disown "$GH_PID" 2>/dev/null || true
for _ in $(seq 50); do [ -s "$FIX/port" ] && break; sleep 0.1; done
[ -s "$FIX/port" ] || { echo "FAIL: the fake GitHub API did not start" >&2; exit 1; }
export K2SIGN_TEST_GITHUB_API="http://127.0.0.1:$(cat "$FIX/port")"
# route <path> <json>: add or replace one fake API answer.
route() {
    python3 -I - "$ROUTES" "$1" "$2" <<'PY'
import json, sys
p, path, body = sys.argv[1:4]
r = json.load(open(p))
if body == "-":
    r.pop(path, None)
else:
    r[path] = json.loads(body)
json.dump(r, open(p, "w"))
PY
}
set_gh_token() {
    printf '%s\n' "$GH_TOKEN_VALUE" > "$tmp/tok"
    EXPECT_STDIN="$tmp/tok" expect 0 "GitHub token saved" "$ADM" set-gh-token --stdin
}
install_signer || { cat "$tmp/install.out" >&2; echo "FAIL: test install" >&2; exit 1; }

export K2SIGN_TEST_ROOT="$TROOT"
K="$TROOT/usr/local/libexec/k2sign/k2sign"
ADM="$TROOT/usr/local/libexec/k2sign/k2sign-admin"
AUDIT="$TROOT/var/k2sign/audit"

# expect <rc> <text or ''> <cmd...>: exact exit code, and the text in stdout+stderr.
expect() {
    local want_rc="$1" want_text="$2" rc=0
    shift 2
    "$@" > "$tmp/out" 2>&1 < "${EXPECT_STDIN:-/dev/null}" || rc=$?
    if [ "$rc" != "$want_rc" ]; then
        echo "exit $rc, wanted $want_rc: $(head -c 400 "$tmp/out")"
        return 1
    fi
    if [ -n "$want_text" ] && ! grep -qF -- "$want_text" "$tmp/out"; then
        echo "no '$want_text' in: $(head -c 400 "$tmp/out")"
        return 1
    fi
}
# check <name> <rc> <text> <cmd...>
check() {
    local name="$1" r
    shift
    if r="$(expect "$@")"; then pass "$name"; else fail "$name" "$r"; fi
}
# check_in <name> <stdin file> <rc> <text> <cmd...>
check_in() {
    local name="$1" in="$2" r
    shift 2
    if r="$(EXPECT_STDIN="$in" expect "$@")"; then pass "$name"; else fail "$name" "$r"; fi
}

# ready: U1 imported, U2 generated and landed in the tree, signer re-installed
# (pins U2/U3), an approver P-256 key enrolled. Sets U2_LINE.
ready() {
    printf '%s\n' "$U1_PW" > "$tmp/u1pw"
    EXPECT_STDIN="$tmp/u1pw" expect 0 "" "$ADM" import u1 "$tmp/keys/u1.tauri.key" --password-stdin \
        || { echo "FAIL: import u1: $(cat "$tmp/out")" >&2; exit 1; }
    expect 0 "" "$ADM" generate-u2 || { echo "FAIL: generate-u2: $(cat "$tmp/out")" >&2; exit 1; }
    U2_LINE="$(tr -d '[:space:]' < "$TROOT/usr/local/etc/k2sign/u2-generated.pub")"
    fixture_keys "$U1_LINE" "$U2_LINE" "$U3_LINE"
    install_signer || { cat "$tmp/install.out" >&2; echo "FAIL: re-install after pubkeys" >&2; exit 1; }
    /usr/bin/openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/keys/approver.key" 2>/dev/null
    /usr/bin/openssl ec -in "$tmp/keys/approver.key" -pubout -out "$tmp/keys/approver.pem" 2>/dev/null
    expect 0 "approver key installed" "$ADM" set-approver "$tmp/keys/approver.pem" \
        || { echo "FAIL: set-approver: $(cat "$tmp/out")" >&2; exit 1; }
    set_gh_token || { echo "FAIL: set-gh-token: $(cat "$tmp/out")" >&2; exit 1; }
}

# approve_with <key> <challenge file> <der out>: what K2 Release Approve does, with openssl.
approve_with() {
    /usr/bin/openssl dgst -sha256 -sign "$1" -out "$3" "$2"
}
nonce_of() { python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["nonce"])' "$1"; }
B1="$(printf 'a%.0s' $(seq 40))"
B2="$(printf 'b%.0s' $(seq 40))"
