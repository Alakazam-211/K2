#!/usr/bin/env python3
"""A stub `minisign` for the packaging tests (tests/packaging/updater-signing.sh).

NOT cryptography: the "signature" is an HMAC keyed by bytes the public key
carries, so anyone can forge it. It exists to exercise the release
plumbing exactly as real minisign is driven: the CLI flags release
scripts use, the file formats (key ids, the ED prehashed algorithm tag,
untrusted/trusted comment lines, the global signature), the password read
from stdin (never argv), and a refusal on a wrong password, a wrong key or
a changed file or comment. Real minisign interop was checked by hand on
2026-10-24 (see the SS1 commit message).

Every call appends {"argv": [...], "env": {...}} as one JSON line to $STUB_MINISIGN_LOG, so tests can prove the password
and the key never appear on argv or in the environment.
"""

import base64
import hashlib
import hmac
import json
import os
import sys


def log():
    p = os.environ.get("STUB_MINISIGN_LOG")
    if p:
        with open(p, "a") as f:
            f.write(json.dumps({"argv": sys.argv[1:], "env": dict(os.environ)}) + "\n")


def opt(args, flag):
    if flag in args:
        i = args.index(flag)
        return args[i + 1]
    return None


def read_password():
    line = sys.stdin.readline()
    return line.rstrip("\n")


def load_secret(path):
    with open(path) as f:
        lines = f.read().splitlines()
    if not lines or not lines[0].startswith("untrusted comment:"):
        sys.exit("stub minisign: unsupported secret key format")
    return json.loads(base64.b64decode(lines[1]))


def load_pub(args):
    if opt(args, "-P"):
        raw = base64.b64decode(opt(args, "-P"))
    else:
        with open(opt(args, "-p")) as f:
            raw = base64.b64decode(f.read().splitlines()[1])
    assert raw[:2] == b"Ed", "not a public key"
    return raw[2:10], raw[10:42]


def mac(pk, data):
    return hmac.new(pk, data, hashlib.sha512).digest()


def main():
    log()
    a = sys.argv[1:]
    if "-v" in a:
        print("minisign " + os.environ.get("STUB_MINISIGN_VERSION", "0.12"))
        return 0
    if "-G" in a:
        pw = "" if "-W" in a else read_password()
        keyid, pk = os.urandom(8), os.urandom(32)
        sk = {"keyid": keyid.hex(), "pk": pk.hex(), "pw": hashlib.sha256(pw.encode()).hexdigest()}
        with open(opt(a, "-s"), "w") as f:
            f.write("untrusted comment: stub encrypted secret key\n" + base64.b64encode(json.dumps(sk).encode()).decode() + "\n")
        with open(opt(a, "-p"), "w") as f:
            f.write("untrusted comment: minisign public key: %s\n" % keyid[::-1].hex().upper())
            f.write(base64.b64encode(b"Ed" + keyid + pk).decode() + "\n")
        return 0
    if "-S" in a:
        sk = load_secret(opt(a, "-s"))
        sys.stdout.write("Password: ")
        pw = read_password()
        if hashlib.sha256(pw.encode()).hexdigest() != sk["pw"]:
            print("Deriving a key from the password and decrypting the secret key... Wrong password for that key")
            return 2
        keyid, pk = bytes.fromhex(sk["keyid"]), bytes.fromhex(sk["pk"])
        algo = b"Ed" if "-l" in a else b"ED"
        with open(opt(a, "-m"), "rb") as f:
            data = f.read()
        msg = hashlib.blake2b(data).digest() if algo == b"ED" else data
        sig = mac(pk, algo + msg)
        tc = opt(a, "-t") or "timestamp:0"
        uc = opt(a, "-c") or "signature from minisign secret key"
        glob = mac(pk, sig + tc.encode())
        with open(opt(a, "-x"), "w") as f:
            f.write("untrusted comment: %s\n%s\ntrusted comment: %s\n%s\n" % (
                uc, base64.b64encode(algo + keyid + sig).decode(), tc, base64.b64encode(glob).decode()))
        print("Deriving a key from the password and decrypting the secret key... done")
        return 0
    if "-V" in a:
        keyid, pk = load_pub(a)
        with open(opt(a, "-x")) as f:
            lines = f.read().splitlines()
        with open(opt(a, "-m"), "rb") as f:
            data = f.read()
        try:
            raw = base64.b64decode(lines[1])
            algo, sid, sig = raw[:2], raw[2:10], raw[10:]
            tc = lines[2][len("trusted comment: "):]
            glob = base64.b64decode(lines[3])
        except Exception:
            print("Invalid signature file", file=sys.stderr)
            return 1
        if sid != keyid:
            print("Signature key id in %s is %s but the key id in the public key is %s" % (
                opt(a, "-x"), sid[::-1].hex().upper(), keyid[::-1].hex().upper()), file=sys.stderr)
            return 1
        msg = hashlib.blake2b(data).digest() if algo == b"ED" else data
        if not hmac.compare_digest(sig, mac(pk, algo + msg)):
            print("Signature verification failed", file=sys.stderr)
            return 1
        if not hmac.compare_digest(glob, mac(pk, sig + tc.encode())):
            print("Comment signature verification failed", file=sys.stderr)
            return 1
        if "-Q" in a:
            print(tc)
        elif "-q" not in a:
            print("Signature and comment signature verified\nTrusted comment: " + tc)
        return 0
    print("stub minisign: unsupported call %r" % a, file=sys.stderr)
    return 2


sys.exit(main())
