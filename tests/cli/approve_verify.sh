#!/usr/bin/env bash
# approve_verify.sh — `k2 approve verify|show|key-id` (prd-signed-approvals-v1):
# run with NO daemon connection at all (no port, no token, an empty HOME),
# over the shared fixtures. Needs the k2-approve program:
#   cargo build -p k2-approve && K2_APPROVE_BIN=target/debug/k2-approve bash tests/cli/approve_verify.sh
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
FIX="$REPO/crates/k2-approve/fixtures/sa1"
. "$REPO/tests/cli/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
[ -n "${K2_APPROVE_BIN:-}" ] && [ -x "$K2_APPROVE_BIN" ] || { echo "FAIL: set K2_APPROVE_BIN to a built k2-approve" >&2; exit 1; }
tmp="$(mktemp -d "${TMPDIR:-/tmp}/approve-cli.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/home"
npass=0 nfail=0
pass() { echo "  ok: $1"; npass=$((npass + 1)); }
fail() { echo "  FAIL: $1" >&2; [ -z "${2:-}" ] || echo "        $2" >&2; nfail=$((nfail + 1)); }

# k2 with no way to reach any daemon.
k2() { env -u K2_PORT -u K2_HOOK_TOKEN -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2_HOST HOME="$tmp/home" "$K2" "$@"; }

# check <name> <rc> <text> <args...>
check() {
    local name="$1" want_rc="$2" want="$3" rc=0
    shift 3
    k2 "$@" > "$tmp/out" 2>&1 || rc=$?
    if [ "$rc" != "$want_rc" ]; then
        fail "$name" "exit $rc, wanted $want_rc: $(head -c 400 "$tmp/out")"
    elif [ -n "$want" ] && ! grep -qF -- "$want" "$tmp/out"; then
        fail "$name" "no '$want' in: $(head -c 400 "$tmp/out")"
    else
        pass "$name"
    fi
}

R="$FIX/records"
NONCE=00112233445566778899aabbccddeeff
REL=(--keys "$FIX/keys" --rp k2sign@build-mac --kind k2.release --check biometric --view device --nonce "$NONCE" --at 2026-11-11T14:05:00Z)

check "an approved release (biometric, device)" 0 "approved by alex with key alex-phone-bio (ES256, biometric on device)" \
    approve verify --approval "$R/ok-release-phone-bio.json" --payload "$R/ok-release-phone-bio.stored.bin" "${REL[@]}"
check "--json" 0 '"check":"biometric","view":"device"' \
    approve verify --approval "$R/ok-release-phone-bio.json" --payload "$R/ok-release-phone-bio.stored.bin" "${REL[@]}" --json
check "a passcode key is refused for a release" 3 "refused (weak_check)" \
    approve verify --approval "$R/refuse-credential-for-release.json" --payload "$R/refuse-credential-for-release.stored.bin" "${REL[@]}"
check "a laptop key is refused where the phone is required" 3 "refused (weak_view)" \
    approve verify --approval "$R/refuse-host-view-for-release.json" --payload "$R/refuse-host-view-for-release.stored.bin" \
    --keys "$FIX/keys" --check credential --view device --at 2026-11-11T14:05:00Z
check "bytes other than the stored ones" 3 "refused (payload_mismatch)" \
    approve verify --approval "$R/refuse-bytes-not-the-stored-ones.json" --payload "$FIX/payloads/valid/release.bin" "${REL[@]}"
check "a one-byte change" 3 "refused (bad_signature)" \
    approve verify --approval "$R/refuse-one-byte-changed.json" "${REL[@]}"
check "past answer_by" 3 "refused (expired)" \
    approve verify --approval "$R/ok-release-phone-bio.json" --keys "$FIX/keys" --at 2026-11-11T14:15:01Z
check "another relying party" 3 "refused (wrong_rp)" \
    approve verify --approval "$R/ok-release-phone-bio.json" --keys "$FIX/keys" --rp k2sign@other-mac --at 2026-11-11T14:05:00Z
check "a record that claims its own check" 3 "refused (record)" \
    approve verify --approval "$R/refuse-record-claims-a-check.json" "${REL[@]}"
check "the unchecked options are named" 0 "not checked because not given: --payload" \
    approve verify --approval "$R/ok-release-phone-bio.json" --keys "$FIX/keys" --at 2026-11-11T14:05:00Z

mkdir -p "$tmp/used"
check "first use with --used-nonces" 0 "approved by alex" \
    approve verify --approval "$R/ok-release-phone-bio.json" --payload "$R/ok-release-phone-bio.stored.bin" "${REL[@]}" --used-nonces "$tmp/used"
check "second use is refused" 3 "refused (nonce_used)" \
    approve verify --approval "$R/ok-release-phone-bio.json" --payload "$R/ok-release-phone-bio.stored.bin" "${REL[@]}" --used-nonces "$tmp/used"
[ -f "$tmp/used/$NONCE" ] && pass "the nonce is recorded as a file" || fail "no $tmp/used/$NONCE"

check "--keys is required" 2 "--keys is required" approve verify --approval "$R/ok-release-phone-bio.json"
check "an unknown option" 2 "unknown argument --level" approve verify --approval "$R/ok-release-phone-bio.json" --keys "$FIX/keys" --level 3
check "a bad key folder" 2 "key folder" approve verify --approval "$R/ok-release-phone-bio.json" --keys "$tmp/home"

want_text="$(cat "$FIX/payloads/valid/release.txt")"
if out="$(k2 approve show "$FIX/payloads/valid/release.bin")" && [ "$out" = "$want_text" ]; then
    pass "show prints the fixture's screen text"
else
    fail "show" "$out"
fi
check "show refuses a hidden character" 3 "refused (forbidden_char)" approve show "$FIX/payloads/invalid/char-rlo-raw.bin"
words="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["alex-phone-bio"]["words"])' "$FIX/keys.json")"
check "key-id prints the fingerprint words" 0 "words:  $words" approve key-id "$FIX/keys/alex-phone-bio.pem"
check "help" 0 "k2 approve verify --approval" approve --help
# request/status/cancel are the daemon's: with no connection they stop at the gate.
check "request needs the daemon" 1 "Cannot connect to K2" approve request --to alex --payload x

rc=0
env -u K2_APPROVE_BIN PATH=/usr/bin:/bin HOME="$tmp/home" "$K2" approve verify --approval x --keys y > "$tmp/out" 2>&1 || rc=$?
if [ "$rc" = 2 ] && grep -qF "the k2-approve program isn't installed here" "$tmp/out"; then pass "no program: says how to get it"; else fail "no program" "rc $rc: $(cat "$tmp/out")"; fi

echo
echo "$(basename "$0"): $npass passed, $nfail failed"
[ "$nfail" -eq 0 ]
