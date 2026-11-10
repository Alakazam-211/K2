#!/usr/bin/env python3
"""Make K2's fixture recording sidecars for the 0.0.2 fixture release (0.46.11, K-a).

- sew-recorder-0.0.2-x86_64-unknown-linux-gnu.tar.gz
- sew-transcriber-0.0.2-x86_64-unknown-linux-gnu.tar.gz
- sew-asr-test-q8_0.gguf (a stand-in speech model, never a real one)
- sew-asr-test-LICENSE.txt (its stand-in licence file, a separate asset
  like Sew's asr-license-*.txt)

The same shape as the sew-media fixture: the binary member (a shell stub
that no test executes) and LICENSE, NOTICE and THIRD_PARTY_NOTICES.md
beside it. Deterministic: ustar, root-owned, mtime 0, gzip mtime 0.

    python3 -I crates/k2-root-proto/testdata/sew/make-recording-fixtures.py

then put the printed sha256s in k2_root_proto::sew's TEST_RELEASES (0.0.2)
and k2_root_proto::sew_recording::TEST_ASR_MODELS.
"""
import gzip, hashlib, io, os, tarfile

V = "0.0.2"
TRIPLE = "x86_64-unknown-linux-gnu"
HERE = os.path.dirname(os.path.abspath(__file__))


def member(tar, name, data, mode):
    info = tarfile.TarInfo(name)
    info.size = len(data)
    info.mode = mode
    info.mtime = 0
    info.uid = info.gid = 0
    info.uname = info.gname = "root"
    tar.addfile(info, io.BytesIO(data))


def sidecar(name):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        stub = f"#!/bin/sh\n# K2 fixture: {name} {V} (never executed)\n".encode()
        member(tar, name, stub, 0o755)
        for n in ("LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md"):
            member(tar, n, b"test\n", 0o644)
    out = io.BytesIO()
    with gzip.GzipFile(fileobj=out, mode="wb", mtime=0) as gz:
        gz.write(raw.getvalue())
    path = os.path.join(HERE, f"{name}-{V}-{TRIPLE}.tar.gz")
    with open(path, "wb") as f:
        f.write(out.getvalue())
    bin_sha = hashlib.sha256(stub).hexdigest()
    print(f"{name}: tarball {hashlib.sha256(out.getvalue()).hexdigest()} binary {bin_sha}")


sidecar("sew-recorder")
sidecar("sew-transcriber")
model = b"GGUF K2 fixture speech model (never a real model)\n" * 64
with open(os.path.join(HERE, "sew-asr-test-q8_0.gguf"), "wb") as f:
    f.write(model)
print(f"model: sha256 {hashlib.sha256(model).hexdigest()} size {len(model)}")
licence = b"K2 fixture: the stand-in speech model's licence text (never a real licence)\n"
with open(os.path.join(HERE, "sew-asr-test-LICENSE.txt"), "wb") as f:
    f.write(licence)
print(f"model licence: sha256 {hashlib.sha256(licence).hexdigest()} size {len(licence)}")
