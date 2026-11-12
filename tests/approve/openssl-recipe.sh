#!/usr/bin/env bash
# openssl-recipe.sh — the "With openssl only" recipe in docs/signed-approvals.md,
# run as written over the shared signed-approval fixtures
# (crates/k2-approve/fixtures/sa1). Every ES256 approval the fixtures accept
# verifies; a changed byte, a flipped signature byte and another key's
# signature don't. Needs python3 and openssl (LibreSSL or OpenSSL); nothing is
# skipped.
#   bash tests/approve/openssl-recipe.sh
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
FIX="$REPO/crates/k2-approve/fixtures/sa1"
DOC="$REPO/docs/signed-approvals.md"
command -v openssl >/dev/null || { echo "FAIL: openssl is not installed" >&2; exit 1; }
tmp="$(mktemp -d "${TMPDIR:-/tmp}/sa1-openssl.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
npass=0 nfail=0
pass() { echo "  ok: $1"; npass=$((npass + 1)); }
fail() { echo "  FAIL: $1" >&2; nfail=$((nfail + 1)); }

# The recipe's step 2 block, exactly as the doc prints it.
python3 -I - "$DOC" "$tmp/recipe.sh" <<'PY'
import sys
doc = open(sys.argv[1]).read()
at = doc.index("## With openssl only")
start = doc.index("   ```\n", at) + len("   ```\n")
end = doc.index("   ```\n", start)
lines = [l[3:] for l in doc[start:end].splitlines()]
assert len(lines) == 3 and lines[0].startswith("python3 -c ") and lines[1].startswith("cmp ") and lines[2].startswith("openssl dgst -sha256 -verify "), lines
open(sys.argv[2], "w").write("set -e\n" + "\n".join(lines) + "\n")
PY

# run_recipe <record> <stored payload> <pem>: the doc's commands in a fresh
# folder, with the doc's file names. Prints openssl's output; exit as the block.
run_recipe() {
    local d
    d="$(mktemp -d "$tmp/run.XXXXXX")"
    cp "$1" "$d/approval.json"
    cp "$2" "$d/stored-request.bin"
    cp "$3" "$d/phone-bio.pem"
    (cd "$d" && bash "$tmp/recipe.sh") 2>&1
}

# Cases: name, key, expected (ok / refuse), stored payload override.
python3 -I - "$FIX/cases.json" "$FIX/keys.json" > "$tmp/cases.tsv" <<'PY'
import json, sys
cases = json.load(open(sys.argv[1]))["cases"]
keys = json.load(open(sys.argv[2]))
by_id = {v["key_id"]: k for k, v in keys.items()}
for c in cases:
    rec = json.load(open(sys.argv[1].rsplit("/", 1)[0] + "/" + c["record"]))
    name = by_id.get(rec.get("key_id"))
    if name is None or keys[name]["alg"] != "ES256" or not keys[name]["trusted"]:
        continue
    want = None
    if c["expect"]["ok"]:
        want = "ok"
    elif c["expect"]["code"] in ("bad_signature", "payload_mismatch"):
        want = "refuse"
    if want and c["stored_payload"]:
        print("\t".join((c["name"], name, want, c["record"], c["stored_payload"])))
PY
[ "$(grep -c $'\tok\t' "$tmp/cases.tsv")" -ge 6 ] || { echo "FAIL: too few accepted ES256 cases in the fixtures" >&2; exit 1; }
[ "$(grep -c $'\trefuse\t' "$tmp/cases.tsv")" -ge 3 ] || { echo "FAIL: too few signature refusals in the fixtures" >&2; exit 1; }

while IFS=$'\t' read -r name key want record stored; do
    out="" rc=0
    out="$(run_recipe "$FIX/$record" "$FIX/$stored" "$FIX/keys/$key.pem")" || rc=$?
    if [ "$want" = ok ]; then
        if [ "$rc" = 0 ] && printf '%s' "$out" | grep -qx 'Verified OK'; then pass "$name verifies"; else fail "$name: wanted Verified OK, got rc $rc: $out"; fi
    else
        if [ "$rc" != 0 ] && ! printf '%s' "$out" | grep -qx 'Verified OK'; then pass "$name is refused"; else fail "$name: wanted a refusal, got rc $rc: $out"; fi
    fi
done < "$tmp/cases.tsv"

# A one-byte change to the stored copy after signing: the signature fails.
rec="$FIX/records/ok-release-phone-bio.json"
python3 -I - "$FIX/payloads/valid/release.bin" "$tmp/changed.bin" <<'PY'
import sys
b = bytearray(open(sys.argv[1], "rb").read())
i = b.index(b"0.46.13")
b[i + 6] ^= 0x01
open(sys.argv[2], "wb").write(bytes(b))
PY
d="$(mktemp -d "$tmp/run.XXXXXX")"
python3 -I -c 'import base64,json,sys; r=json.load(open(sys.argv[1])); open(sys.argv[2],"wb").write(base64.b64decode(r["sig_b64"]))' "$rec" "$d/sig.der"
if out="$(openssl dgst -sha256 -verify "$FIX/keys/alex-phone-bio.pem" -signature "$d/sig.der" "$tmp/changed.bin" 2>&1)"; then
    fail "one changed byte still verified: $out"
else
    pass "one changed byte is refused"
fi
# The record's own bytes are the stored ones for an accepted case (cmp passes).
if run_recipe "$rec" "$FIX/payloads/valid/release.bin" "$FIX/keys/alex-phone-bio.pem" >/dev/null; then
    pass "the record carries the stored bytes"
else
    fail "the recipe refused the stored release payload"
fi

echo
echo "$(basename "$0"): $npass passed, $nfail failed"
[ "$nfail" -eq 0 ]
