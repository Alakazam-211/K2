#!/usr/bin/env bash
# k2sign-ci-formats.sh — the CI formats golden vectors (docs/ci-formats-v1.md
# §10) checked from the Python side, against the same file Rust checks
# (crates/k2-node-proto/tests/ci_formats_v1.rs):
#   - every body is k2sign.py's own canonical JSON of itself;
#   - the signed bytes are domain || 0 || u32be(len) || body;
#   - P-256 signatures verify with /usr/bin/openssl, Ed25519 ones with the
#     RFC 8032 reference (and re-sign to the same bytes);
#   - the documents link up (digests, plan digests, the commit object);
#   - and k2sign (the S3b path) refuses receipt v2 by name: a v2 bundle, a
#     v2 receipt, a plan asking for v2 (V4/V7: S3b stays v1 only).
# No keys, no network, no install: plain python3 -I and /usr/bin/openssl.
#   bash tests/release/k2sign-ci-formats.sh
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
[ -x /usr/bin/openssl ] || { echo "FAIL: /usr/bin/openssl missing" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 missing" >&2; exit 1; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

python3 -I - "$REPO" "$tmp" <<'PY'
import base64, hashlib, importlib.util, json, os, struct, subprocess, sys
sys.dont_write_bytecode = True
repo, tmp = sys.argv[1], sys.argv[2]

def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
    return m

k2sign = load("k2sign_ci_formats", os.path.join(repo, "scripts/k2sign/k2sign.py"))
ed = load("ed25519_ref", os.path.join(repo, "tests/release/fixtures/ed25519_ref.py"))
V = json.load(open(os.path.join(repo, "tests/release/fixtures/ci-formats-v1.json")))
passed = 0

def ok(what):
    global passed
    passed += 1
    print("  ok  " + what)

# RFC 8032 test 1: the reference implementation itself.
sk = bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
assert ed.public_key(sk).hex() == "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
assert ed.sign(sk, b"").hex().startswith("e5564300c360ac729086e2cc806e828a84877f1e")
ok("Ed25519 reference matches RFC 8032 test 1")

def signed_bytes(domain, body):
    raw = body.encode("utf-8")
    return domain.encode() + b"\0" + struct.pack(">I", len(raw)) + raw

def p256_ok(spki_der, msg, sig_b64):
    pub, m, s = (os.path.join(tmp, n) for n in ("pub.pem", "msg", "sig"))
    open(pub, "w").write(k2sign.pem_of_spki(spki_der)); open(m, "wb").write(msg)
    open(s, "wb").write(base64.b64decode(sig_b64, validate=True))
    p = subprocess.run(["/usr/bin/openssl", "dgst", "-sha256", "-verify", pub, "-signature", s, m],
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return p.returncode == 0 and p.stdout.strip() == b"Verified OK"

keys = V["keys"]
for n in ("node_a_epoch0", "node_a_epoch1", "node_b", "controller"):
    der = k2sign.spki_from_pem(keys[n]["public_key_pem"])
    assert base64.b64encode(der).decode() == keys[n]["spki_b64"] and hashlib.sha256(der).hexdigest() == keys[n]["fp"], n
seed = bytes.fromhex(keys["roster"]["seed_hex"])
rpub = ed.public_key(seed)
assert keys["roster"]["public"] == "ed25519:" + base64.b64encode(rpub).decode()
assert keys["roster"]["fp"] == hashlib.sha256(rpub).hexdigest()
ok("test keys: P-256 fingerprints and the roster key")

by = {v["name"]: v for v in V["vectors"]}
for name, v in sorted(by.items()):
    body = v["body"]
    assert k2sign.canonical_json(json.loads(body)) == body, name + ": not k2sign's canonical JSON"
    assert v["digest"] == "sha256:" + hashlib.sha256(body.encode()).hexdigest(), name
    if v["alg"] is None:
        continue
    sb = signed_bytes(v["domain"], body)
    assert sb.hex() == v["signed_bytes_hex"], name + ": signed bytes"
    if v["alg"] == "p256":
        der = base64.b64decode(keys[v["signer"]]["spki_b64"])
        assert p256_ok(der, sb, v["sig"]), name + ": openssl does not verify"
        assert not p256_ok(der, sb + b" ", v["sig"]), name + ": a changed message verifies"
    else:
        sig = base64.b64decode(v["sig"], validate=True)
        assert ed.verify(rpub, sb, sig), name + ": Ed25519 does not verify"
        assert ed.sign(seed, sb) == sig, name + ": Ed25519 is not deterministic here"
        assert not ed.verify(rpub, sb + b" ", sig), name
    ok("%s (%s, %s): canonical, signed bytes, signature" % (name, v["kind"], v["domain"]))

# The links the signer will check.
J = lambda n: json.loads(by[n]["body"])
D = lambda n: by[n]["digest"]
roster, seat, att = J("node_roster"), J("seat_statement"), J("owner_attestation_b")
ent = {e["node_fp"]: e for e in roster["entries"]}
assert ent[keys["node_a_epoch1"]["fp"]]["statement_digest"] == D("node_statement_a_epoch1")
assert ent[keys["node_b"]["fp"]]["attestation_digest"] == D("owner_attestation_b")
assert att["statement_digest"] == D("node_statement_b")
rot = J("rotation_a")
assert (rot["old_fp"], rot["new_fp"]) == (keys["node_a_epoch0"]["fp"], keys["node_a_epoch1"]["fp"])
assert seat["client_fp"] == keys["controller"]["fp"] and seat["roster_seq"] == roster["seq"]
plan = J("plan_receipt_v2")
assert plan["receipt_v"] == 2 and set(plan) <= k2sign.PLAN_KEYS
l1 = J("receipt_v2_l1")
assert l1["v"] == 2 and l1["plan_digest"] == D("plan_receipt_v2")
assert l1["run"]["statement_digest"] == D("node_statement_a_epoch1")
b = V["evidence_bundle"]
raw = base64.b64decode(b["source"]["commit_object"])
sha, tree, parents = k2sign.parse_commit_object(raw)
assert sha == b["source"]["commit"] == l1["src"]["commit"] and tree == b["source"]["tree"]
for label, r in b["receipts"].items():
    rc = json.loads(r["body"])
    assert "sha256:" + hashlib.sha256(k2sign.canonical_json(r["plan"]).encode()).hexdigest() == rc["plan_digest"], label
    assert k2sign.receipt_signature_ok(type("C", (), {"own": lambda s, p: None})(), base64.b64decode(b["pairing_pins"][rc["node_fp"]]),
                                       r["body"], r["sig"], tmp), label
ok("documents link: roster, statements, attestation, rotation, seat, plan, receipts, commit object")

# V4/V7: the S3b signer refuses v2 by name, never by accident.
try:
    k2sign.compute_bundle_receipts(b)
    raise AssertionError("k2sign took a v2 bundle")
except k2sign.Refused as e:
    assert "not a v1 bundle" in str(e), e
try:
    k2sign.check_plan("L1", plan, l1, {}, plan["argv"], plan["needs"])
    raise AssertionError("k2sign took a plan asking for receipt v2")
except k2sign.Refused as e:
    assert "receipt v2" in str(e), e
plan_v1 = dict(plan); del plan_v1["receipt_v"]
assert "receipt_v" not in k2sign.canonical_json(plan_v1)
ok("k2sign refuses a v2 bundle and a plan asking for receipt v2 (S3b stays v1)")
print("k2sign-ci-formats: %d passed" % passed)
PY
