#!/usr/bin/env python3
"""Writes the CI formats golden vectors (docs/ci-formats-v1.md §10):

  tests/release/fixtures/ci-formats-v1.json               every signed document + an evidence bundle
  tests/release/fixtures/ci-formats-v1-signer-policy.toml a signer policy that accepts that bundle
  tests/release/fixtures/ci-formats-v1-ci.toml            the matching .k2/ci.toml

  python3 -I tests/release/fixtures/make-ci-formats-v1.py

Canonical JSON is k2sign.py's own canonical_json (the Python signer's
form); P-256 signatures are /usr/bin/openssl's; Ed25519 is the RFC 8032
reference (ed25519_ref.py). Re-running keeps the test keys and, for an
unchanged body, the recorded P-256 signature (ECDSA is randomized), so the
files only change when a format changes. Rust (crates/k2-node-proto/tests/
ci_formats_v1.rs) and Python (tests/release/k2sign-ci-formats.sh) both check
these files; neither regenerates them.

TEST KEYS ONLY: the private keys in the JSON are public by design."""

import base64
import hashlib
import importlib.util
import json
import os
import struct
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
OUT = os.path.join(HERE, "ci-formats-v1.json")
OPENSSL = "/usr/bin/openssl"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


sys.dont_write_bytecode = True
k2sign = load("k2sign_for_vectors", os.path.join(REPO, "scripts", "k2sign", "k2sign.py"))
ed = load("ed25519_ref", os.path.join(HERE, "ed25519_ref.py"))
canon = k2sign.canonical_json


def signed_bytes(domain, body):
    raw = body.encode("utf-8")
    return domain.encode() + b"\0" + struct.pack(">I", len(raw)) + raw


def digest(body):
    return "sha256:" + hashlib.sha256(body.encode("utf-8")).hexdigest()


def run(args, data=None):
    p = subprocess.run(args, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if p.returncode != 0:
        raise SystemExit("%s failed: %s" % (" ".join(args), p.stderr.decode(errors="replace")))
    return p.stdout


old = json.load(open(OUT)) if os.path.exists(OUT) else {"keys": {}, "vectors": []}
old_sigs = {(v["domain"], v["body"]): v["sig"] for v in old["vectors"] if v.get("alg") == "p256"}
tmp = tempfile.mkdtemp()


def p256_key(name):
    k = old["keys"].get(name)
    if k is None:
        pem = run([OPENSSL, "ecparam", "-name", "prime256v1", "-genkey", "-noout"])
        pem = run([OPENSSL, "pkcs8", "-topk8", "-nocrypt"], pem).decode()
        k = {"alg": "p256", "private_key_pem": pem}
    path = os.path.join(tmp, name + ".pem")
    open(path, "w").write(k["private_key_pem"])
    der = run([OPENSSL, "ec", "-in", path, "-pubout", "-outform", "DER"])
    k["spki_b64"] = base64.b64encode(der).decode()
    k["public_key_pem"] = k2sign.pem_of_spki(der)
    k["fp"] = hashlib.sha256(der).hexdigest()
    k["path"] = path
    return k


keys = {n: p256_key(n) for n in ("node_a_epoch0", "node_a_epoch1", "node_b", "controller")}
SEED = hashlib.sha256(b"k2-ci-formats-v1 test node-roster key").digest()
ROSTER_PUB = ed.public_key(SEED)
keys["roster"] = {"alg": "ed25519", "seed_hex": SEED.hex(),
                  "public": "ed25519:" + base64.b64encode(ROSTER_PUB).decode(),
                  "fp": hashlib.sha256(ROSTER_PUB).hexdigest()}
OWNER_SERVER_FP = hashlib.sha256(b"k2-ci-formats-v1 owner server").hexdigest()
A0, A1, B, C = keys["node_a_epoch0"], keys["node_a_epoch1"], keys["node_b"], keys["controller"]
R = keys["roster"]

vectors = []


def p256_sign(key, domain, doc, name, kind):
    body = canon(doc)
    sig = old_sigs.get((domain, body))
    if sig is None:
        der = run([OPENSSL, "dgst", "-sha256", "-sign", key["path"]], signed_bytes(domain, body))
        sig = base64.b64encode(der).decode()
    vectors.append({"name": name, "kind": kind, "domain": domain, "alg": "p256", "signer": key_name(key),
                    "body": body, "signed_bytes_hex": signed_bytes(domain, body).hex(),
                    "digest": digest(body), "sig": sig})
    return body, sig


def ed_sign(domain, doc, name, kind):
    body = canon(doc)
    sig = base64.b64encode(ed.sign(SEED, signed_bytes(domain, body))).decode()
    vectors.append({"name": name, "kind": kind, "domain": domain, "alg": "ed25519", "signer": "roster",
                    "body": body, "signed_bytes_hex": signed_bytes(domain, body).hex(),
                    "digest": digest(body), "sig": sig})
    return body, sig


def key_name(key):
    return next(n for n, k in keys.items() if k is key)


def host_id(machine_id):
    return hashlib.sha256(b"k2-node-host-v1\0" + machine_id.encode()).hexdigest()


def statement(key, epoch, prev, born, issued, taint, version, commit, machine):
    return {"v": 1, "node_fp": key["fp"], "node_spki": key["spki_b64"], "key_epoch": epoch, "key_born_at": born,
            "prev_fp": prev, "issued_at": issued, "wall": "job_user", "dev": False, "taint": taint,
            "key_exposure": {"store": "file", "owner": "k2node", "mode": 384, "job_readable": False},
            "k2node_version": version, "k2node_commit": commit, "protocol": 9, "os": "linux", "arch": "x86_64",
            "distro": "ubuntu", "distro_version": "24.04", "kernel": "6.8.0-45-generic", "ptrace_scope": 1,
            "host_id": host_id(machine)}


NODE_COMMIT = "4" * 40
st_a0, st_a0_sig = p256_sign(A0, "k2-node-statement-v1",
                             statement(A0, 0, None, 1800000000, 1800000100, "unknown", "0.46.9", None, "machine-a"),
                             "node_statement_a_epoch0", "node_statement")
rot, rot_sig = p256_sign(A0, "k2-node-rotation-v1",
                         {"v": 1, "old_fp": A0["fp"], "new_fp": A1["fp"], "new_spki": A1["spki_b64"], "new_epoch": 1,
                          "at": 1800000500, "reason": "wall"},
                         "rotation_a", "rotation")
st_a1, st_a1_sig = p256_sign(A1, "k2-node-statement-v1",
                             statement(A1, 1, A0["fp"], 1800000500, 1800000510, "clean", "0.46.10", NODE_COMMIT, "machine-a"),
                             "node_statement_a_epoch1", "node_statement")
st_b, st_b_sig = p256_sign(B, "k2-node-statement-v1",
                           statement(B, 0, None, 1800000000, 1800000200, "unknown", "0.46.10", NODE_COMMIT, "machine-b"),
                           "node_statement_b", "node_statement")
att, att_sig = ed_sign("k2-node-attest-v1",
                       {"v": 1, "owner_id": "owner.example", "roster_key": R["public"], "node_fp": B["fp"],
                        "key_epoch": 0, "statement_digest": digest(st_b), "installed_with_wall": True,
                        "unwalled_jobs_seen": False,
                        "checked": "install log (no --no-job-wall) and the job ledger: no job under wall none (vérifié)",
                        "approved_by": "owner", "at": 1800000300},
                       "owner_attestation_b", "owner_attestation")


def entry(key, name, epoch, st, status, valid_from, valid_to, basis, att_digest, host):
    return {"node_fp": key["fp"], "node_spki": key["spki_b64"], "name": name, "key_epoch": epoch,
            "statement_digest": digest(st), "status": status, "revoked_reason": None, "valid_from": valid_from,
            "valid_to": valid_to, "taint_basis": basis, "attestation_digest": att_digest, "host": host,
            "provider": "provider-x", "site": "site-1", "admin_reach": ["fleet-ssh"], "pod": None, "point": None}


entries = sorted([
    entry(A0, "node-a", 0, st_a0, "retired", 1800000000, 1800000500, None, None, "host-a"),
    entry(A1, "node-a", 1, st_a1, "active", 1800000500, None, "rekeyed", None, "host-a"),
    entry(B, "node-b", 0, st_b, "active", 1800000000, None, "owner_attested", digest(att), "host-b"),
], key=lambda e: e["node_fp"])
roster, roster_sig = ed_sign("k2-node-roster-v1",
                             {"v": 1, "owner_id": "owner.example", "owner_server_fp": OWNER_SERVER_FP,
                              "roster_key": R["public"], "seq": 2, "issued_at": 1800000600,
                              "prev_digest": digest("the owner's roster seq 1"), "entries": entries},
                             "node_roster", "node_roster")
seat, seat_sig = ed_sign("k2-node-seat-v1",
                         {"v": 1, "owner_id": "owner.example", "roster_key": R["public"], "seat_id": "seat-1",
                          "client_fp": C["fp"], "client_label": "client.example", "lane": "ci-standard",
                          "nodes": ["node-a", "node-b"], "roster_seq": 2, "state": "active", "issued_at": 1800000600,
                          "pod": None, "line": None},
                         "seat_statement", "seat_statement")

# ── the build: a commit object, two plans, two v2 receipts ──
TREE = "5" * 40
BASE = "6" * 40
commit_raw = ("tree %s\nparent %s\nauthor ci <ci@example.com> 1800000650 +0000\n"
              "committer ci <ci@example.com> 1800000650 +0000\n\nrelease 1.2.3\n" % (TREE, BASE)).encode()
COMMIT = hashlib.sha1(b"commit %d\0" % len(commit_raw) + commit_raw).hexdigest()
ARGV = ["bash", "scripts/ci/build.sh", "x86_64"]


def plan(job, node):
    return {"job_id": job, "node_id": node, "workspace_id": "ws-client", "workspace_label": "client",
            "requested_by": "agent", "argv": ARGV, "env": {"BUILD_COMMIT": COMMIT, "BUILD_TREE": TREE}, "cwd": None,
            "limits": {"max_secs": 7200, "cpu_millis": 4000, "mem_bytes": 8589934592, "disk_bytes": 107374182400,
                       "log_cap_bytes": 67108864},
            "src": {"project_key": "f" * 64, "remote_url": None, "commit": COMMIT, "dirty": None, "slots": 1},
            "exclusive": False, "created_at": 1800000700, "needs": ["rust@1.99.1"], "artifacts": ["dist/*"],
            "secret_env": ["BUILD_API_KEY"], "tier": "ci", "receipt_v": 2}


ART = {"bytes": 4096, "name": "dist/acme", "sha256": "7" * 64}


def receipt(key, job, p, epoch, st, isolation, uid_index, version, started):
    return {"v": 2, "job_id": job, "attempt": 1, "generation": 1, "plan_digest": digest(canon(p)),
            "node_fp": key["fp"], "controller_fp": C["fp"], "workspace_id": "ws-client",
            "argv_sha256": hashlib.sha256(canon(ARGV).encode()).hexdigest(),
            "env_names": ["BUILD_COMMIT", "BUILD_TREE", "CI", "HOME", "PATH"],
            "src": {"commit": COMMIT, "tree": TREE, "dirty_sha256": None, "slot": 0, "warmth": "private"},
            "tools": {"rustc": "1.99.1"}, "boot_id": "boot-" + key["fp"][:8], "started_at": started,
            "ended_at": started + 600, "state": "done", "exit": {"code": 0, "signal": None},
            "log": {"sha256": "8" * 64, "bytes": 1234, "truncated": False}, "artifacts": [ART],
            "secret_env": ["BUILD_API_KEY"], "tools_used": {"rustc": "1.99.1"},
            "run": {"isolation": isolation, "uid_index": uid_index, "wall": "job_user", "tenant_slot": 1,
                    "k2node_version": version, "k2node_commit": NODE_COMMIT, "key_epoch": epoch,
                    "statement_digest": digest(st), "roster_seq_seen": 2, "lane": "ci-standard"}}


plan_l1, plan_v1 = plan("a" * 32, "node-a"), plan("b" * 32, "node-b")
vectors.append({"name": "plan_receipt_v2", "kind": "plan", "domain": None, "alg": None, "signer": None,
                "body": canon(plan_l1), "signed_bytes_hex": None, "digest": digest(canon(plan_l1)), "sig": None})
rc_l1, rc_l1_sig = p256_sign(A1, "k2-compute-receipt-v1", receipt(A1, "a" * 32, plan_l1, 1, st_a1, "job_uid", 2,
                                                                  "0.46.10", 1800000710),
                             "receipt_v2_l1", "receipt")
rc_v1, rc_v1_sig = p256_sign(B, "k2-compute-receipt-v1", receipt(B, "b" * 32, plan_v1, 0, st_b, "exclusive", None,
                                                                 "0.46.10", 1800000720),
                             "receipt_v2_v1", "receipt")

S = lambda body, sig: {"body": body, "sig": sig}
bundle = {
    "v": 2, "kind": "k2-ci-evidence", "project": "acme-desktop", "version": "1.2.3", "run_id": "run-1",
    "controller_fp": C["fp"], "created_at": 1800001400,
    "source": {"commit": COMMIT, "tree": TREE, "commit_object": base64.b64encode(commit_raw).decode()},
    "receipts": {
        "L1": {"build": "linux-x86_64", "role": "build", "body": rc_l1, "sig": rc_l1_sig, "plan": plan_l1},
        "V1": {"build": "linux-x86_64", "role": "verify", "body": rc_v1, "sig": rc_v1_sig, "plan": plan_v1},
    },
    "statements": [S(st_a1, st_a1_sig), S(st_b, st_b_sig), S(st_a0, st_a0_sig)],
    "rotations": [S(rot, rot_sig)],
    "attestations": [S(att, att_sig)],
    "rosters": [S(roster, roster_sig)],
    "seats": [S(seat, seat_sig)],
    "pairing_pins": {A1["fp"]: A1["spki_b64"], B["fp"]: B["spki_b64"]},
}

for k in keys.values():
    k.pop("path", None)
out = {
    "comment": ("Golden vectors for docs/ci-formats-v1.md (prd-ci-on-compute-for-clients-v1 C-1). TEST KEYS ONLY. "
                "Every body is canonical JSON; sig covers domain || 0x00 || u32be(len(body)) || body. p256: ECDSA "
                "P-256 SHA-256 DER, base64 (randomized: verify it, don't compare it). ed25519: raw 64 bytes, base64 "
                "(deterministic: re-sign with seed_hex and compare). Written by make-ci-formats-v1.py; checked by "
                "crates/k2-node-proto/tests/ci_formats_v1.rs and tests/release/k2sign-ci-formats.sh."),
    "keys": keys,
    "owner_server_fp": OWNER_SERVER_FP,
    "commit_object_b64": base64.b64encode(commit_raw).decode(),
    "vectors": vectors,
    "evidence_bundle": bundle,
}
with open(OUT, "w") as f:
    json.dump(out, f, indent=2, sort_keys=True, ensure_ascii=False)
    f.write("\n")

POLICY = """# A signer policy (docs/ci-formats-v1.md §8) that accepts ci-formats-v1.json's evidence bundle.
version = 1
project = "acme-desktop"
controllers = ["%(controller)s"]
owners = ["%(owner)s"]
workspaces = ["ws-client"]
receipt_max_hours = 24
roster_max_age_hours = 168
roster_changes = "ask"
seat_moves = "ask"
taint_basis = ["owner_attested", "rekeyed", "self_clean"]

[source]
kind = "github_tag"
repo = "acme/desktop"
tag_prefix = "v"

[build.linux-x86_64]
argv = ["bash", "scripts/ci/build.sh", "x86_64"]
needs = ["rust@{rust_pin}"]
artifacts = ["dist/*"]
env = ["BUILD_COMMIT", "BUILD_TREE"]
secrets = ["BUILD_API_KEY"]
isolation = ["job_uid", "exclusive"]
walls = ["job_user"]
dual = ["dist/acme"]
independent = ["host"]

[build.linux-x86_64.secret_sha256]
BUILD_API_KEY = "%(baked)s"

[assets.acme]
key = "updater"
build = "linux-x86_64"
artifact = "dist/acme"
check = "artifact"

[keys.updater]
kind = "minisign"
public = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"
""" % {"controller": C["fp"], "owner": R["fp"], "baked": hashlib.sha256(b"test-api-key").hexdigest()}
open(os.path.join(HERE, "ci-formats-v1-signer-policy.toml"), "w").write(POLICY)

CI = """# A .k2/ci.toml (docs/ci-formats-v1.md §9) matching ci-formats-v1-signer-policy.toml.
version = 1

[watch]
remote = "origin"
branches = ["main", "integ/*"]
poll_secs = 60

[statuses]
token_secret = "K2_CI_STATUSES_TOKEN"
env_name = "CI_STATUSES_TOKEN"
context = "k2-ci"

[checks.unit]
argv = ["bash", "scripts/ci/checks.sh", "unit"]
needs = ["rust@1.99.1"]
os = "linux"
timeout_secs = 3600
cpus = 4
mem_gb = 8

[release]
project = "acme-desktop"
owner = "owner.example"
lane = "ci-standard"

[release.builds.linux-x86_64]
argv = ["bash", "scripts/ci/build.sh", "x86_64"]
needs = ["rust@1.99.1"]
artifacts = ["dist/*"]
env = ["BUILD_COMMIT", "BUILD_TREE"]
secrets = ["BUILD_API_KEY"]
label = "L1"
verify_label = "V1"
"""
open(os.path.join(HERE, "ci-formats-v1-ci.toml"), "w").write(CI)
print("wrote %s (%d vectors), the signer policy and the ci.toml" % (OUT, len(vectors)))
