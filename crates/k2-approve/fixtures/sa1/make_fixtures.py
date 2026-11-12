#!/usr/bin/env python3
"""Make the shared signed-approval fixtures (K2-SIGNED-APPROVAL-1).

Rust (crates/k2-approve/tests/fixtures.rs), Python (k2sign) and Swift (the
phone app) all read what this writes, and must all reach the verdicts in
payloads.json and cases.json. The expected display text for each valid
payload is written here by hand: it IS the wording contract.

    python3 -I crates/k2-approve/fixtures/sa1/make_fixtures.py

The private keys stay out of git (keys/private/*.key is ignored): verifying
needs only the public halves. A run with no private keys makes new ones,
and every key id, record and manifest changes with them; a run with them
keeps the key ids (ECDSA signatures are randomized, so records still change).
Needs openssl on PATH (LibreSSL or OpenSSL).
"""

import base64
import hashlib
import json
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PREFIX = b"K2-SIGNED-APPROVAL-1\x00"
OPENSSL = shutil.which("openssl") or "/usr/bin/openssl"

# name: (person, check, view, kind of key, in approvers.tsv)
KEYS = {
    "alex-phone-bio": ("alex", "biometric", "device", "ec", True),
    "alex-phone-cred": ("alex", "credential", "device", "ec", True),
    "alex-mac-legacy": ("alex", "credential", "host", "ec", True),
    "alex-win-hello": ("alex", "credential", "host", "rsa", True),
    "sam-phone-bio": ("sam", "biometric", "device", "ec", True),
    "untrusted": ("alex", "biometric", "device", "ec", False),
}

ISSUED = "2026-11-11T14:00:00Z"
ANSWER_BY = "2026-11-11T14:15:00Z"
NOW_OK = "2026-11-11T14:05:00Z"
RP = "k2sign@build-mac"
NONCE = "00112233445566778899aabbccddeeff"
BASE = "0123456789abcdef0123456789abcdef01234567"
OLD_BASE = "fedcba9876543210fedcba9876543210fedcba98"


def run(*argv, data=None):
    p = subprocess.run(argv, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if p.returncode != 0:
        sys.exit("%s failed: %s" % (" ".join(argv), p.stderr.decode()))
    return p.stdout


def write(rel, data):
    path = os.path.join(HERE, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data if isinstance(data, bytes) else data.encode())


def make_keys():
    os.makedirs(os.path.join(HERE, "keys", "private"), exist_ok=True)
    tsv = ["# name\tperson\tcheck\tview  (TEST KEYS: the private halves are in private/)"]
    ids = {}
    for name, (person, check, view, kt, listed) in KEYS.items():
        priv = os.path.join(HERE, "keys", "private", name + ".key")
        if not os.path.exists(priv):
            if kt == "ec":
                run(OPENSSL, "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", priv)
            else:
                run(OPENSSL, "genrsa", "-out", priv, "2048")
        pem = run(OPENSSL, "pkey", "-in", priv, "-pubout")
        der = run(OPENSSL, "pkey", "-in", priv, "-pubout", "-outform", "DER")
        ids[name] = hashlib.sha256(der).hexdigest()
        if listed:
            write("keys/%s.pem" % name, pem)
            tsv.append("\t".join((name, person, check, view)))
    write("keys/approvers.tsv", "\n".join(tsv) + "\n")
    return ids


def words(key_id):
    wl = open(os.path.join(HERE, "wordlist.txt")).read().split("\n")[:256]
    assert len(wl) == 256 and len(set(wl)) == 256
    return " ".join(wl[b] for b in bytes.fromhex(key_id)[:6])


def payload_obj(ids, **over):
    o = {
        "kind": "k2.release", "kind_v": 1, "rp": RP, "approver": "alex",
        "approver_keys": [ids["alex-phone-bio"]], "nonce": NONCE,
        "issued": ISSUED, "answer_by": ANSWER_BY,
        "body": {
            "version": "0.46.13", "base_commit": BASE,
            "assets": ["K2.app.tar.gz", "K2_0.46.13_aarch64.dmg", "k2-daemon-linux-x86_64"],
            "slots": ["u1", "u2"], "windows": False, "reapproval": False,
            "replacement": False, "replaces_base": "",
            "max_signatures": 10, "session_until": "2026-11-12T02:00:00Z",
        },
    }
    o.update(over)
    return o


def enc(o, pretty=False):
    if pretty:
        return PREFIX + json.dumps(o, indent=2, ensure_ascii=False).encode()
    return PREFIX + json.dumps(o, separators=(",", ":"), ensure_ascii=False).encode()


FOOTER = "\nWho will act: k2sign@build-mac\nAsked: alex\nAnswer by: 2026-11-11 14:15:00 UTC\n"


def valid_payloads(ids):
    out = {}
    out["release"] = (enc(payload_obj(ids)),
        "Approve K2 release 0.46.13 at base commit %s.\n"
        "Sign: K2.app.tar.gz, K2_0.46.13_aarch64.dmg, k2-daemon-linux-x86_64.\n"
        "Windows setup: no.\n"
        "Signing stays open until 2026-11-12 02:00:00 UTC.\n" % BASE + FOOTER)
    o = payload_obj(ids)
    o["approver_keys"] = [ids["alex-phone-bio"], ids["alex-phone-cred"]]
    o["body"] = dict(o["body"], windows=True, reapproval=True,
                     assets=["K2.app.tar.gz", "K2_0.46.13_x64-setup.exe"], max_signatures=8)
    out["release-reapprove-windows"] = (enc(o),
        "RE-APPROVE: Approve K2 release 0.46.13 at base commit %s.\n"
        "Sign: K2.app.tar.gz, K2_0.46.13_x64-setup.exe.\n"
        "Windows setup: yes, with no independent check.\n"
        "Signing stays open until 2026-11-12 02:00:00 UTC.\n" % BASE + FOOTER)
    o = payload_obj(ids)
    o["body"] = dict(o["body"], replacement=True, replaces_base=OLD_BASE)
    out["release-replacement"] = (enc(o),
        "RE-APPROVE for new code: the earlier approval for base commit %s was never used.\n"
        "Approve K2 release 0.46.13 at base commit %s.\n"
        "Sign: K2.app.tar.gz, K2_0.46.13_aarch64.dmg, k2-daemon-linux-x86_64.\n"
        "Windows setup: no.\n"
        "Signing stays open until 2026-11-12 02:00:00 UTC.\n" % (OLD_BASE, BASE) + FOOTER)
    o = payload_obj(ids)
    o["body"] = dict(o["body"], replacement=True)
    out["release-replacement-old-base-unknown"] = (enc(o),
        "RE-APPROVE for new code: an earlier approval of 0.46.13 was never used.\n"
        "Approve K2 release 0.46.13 at base commit %s.\n"
        "Sign: K2.app.tar.gz, K2_0.46.13_aarch64.dmg, k2-daemon-linux-x86_64.\n"
        "Windows setup: no.\n"
        "Signing stays open until 2026-11-12 02:00:00 UTC.\n" % BASE + FOOTER)
    refund = payload_obj(ids, kind="com.example.refund", kind_v=2,
                         approver_keys=[ids["alex-phone-cred"], ids["alex-phone-bio"]],
                         body={"order": "A-1042", "amount": "125.00 USD", "items": ["blue mug", "tea set"],
                               "notify_customer": True, "count": 3, "tags": [], "note": "Ünïcode is fine 😀",
                               "balance_change": -125})
    generic = ("K2 has no built-in wording for this kind. Every field is shown below.\n"
               "Kind: com.example.refund (version 2)\n"
               "amount: 125.00 USD\n"
               "balance_change: -125\n"
               "count: 3\n"
               "items:\n  - blue mug\n  - tea set\n"
               "note: Ünïcode is fine 😀\n"
               "notify_customer: true\n"
               "order: A-1042\n"
               "tags: (none)\n" + FOOTER)
    out["generic-refund"] = (enc(refund), generic)
    out["generic-refund-pretty"] = (enc(refund, pretty=True), generic)
    # Escapes are fine when what they spell is allowed.
    esc = (PREFIX + b'{"kind":"com.example.note","kind_v":1,"rp":"k2sign@build-mac","approver":"alex",'
           b'"approver_keys":["' + ids["alex-phone-bio"].encode() + b'"],"nonce":"' + NONCE.encode() +
           b'","issued":"' + ISSUED.encode() + b'","answer_by":"' + ANSWER_BY.encode() +
           b'","body":{"text":"caf\\u00e9 \\"quoted\\" a\\/b \\\\ \\ud83d\\ude00"}}')
    out["generic-escapes"] = (esc,
        "K2 has no built-in wording for this kind. Every field is shown below.\n"
        "Kind: com.example.note (version 1)\n"
        'text: café "quoted" a/b \\ 😀\n' + FOOTER)
    return out


def invalid_payloads(ids):
    good = enc(payload_obj(ids))
    js = good[len(PREFIX):]
    o = lambda **kw: enc(payload_obj(ids, **kw))
    body = lambda **kw: dict(payload_obj(ids)["body"], **kw)
    gen = lambda b: enc(payload_obj(ids, kind="com.example.x", body=b))
    cases = [
        ("prefix-missing", js, "prefix"),
        ("prefix-version-2", b"K2-SIGNED-APPROVAL-2\x00" + js, "prefix"),
        ("prefix-no-zero-byte", b"K2-SIGNED-APPROVAL-1" + js, "prefix"),
        ("too-large", enc(payload_obj(ids, kind="com.example.x", body={"pad": "x" * 16400})), "too_large"),
        ("utf8-bad-byte", good.replace(b'"alex"', b'"al\xffx"'), "utf8"),
        ("json-trailing-text", good + b" x", "json"),
        ("json-cut-short", good[:-1], "json"),
        ("json-bom", PREFIX + b"\xef\xbb\xbf" + js, "json"),
        ("json-nan", gen({"n": 1}).replace(b'"n":1', b'"n":NaN'), "json"),
        ("not-object", PREFIX + b"[" + js + b"]", "not_object"),
        ("duplicate-top-key", good.replace(b'"approver":"alex"', b'"approver":"alex","approver":"sam"'), "duplicate_key"),
        ("duplicate-key-by-escape", good.replace(b'"approver":"alex"', b'"approver":"alex","appro\\u0076er":"sam"'), "duplicate_key"),
        ("duplicate-body-key", good.replace(b'"windows":false', b'"windows":false,"windows":true'), "duplicate_key"),
        ("char-rlo-raw", gen({"note": "pay ‮moc.live"}), "forbidden_char"),
        ("char-rlo-escaped", gen({"note": "x"}).replace(b'"note":"x"', b'"note":"pay \\u202emoc"'), "forbidden_char"),
        ("char-zero-width-in-approver", good.replace(b'"alex"', '"al​ex"'.encode()), "forbidden_char"),
        ("char-newline-escaped", gen({"note": "x"}).replace(b'"note":"x"', b'"note":"a\\nb"'), "forbidden_char"),
        ("char-newline-raw", gen({"note": "x"}).replace(b'"note":"x"', b'"note":"a\nb"'), "forbidden_char"),
        ("char-lone-surrogate", gen({"note": "x"}).replace(b'"note":"x"', b'"note":"\\ud800"'), "forbidden_char"),
        ("char-in-key", gen({"note": "x"}).replace(b'"note":"x"', b'"no\\u200bte":"x"'), "forbidden_char"),
        ("char-variation-selector", gen({"note": "ok️"}), "forbidden_char"),
        ("char-tag", gen({"note": "ok\U000e0041"}), "forbidden_char"),
        ("int-fraction", gen({"n": 1}).replace(b'"n":1', b'"n":1.5'), "not_integer"),
        ("int-exponent", gen({"n": 1}).replace(b'"n":1', b'"n":1e3'), "not_integer"),
        ("int-minus-zero", gen({"n": 1}).replace(b'"n":1', b'"n":-0'), "not_integer"),
        ("int-too-big", gen({"n": 1}).replace(b'"n":1', b'"n":9007199254740992'), "not_integer"),
        ("int-kind-v-1.0", good.replace(b'"kind_v":1', b'"kind_v":1.0'), "not_integer"),
        ("unknown-field", o(requested_by="agent-x"), "unknown_field"),
        ("missing-answer-by", enc({k: v for k, v in payload_obj(ids).items() if k != "answer_by"}), "missing_field"),
        ("type-kind-v-string", o(kind_v="1"), "field_type"),
        ("type-approver-keys-string", o(approver_keys=ids["alex-phone-bio"]), "field_type"),
        ("type-body-list", o(body=["x"]), "field_type"),
        ("type-rp-null", o(rp=None), "field_type"),
        ("format-kind-capitals", o(kind="K2.Release"), "field_format"),
        ("format-kind-one-part", o(kind="release"), "field_format"),
        ("format-nonce-uppercase", o(nonce=NONCE.upper()), "field_format"),
        ("format-issued-fraction", o(issued="2026-11-11T14:00:00.5Z"), "field_format"),
        ("format-issued-offset", o(issued="2026-11-11T14:00:00+00:00"), "field_format"),
        ("format-answer-by-feb-30", o(answer_by="2026-02-30T00:00:00Z", issued="2026-02-01T00:00:00Z"), "field_format"),
        ("format-approver-keys-empty", o(approver_keys=[]), "field_format"),
        ("format-approver-keys-17", o(approver_keys=["%064x" % i for i in range(17)]), "field_format"),
        ("format-approver-keys-twice", o(approver_keys=[ids["alex-phone-bio"]] * 2), "field_format"),
        ("format-approver-key-short", o(approver_keys=["abc"]), "field_format"),
        ("format-approver-empty", o(approver=""), "field_format"),
        ("format-rp-too-long", o(rp="r" * 129), "field_format"),
        ("format-body-key", gen({"Bad-Key": "x"}), "field_format"),
        ("body-object", gen({"nested": {"a": "b"}}), "body_value"),
        ("body-list-of-ints", gen({"l": ["a", 1]}), "body_value"),
        ("body-null", gen({"n": None}), "body_value"),
        ("times-answer-by-equals-issued", o(answer_by=ISSUED), "times"),
        ("times-answer-by-before-issued", o(answer_by="2026-11-11T13:59:59Z"), "times"),
        ("release-kind-v-2", o(kind_v=2), "shape"),
        ("release-extra-field", o(body=body(project="other")), "shape"),
        ("release-missing-session-until", o(body={k: v for k, v in body().items() if k != "session_until"}), "shape"),
        ("release-short-base", o(body=body(base_commit="0123456")), "shape"),
        ("release-bad-asset-name", o(body=body(assets=["../etc/passwd"])), "shape"),
        ("release-asset-twice", o(body=body(assets=["K2.app.tar.gz", "K2.app.tar.gz"])), "shape"),
        ("release-windows-string", o(body=body(windows="no")), "shape"),
        ("release-version-leading-zero", o(body=body(version="0.046.13")), "shape"),
        ("release-replacement-and-reapproval", o(body=body(replacement=True, reapproval=True)), "shape"),
        ("release-replaces-base-without-replacement", o(body=body(replaces_base=OLD_BASE)), "shape"),
        ("release-replaces-base-short", o(body=body(replacement=True, replaces_base="fedcba9")), "shape"),
        ("release-missing-replacement", o(body={k: v for k, v in body().items() if k != "replacement"}), "shape"),
    ]
    names = [c[0] for c in cases]
    assert len(names) == len(set(names))
    return cases


def sign(name, data):
    priv = os.path.join(HERE, "keys", "private", name + ".key")
    tmp = os.path.join(HERE, ".tmp-sign")
    with open(tmp, "wb") as f:
        f.write(data)
    try:
        return run(OPENSSL, "dgst", "-sha256", "-sign", priv, tmp)
    finally:
        os.unlink(tmp)


def record(payload, key_id, sig, alg="ES256"):
    return json.dumps({"v": 1, "payload_b64": base64.b64encode(payload).decode(), "key_id": key_id,
                       "alg": alg, "sig_b64": base64.b64encode(sig).decode()}, separators=(",", ":"))


def main():
    ids = make_keys()
    for d in ("payloads", "records"):
        shutil.rmtree(os.path.join(HERE, d), ignore_errors=True)

    manifest = {"valid": [], "invalid": []}
    valid = valid_payloads(ids)
    for name, (data, text) in valid.items():
        write("payloads/valid/%s.bin" % name, data)
        write("payloads/valid/%s.txt" % name, text)
        manifest["valid"].append({"name": name, "payload": "payloads/valid/%s.bin" % name,
                                  "display": "payloads/valid/%s.txt" % name})
    for name, data, code in invalid_payloads(ids):
        write("payloads/invalid/%s.bin" % name, data)
        manifest["invalid"].append({"name": name, "payload": "payloads/invalid/%s.bin" % name, "code": code})
    write("payloads.json", json.dumps(manifest, indent=1) + "\n")

    keys_out = {name: {"key_id": kid, "words": words(kid), "trusted": KEYS[name][4],
                       "alg": "RS256" if KEYS[name][3] == "rsa" else "ES256"} for name, kid in ids.items()}
    write("keys.json", json.dumps(keys_out, indent=1, sort_keys=True) + "\n")

    cases = []

    def req(**kw):
        r = {"rp": RP, "kind": "k2.release", "check": "biometric", "view": "device", "now": NOW_OK, "nonce": NONCE}
        r.update(kw)
        return r

    def case(name, key, payload, expect, require=None, stored="same", sig=None, alg=None, rec=None, used=()):
        if rec is None:
            s = sig if sig is not None else sign(key, payload)
            rec = record(payload, ids[key], s, alg or ("RS256" if KEYS[key][3] == "rsa" else "ES256"))
        write("records/%s.json" % name, rec)
        st = None
        if stored == "same":
            st = payload
        elif stored is not None:
            st = stored
        stored_path = None
        if st is not None:
            stored_path = "records/%s.stored.bin" % name
            write(stored_path, st)
        cases.append({"name": name, "record": "records/%s.json" % name, "stored_payload": stored_path,
                      "require": require or req(), "used_nonces": list(used), "expect": expect})

    def ok(key):
        person, check, view = KEYS[key][:3]
        return {"ok": True, "key_name": key, "person": person, "key_id": ids[key], "check": check, "view": view}

    rel = valid["release"][0]
    both = valid["release-reapprove-windows"][0]
    refund = valid["generic-refund"][0]
    case("ok-release-phone-bio", "alex-phone-bio", rel, ok("alex-phone-bio"))
    case("ok-release-no-stored-copy", "alex-phone-bio", rel, ok("alex-phone-bio"), stored=None)
    case("ok-release-phone-cred-everyday", "alex-phone-cred", both, ok("alex-phone-cred"), require=req(check="credential"))
    case("ok-generic-refund", "alex-phone-cred", refund, ok("alex-phone-cred"),
         require=req(kind="com.example.refund", check="credential"))
    case("ok-generic-pretty", "alex-phone-bio", valid["generic-refund-pretty"][0], ok("alex-phone-bio"),
         require=req(kind="com.example.refund"))
    mac = enc(payload_obj(ids, approver_keys=[ids["alex-mac-legacy"]]))
    case("ok-mac-host-when-allowed", "alex-mac-legacy", mac, ok("alex-mac-legacy"), require=req(check="credential", view="host"))
    win = enc(payload_obj(ids, approver_keys=[ids["alex-win-hello"]]))
    case("ok-rs256-host-when-allowed", "alex-win-hello", win, ok("alex-win-hello"), require=req(check="credential", view="host"))
    case("ok-no-optional-checks", "alex-phone-bio", rel, ok("alex-phone-bio"),
         require={"rp": None, "kind": None, "check": "credential", "view": "page", "now": NOW_OK, "nonce": None})

    no = lambda code: {"ok": False, "code": code}
    # A one-byte change: the signature no longer verifies.
    changed = rel.replace(b"0.46.13", b"0.46.14", 1)
    s = sign("alex-phone-bio", rel)
    case("refuse-one-byte-changed", "alex-phone-bio", changed, no("bad_signature"), stored=None, sig=s)
    case("refuse-bytes-not-the-stored-ones", "alex-phone-bio", changed, no("payload_mismatch"), stored=rel, sig=s)
    bad_sig = bytearray(sign("alex-phone-bio", rel))
    bad_sig[-1] ^= 0x01
    case("refuse-signature-byte-flipped", "alex-phone-bio", rel, no("bad_signature"), sig=bytes(bad_sig))
    case("refuse-signed-by-other-key-same-id", "alex-phone-bio", rel, no("bad_signature"), sig=sign("alex-phone-cred", rel))
    case("refuse-untrusted-key", "untrusted", enc(payload_obj(ids, approver_keys=[ids["untrusted"]])), no("unknown_key"))
    case("refuse-alg-mismatch", "alex-phone-bio", rel, no("alg_mismatch"), alg="RS256")
    case("refuse-key-not-listed", "alex-phone-cred", rel, no("key_not_listed"), require=req(check="credential"))
    sam = enc(payload_obj(ids, approver_keys=[ids["sam-phone-bio"]]))
    case("refuse-key-of-another-person", "sam-phone-bio", sam, no("wrong_approver"))
    case("refuse-wrong-rp", "alex-phone-bio", rel, no("wrong_rp"), require=req(rp="k2sign@other-mac"))
    case("refuse-wrong-kind", "alex-phone-bio", rel, no("wrong_kind"), require=req(kind="k2.delete"))
    case("refuse-credential-for-release", "alex-phone-cred", both, no("weak_check"))
    case("refuse-mac-legacy-for-release", "alex-mac-legacy", mac, no("weak_check"))
    case("refuse-host-view-for-release", "alex-mac-legacy", mac, no("weak_view"), require=req(check="credential"))
    case("refuse-expired", "alex-phone-bio", rel, no("expired"), require=req(now="2026-11-11T14:15:01Z"))
    case("ok-at-answer-by", "alex-phone-bio", rel, ok("alex-phone-bio"), require=req(now=ANSWER_BY))
    case("refuse-issued-in-future", "alex-phone-bio", rel, no("issued_future"), require=req(now="2026-11-11T13:54:59Z"))
    case("ok-issued-within-skew", "alex-phone-bio", rel, ok("alex-phone-bio"), require=req(now="2026-11-11T13:55:00Z"))
    case("refuse-wrong-nonce", "alex-phone-bio", rel, no("wrong_nonce"), require=req(nonce="ffeeddccbbaa99887766554433221100"))
    case("refuse-nonce-used", "alex-phone-bio", rel, no("nonce_used"), used=[NONCE])
    dup = rel.replace(b'"windows":false', b'"windows":false,"windows":true')
    case("refuse-signed-duplicate-key", "alex-phone-bio", dup, no("duplicate_key"))
    rlo = enc(payload_obj(ids, kind="com.example.x", body={"note": "pay ‮moc"}))
    case("refuse-signed-hidden-char", "alex-phone-bio", rlo, no("forbidden_char"), require=req(kind="com.example.x"))
    shp = enc(payload_obj(ids, body={k: v for k, v in payload_obj(ids)["body"].items() if k != "session_until"}))
    case("refuse-signed-release-bad-shape", "alex-phone-bio", shp, no("shape"))
    good_rec = json.loads(record(rel, ids["alex-phone-bio"], sign("alex-phone-bio", rel)))
    extra = dict(good_rec, check="biometric")
    case("refuse-record-claims-a-check", None, rel, no("record"), rec=json.dumps(extra, separators=(",", ":")))
    case("refuse-record-duplicate-field", None, rel, no("record"),
         rec=record(rel, ids["alex-phone-bio"], sign("alex-phone-bio", rel)).replace('{"v":1,', '{"v":1,"v":1,'))
    case("refuse-record-v2", None, rel, no("record"), rec=json.dumps(dict(good_rec, v=2), separators=(",", ":")))
    case("refuse-record-base64-spaces", None, rel, no("record"),
         rec=json.dumps(dict(good_rec, sig_b64=good_rec["sig_b64"][:8] + " " + good_rec["sig_b64"][8:]), separators=(",", ":")))
    case("refuse-record-unknown-alg", None, rel, no("record"), rec=json.dumps(dict(good_rec, alg="ES384"), separators=(",", ":")))

    names = [c["name"] for c in cases]
    assert len(names) == len(set(names))
    write("cases.json", json.dumps({"keys": "keys", "cases": cases}, indent=1) + "\n")
    print("wrote %d valid and %d invalid payloads, %d record cases" % (len(manifest["valid"]), len(manifest["invalid"]), len(cases)))


if __name__ == "__main__":
    main()
