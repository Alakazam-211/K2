#!/usr/bin/env bash
# Check the golden build statement with two stock verifiers, neither of them
# ours: cosign (key mode, no transparency log) and the in-toto reference
# library securesystemslib (DSSE verify with an ECDSA P-256 key). Both check
# the node signature only; the owner and roster chain is kessel-attest's.
#
#   crates/kessel-attest/scripts/stock-verifiers.sh [OUT_DIR]
#
# Needs python3; cosign on PATH for the cosign half; uv (or a python3 that
# can import securesystemslib and cryptography) for the other half. Each
# half prints the exact command it ran and PASS or FAIL. Exit 0 only when
# every half that could run passed and at least one ran; a missing tool is
# reported as SKIP, never as a pass.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
fixture="$here/../../../tests/release/fixtures/provenance-v1.json"
[ -f "$fixture" ] || { echo "no fixture at $fixture" >&2; exit 2; }
out="${1:-$(mktemp -d)}"
mkdir -p "$out"

# The golden vector a: its node key, its .intoto.jsonl line, and a file
# whose sha256 is its first subject (the generator hashed exactly this text).
python3 - "$fixture" "$out" <<'PY'
import base64, json, os, sys
fx = json.load(open(sys.argv[1]))
out = sys.argv[2]
v = fx["vectors"][0]
open(os.path.join(out, "node.pem"), "w").write(fx["keys"]["node"]["public_key_pem"])
open(os.path.join(out, "keyid"), "w").write(fx["keys"]["node"]["fp"])
open(os.path.join(out, "L1.intoto.jsonl"), "w").write(v["line"])
open(os.path.join(out, "L1.intoto.b64"), "w").write(base64.b64encode(v["line"].strip().encode()).decode())
open(os.path.join(out, "acme-linux-x86_64"), "wb").write(b"acme-linux-x86_64")
# A copy with one payload byte changed, for the negative check.
env = json.loads(v["line"])
p = bytearray(base64.b64decode(env["payload"]))
p[p.index(ord("1"))] = ord("2")
env["payload"] = base64.b64encode(bytes(p)).decode()
open(os.path.join(out, "bad.intoto.jsonl"), "w").write(json.dumps(env, separators=(",", ":")) + "\n")
PY
cd "$out" || exit 2
sha256sum acme-linux-x86_64 2>/dev/null || shasum -a 256 acme-linux-x86_64

ran=0
failed=0
report() { printf '%-5s %s\n' "$1" "$2"; }

# ── cosign ───────────────────────────────────────────────────────────────
if command -v cosign >/dev/null 2>&1; then
    cosign version 2>&1 | grep -E 'GitVersion|^v' | head -n 2
    tried_ok=""
    # The flag set differs between cosign 2 and 3; try the documented forms
    # and report each.
    for sigfile in L1.intoto.jsonl L1.intoto.b64; do
        for extra in "" "--new-bundle-format=false" "--use-signed-timestamps=false --new-bundle-format=false"; do
            # shellcheck disable=SC2086
            cmd="cosign verify-blob-attestation --key node.pem --signature $sigfile --type slsaprovenance1 --insecure-ignore-tlog=true $extra acme-linux-x86_64"
            echo "\$ $cmd"
            # shellcheck disable=SC2086
            if out_text="$(cosign verify-blob-attestation --key node.pem --signature "$sigfile" --type slsaprovenance1 --insecure-ignore-tlog=true $extra acme-linux-x86_64 2>&1)"; then
                printf '%s\n' "$out_text" | tail -n 3
                tried_ok="$cmd"
                break 2
            else
                printf '%s\n' "$out_text" | tail -n 3
            fi
        done
    done
    ran=$((ran + 1))
    if [ -n "$tried_ok" ]; then
        report PASS "cosign: $tried_ok"
        # The same command on a changed payload must fail.
        neg="${tried_ok/--signature L1.intoto.jsonl/--signature bad.intoto.jsonl}"
        neg="${neg/--signature L1.intoto.b64/--signature bad.intoto.jsonl}"
        echo "\$ $neg"
        # shellcheck disable=SC2086
        if eval "$neg" >/dev/null 2>&1; then
            report FAIL "cosign accepted a changed payload"
            failed=$((failed + 1))
        else
            report PASS "cosign refuses a changed payload"
        fi
        # And a file the statement doesn't name (cosign's subject check).
        printf 'not the release' > other-file
        neg="${tried_ok% acme-linux-x86_64} other-file"
        echo "\$ $neg"
        # shellcheck disable=SC2086
        if eval "$neg" >/dev/null 2>&1; then
            report FAIL "cosign accepted a file the statement doesn't name"
            failed=$((failed + 1))
        else
            report PASS "cosign refuses a file the statement doesn't name"
        fi
    else
        report FAIL "cosign: no flag set accepted the golden statement (output above)"
        failed=$((failed + 1))
    fi
else
    report SKIP "cosign: not on PATH"
fi

# ── securesystemslib (the in-toto reference library) ─────────────────────
sslib_py='
import json, sys
from cryptography.hazmat.primitives import serialization
from securesystemslib.dsse import Envelope
from securesystemslib.signer import SSlibKey
import securesystemslib
print("securesystemslib", securesystemslib.__version__)
pub = serialization.load_pem_public_key(open("node.pem", "rb").read())
key = SSlibKey.from_crypto(pub, keyid=open("keyid").read().strip())
print("key", key.keytype, key.scheme)
for name, want in (("L1.intoto.jsonl", True), ("bad.intoto.jsonl", False)):
    env = Envelope.from_dict(json.loads(open(name).read()))
    try:
        ok = bool(env.verify([key], 1))
    except Exception as e:
        print(name, "refused:", type(e).__name__, e)
        ok = False
    print(name, "verified" if ok else "not verified")
    if ok != want:
        sys.exit(1)
    if ok:
        st = json.loads(env.payload)
        print("payloadType", env.payload_type, "predicateType", st["predicateType"], "subjects", len(st["subject"]))
'
sslib_cmd=""
if command -v uv >/dev/null 2>&1; then
    sslib_cmd="uv run --no-project --with securesystemslib[crypto] python3 -c <script>"
    echo "\$ $sslib_cmd"
    uv run --no-project --with 'securesystemslib[crypto]' python3 -c "$sslib_py"
    rc=$?
elif python3 -c 'import securesystemslib, cryptography' >/dev/null 2>&1; then
    sslib_cmd="python3 -c <script>"
    echo "\$ $sslib_cmd"
    python3 -c "$sslib_py"
    rc=$?
else
    rc=-1
fi
if [ "$rc" = -1 ]; then
    report SKIP "securesystemslib: neither uv nor an importable securesystemslib"
else
    ran=$((ran + 1))
    if [ "$rc" = 0 ]; then
        report PASS "securesystemslib: Envelope.verify([SSlibKey.from_crypto(pem)], 1) accepts the golden statement and refuses a changed payload"
    else
        report FAIL "securesystemslib (output above)"
        failed=$((failed + 1))
    fi
fi

echo "stock verifiers: $ran ran, $failed failed"
[ "$ran" -gt 0 ] && [ "$failed" = 0 ]
