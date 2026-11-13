#!/usr/bin/env python3
"""Writes the build statement golden vectors (docs/provenance-v1.md):

  tests/release/fixtures/provenance-v1.json

  python3 -I tests/release/fixtures/make-provenance-v1.py

Each vector holds a plan, the node-signed receipt for it, the facts only the
node has, and the statement those must give, written out here by hand from
the spec (not from the Rust code), then its canonical payload, the DSSE PAE
and an envelope signed by /usr/bin/openssl. crates/k2-node-proto/tests/
provenance_v1.rs builds the statement from the same inputs and must produce
exactly this payload, and must verify this envelope.

Canonical JSON: keys sorted, no whitespace, ASCII only in these vectors.
Re-running keeps the test key and, for an unchanged body, the recorded
P-256 signatures (ECDSA is randomized), so the file only changes when a
format changes.

TEST KEYS ONLY: the private key in the JSON is public by design."""

import base64
import hashlib
import json
import os
import struct
import subprocess
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "provenance-v1.json")
OPENSSL = "/usr/bin/openssl"

PAYLOAD_TYPE = "application/vnd.in-toto+json"
RECEIPT_DOMAIN = "k2-compute-receipt-v1"


def canon(v):
    return json.dumps(v, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def sha(b):
    return hashlib.sha256(b).hexdigest()


def run(args, data=None):
    p = subprocess.run(args, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if p.returncode != 0:
        raise SystemExit("%s failed: %s" % (" ".join(args), p.stderr.decode(errors="replace")))
    return p.stdout


def signed_bytes(domain, body):
    raw = body.encode("utf-8")
    return domain.encode() + b"\0" + struct.pack(">I", len(raw)) + raw


def pae(payload_type, payload):
    t = payload_type.encode()
    return b"DSSEv1 %d %s %d " % (len(t), t, len(payload)) + payload


def pem_of_spki(der):
    b = base64.b64encode(der).decode()
    return "-----BEGIN PUBLIC KEY-----\n" + "".join(b[i:i + 64] + "\n" for i in range(0, len(b), 64)) + "-----END PUBLIC KEY-----\n"


old = json.load(open(OUT)) if os.path.exists(OUT) else {"keys": {}, "vectors": []}
old_sigs = {}
for v in old.get("vectors", []):
    old_sigs[("receipt", v["receipt"]["body"])] = v["receipt"]["sig"]
    old_sigs[("dsse", v["payload"])] = v["envelope"]["signatures"][0]["sig"]
tmp = tempfile.mkdtemp()

k = old["keys"].get("node")
if k is None:
    pem = run([OPENSSL, "ecparam", "-name", "prime256v1", "-genkey", "-noout"])
    pem = run([OPENSSL, "pkcs8", "-topk8", "-nocrypt"], pem).decode()
    k = {"alg": "p256", "private_key_pem": pem}
KEY_PATH = os.path.join(tmp, "node.pem")
open(KEY_PATH, "w").write(k["private_key_pem"])
der = run([OPENSSL, "ec", "-in", KEY_PATH, "-pubout", "-outform", "DER"])
k = {"alg": "p256", "private_key_pem": k["private_key_pem"], "public_key_pem": pem_of_spki(der),
     "spki_b64": base64.b64encode(der).decode(), "fp": sha(der)}
NODE_FP = k["fp"]


def p256(kind, msg_key, msg):
    s = old_sigs.get((kind, msg_key))
    if s is None:
        s = base64.b64encode(run([OPENSSL, "dgst", "-sha256", "-sign", KEY_PATH], msg)).decode()
    return s


ROSTER_FP = sha(b"provenance-v1 test roster key")
OWNER_SERVER_FP = sha(b"provenance-v1 test owner server")
CONTROLLER_FP = sha(b"provenance-v1 test controller")
STATEMENT_A = "sha256:" + sha(b"provenance-v1 node self-statement a")
STATEMENT_B = "sha256:" + sha(b"provenance-v1 node self-statement b")
COMMIT_A = sha(b"commit a")[:40]
TREE_A = sha(b"tree a")[:40]
COMMIT_B = sha(b"commit b")[:40]
TREE_B = sha(b"tree b")[:40]
COMMIT_LIB = sha(b"commit lib")[:40]
DIRTY_B = sha(b"dirty patch b")
LOG_A = sha(b"log a")
LOG_B = sha(b"log b")
BIN_A = sha(b"acme-linux-x86_64")
SUM_A = sha(b"acme-linux-x86_64.sha256")
XML_B = sha(b"report.xml")


def rfc3339(t):
    import datetime
    return datetime.datetime.fromtimestamp(t, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def argv_sha256(argv):
    return sha(canon(argv).encode())


vectors = []


def vector(name, plan, receipt, facts, statement):
    plan_digest = "sha256:" + sha(canon(plan).encode())
    assert receipt["plan_digest"] == plan_digest, name
    body = canon(receipt)
    rsig = p256("receipt", body, signed_bytes(RECEIPT_DOMAIN, body))
    payload = canon(statement)
    # The two facts the statement takes from the signed receipt's bytes.
    rc = [b for b in statement["predicate"]["runDetails"]["byproducts"] if b["name"] == "k2-receipt"][0]
    assert rc["digest"]["sha256"] == sha(body.encode()), name
    p = pae(PAYLOAD_TYPE, payload.encode())
    sig = p256("dsse", payload, p)
    envelope = {"payloadType": PAYLOAD_TYPE, "payload": base64.b64encode(payload.encode()).decode(),
                "signatures": [{"keyid": NODE_FP, "sig": sig}]}
    vectors.append({
        "name": name, "plan": plan, "plan_digest": plan_digest,
        "receipt": {"body": body, "sig": rsig}, "facts": facts,
        "statement": statement, "payload": payload, "pae_hex": p.hex(),
        "envelope": envelope, "line": json.dumps(envelope, separators=(",", ":")) + "\n",
    })


# ── A: a release-like build, receipt v1, remote with a password ──
ARGV_A = ["bash", "scripts/ci/build.sh", "x86_64"]
plan_a = {
    "job_id": "job-a", "node_id": "test-node-01", "workspace_id": "ws-acme", "workspace_label": "acme",
    "requested_by": "agent", "argv": ARGV_A, "env": {"BUILD_COMMIT": COMMIT_A}, "cwd": None,
    "limits": {"max_secs": 7200, "cpu_millis": 8000, "mem_bytes": 21474836480, "disk_bytes": 107374182400,
               "log_cap_bytes": 67108864},
    "src": {"project_key": "pk-acme", "remote_url": "https://ci-bot:token-123@git.example.test/acme/app.git",
            "commit": COMMIT_A, "dirty": None, "slots": 1},
    "exclusive": True, "created_at": 1800000700, "needs": ["cmake@3", "rust@1.99.1", "sys:pkg-config"],
    "artifacts": ["dist/*"], "secret_env": ["BUILD_API_KEY"], "tier": "ci", "provenance": 1,
}
receipt_a = {
    "v": 1, "job_id": "job-a", "attempt": 1, "generation": 1, "plan_digest": "sha256:" + sha(canon(plan_a).encode()),
    "node_fp": NODE_FP, "controller_fp": CONTROLLER_FP, "workspace_id": "ws-acme", "argv_sha256": argv_sha256(ARGV_A),
    "env_names": ["BUILD_COMMIT", "K2_JOB_ID"],
    "src": {"commit": COMMIT_A, "tree": TREE_A, "dirty_sha256": None, "slot": None, "warmth": "private"},
    "tools": {}, "boot_id": "boot-1", "started_at": 1800000731, "ended_at": 1800002260, "state": "done",
    "exit": {"code": 0, "signal": None}, "log": {"sha256": LOG_A, "bytes": 812345, "truncated": False},
    "artifacts": [{"name": "dist/acme-linux-x86_64", "sha256": BIN_A, "bytes": 31457280},
                  {"name": "dist/acme-linux-x86_64.sha256", "sha256": SUM_A, "bytes": 90}],
    "secret_env": ["BUILD_API_KEY"],
    "tools_used": {"cargo": "1.99.1", "rust-components": "cargo,clippy,rustc", "rustc": "1.99.1"},
}
facts_a = {"owner": {"roster_key": ROSTER_FP}, "network": "none", "isolation": "exclusive", "wall": "job_user",
           "tenant_slot": 0, "lane": "ci-standard", "k2node_version": "0.46.13", "k2node_commit": "6" * 40,
           "protocol": 10, "key_epoch": 3, "statement_digest": STATEMENT_A, "node_name": "test-node-01"}


def toolchain(tool, version, uri_version):
    return {"name": "toolchain:" + tool, "uri": "pkg:generic/%s@%s" % (tool, uri_version),
            "annotations": {"version": version, "observedBy": "node-probe"}}


statement_a = {
    "_type": "https://in-toto.io/Statement/v1",
    "subject": [{"name": "dist/acme-linux-x86_64", "digest": {"sha256": BIN_A}},
                {"name": "dist/acme-linux-x86_64.sha256", "digest": {"sha256": SUM_A}}],
    "predicateType": "https://slsa.dev/provenance/v1",
    "predicate": {
        "buildDefinition": {
            "buildType": "https://k2.dev/compute/buildtypes/job/v1",
            "externalParameters": {
                "source": {"uri": "git+https://git.example.test/acme/app.git", "digest": {"gitCommit": COMMIT_A}},
                "argv": ARGV_A, "env": {"BUILD_COMMIT": COMMIT_A}, "secretEnv": ["BUILD_API_KEY"],
                "needs": ["cmake@3", "rust@1.99.1", "sys:pkg-config"], "packages": [], "artifacts": ["dist/*"],
                "dirty": False,
            },
            "internalParameters": {
                "planDigest": receipt_a["plan_digest"], "jobId": "job-a", "attempt": 1, "generation": 1,
                "controllerFp": CONTROLLER_FP, "workspaceId": "ws-acme", "tier": "ci", "lane": "ci-standard",
                "network": "none",
                "limits": {"maxSecs": 7200, "cpuMillis": 8000, "memBytes": 21474836480, "diskBytes": 107374182400},
                "exclusive": True, "isolation": "exclusive", "wall": "job_user", "tenantSlot": 0, "warmth": "private",
            },
            "resolvedDependencies": [
                {"uri": "git+https://git.example.test/acme/app.git@" + COMMIT_A,
                 "digest": {"gitCommit": COMMIT_A, "gitTree": TREE_A}},
                toolchain("cargo", "1.99.1", "1.99.1"),
                toolchain("rust-components", "cargo,clippy,rustc", "cargo%2Cclippy%2Crustc"),
                toolchain("rustc", "1.99.1", "1.99.1"),
            ],
        },
        "runDetails": {
            "builder": {
                "id": "https://k2.dev/compute/builder/v1/owner/" + ROSTER_FP,
                "version": {"k2-node": "0.46.13", "k2-node-commit": "6" * 40, "protocol": "10"},
                "builderDependencies": [
                    {"name": "k2-node-statement", "digest": {"sha256": STATEMENT_A[len("sha256:"):]}},
                    {"name": "k2-node-key", "digest": {"sha256": NODE_FP},
                     "annotations": {"keyEpoch": 3, "node": "test-node-01"}},
                ],
            },
            "metadata": {"invocationId": "job-a/1/1", "startedOn": rfc3339(1800000731), "finishedOn": rfc3339(1800002260)},
            "byproducts": [
                {"name": "log", "digest": {"sha256": LOG_A}, "annotations": {"bytes": 812345, "truncated": False}},
                {"name": "k2-receipt", "digest": {"sha256": sha(canon(receipt_a).encode())}, "annotations": {"v": 1}},
            ],
        },
    },
}
vector("a_release_v1_receipt", plan_a, receipt_a, facts_a, statement_a)

# ── B: a bundle-only job with a dirty patch, a sibling repo and a job ──
# ── cache, receipt v2 (its run facts win), no roster yet            ──
ARGV_B = ["sh", "-c", "make report"]
plan_b = {
    "job_id": "job-b", "node_id": "test-node-02", "workspace_id": "ws-acme", "workspace_label": "acme",
    "requested_by": "agent", "argv": ARGV_B, "env": {}, "cwd": "app",
    "limits": {"max_secs": 600, "cpu_millis": None, "mem_bytes": None, "disk_bytes": 10737418240,
               "log_cap_bytes": 67108864},
    "src": {"project_key": "pk-bundle", "remote_url": None, "commit": COMMIT_B,
            "dirty": {"sha256": DIRTY_B, "bytes": 2048}, "slots": 1},
    "exclusive": False, "created_at": 1800003000, "artifacts": ["out/report.xml"], "cache": "deps",
    "src_path": "app",
    "with": [{"path": "lib", "project_key": "pk-lib", "remote_url": "git@git.example.test:acme/lib.git",
              "commit": COMMIT_LIB}],
    "packages": ["pkg-config"], "receipt_v": 2, "provenance": 1,
}
receipt_b = {
    "v": 2, "job_id": "job-b", "attempt": 2, "generation": 3, "plan_digest": "sha256:" + sha(canon(plan_b).encode()),
    "node_fp": NODE_FP, "controller_fp": CONTROLLER_FP, "workspace_id": "ws-acme", "argv_sha256": argv_sha256(ARGV_B),
    "env_names": ["K2_JOB_ID"],
    "src": {"commit": COMMIT_B, "tree": TREE_B, "dirty_sha256": DIRTY_B, "slot": None, "warmth": "cold"},
    "tools": {"git": "2.43.0"}, "boot_id": "boot-2", "started_at": 1800003010, "ended_at": 1800003070,
    "state": "failed", "exit": {"code": 2, "signal": None}, "log": {"sha256": LOG_B, "bytes": 1048576, "truncated": True},
    "artifacts": [{"name": "out/report.xml", "sha256": XML_B, "bytes": 4096}],
    "run": {"isolation": "job_uid", "uid_index": 2, "wall": "job_user", "tenant_slot": 1, "k2node_version": "0.46.13",
            "k2node_commit": None, "key_epoch": 0, "statement_digest": STATEMENT_B, "roster_seq_seen": None,
            "lane": None},
}
facts_b = {"owner": {"server": OWNER_SERVER_FP}, "network": "on", "isolation": "tenant_uid", "wall": "job_user",
           "tenant_slot": 1, "lane": "not-used", "k2node_version": "0.46.13", "k2node_commit": None,
           "protocol": 10, "key_epoch": 9, "statement_digest": "sha256:" + "0" * 64, "node_name": None}
statement_b = {
    "_type": "https://in-toto.io/Statement/v1",
    "subject": [{"name": "out/report.xml", "digest": {"sha256": XML_B}}],
    "predicateType": "https://slsa.dev/provenance/v1",
    "predicate": {
        "buildDefinition": {
            "buildType": "https://k2.dev/compute/buildtypes/job/v1",
            "externalParameters": {
                "source": {"uri": "k2-project:pk-bundle", "digest": {"gitCommit": COMMIT_B}},
                "argv": ARGV_B, "cwd": "app", "env": {}, "secretEnv": [], "needs": [], "packages": ["pkg-config"],
                "artifacts": ["out/report.xml"], "dirty": True,
            },
            "internalParameters": {
                "planDigest": receipt_b["plan_digest"], "jobId": "job-b", "attempt": 2, "generation": 3,
                "controllerFp": CONTROLLER_FP, "workspaceId": "ws-acme", "tier": "normal", "network": "on",
                "limits": {"maxSecs": 600, "diskBytes": 10737418240},
                "exclusive": False, "isolation": "job_uid", "wall": "job_user", "tenantSlot": 1, "warmth": "cold",
            },
            "resolvedDependencies": [
                {"uri": "k2-project:pk-bundle@" + COMMIT_B, "digest": {"gitCommit": COMMIT_B, "gitTree": TREE_B}},
                {"name": "with:lib", "uri": "git+ssh://git.example.test/acme/lib.git@" + COMMIT_LIB,
                 "digest": {"gitCommit": COMMIT_LIB}},
                {"name": "dirty-patch", "digest": {"sha256": DIRTY_B}},
                {"name": "job-cache:deps", "uri": "k2-job-cache:deps", "annotations": {"mutable": True}},
            ],
        },
        "runDetails": {
            "builder": {
                "id": "https://k2.dev/compute/builder/v1/owner-server/" + OWNER_SERVER_FP,
                "version": {"k2-node": "0.46.13", "protocol": "10"},
                "builderDependencies": [
                    {"name": "k2-node-statement", "digest": {"sha256": STATEMENT_B[len("sha256:"):]}},
                    {"name": "k2-node-key", "digest": {"sha256": NODE_FP}, "annotations": {"keyEpoch": 0}},
                ],
            },
            "metadata": {"invocationId": "job-b/2/3", "startedOn": rfc3339(1800003010), "finishedOn": rfc3339(1800003070)},
            "byproducts": [
                {"name": "log", "digest": {"sha256": LOG_B}, "annotations": {"bytes": 1048576, "truncated": True}},
                {"name": "k2-receipt", "digest": {"sha256": sha(canon(receipt_b).encode())}, "annotations": {"v": 2}},
            ],
        },
    },
}
vector("b_bundle_v2_receipt", plan_b, receipt_b, facts_b, statement_b)

doc = {
    "comment": "Golden vectors for docs/provenance-v1.md (prd-compute-build-attestations-v1 S2). TEST KEY ONLY. "
               "Each vector: a plan, its node-signed receipt (domain k2-compute-receipt-v1), the node's own facts, "
               "and the in-toto statement they give, written from the spec; payload = its canonical JSON; the "
               "envelope is DSSE, ECDSA P-256 SHA-256 DER over the PAE (randomized: verify it, don't compare it). "
               "Written by make-provenance-v1.py; checked by crates/k2-node-proto/tests/provenance_v1.rs.",
    "keys": {"node": k},
    "vectors": vectors,
}
with open(OUT, "w") as f:
    json.dump(doc, f, indent=2, sort_keys=True, ensure_ascii=False)
    f.write("\n")
print("wrote", OUT)
