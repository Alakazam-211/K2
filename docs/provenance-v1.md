# K2 Compute build statements, v1

Every K2 Compute job that keeps at least one artifact can come with a **build
statement**: an [in-toto Statement v1](https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md)
whose predicate is [SLSA Provenance v1](https://slsa.dev/spec/v1.0/provenance),
in a [DSSE envelope](https://github.com/secure-systems-lab/dsse/blob/master/envelope.md),
signed by the node that ran the job.

This page is the frozen format. Golden vectors:
`tests/release/fixtures/provenance-v1.json` (written by
`tests/release/fixtures/make-provenance-v1.py` from this page, checked by
`crates/k2-node-proto/tests/provenance_v1.rs`). The code is
`crates/k2-node-proto/src/provenance.rs`. A new field is a new `buildType`
(`…/job/v2`), never a silent change to v1.

## Who writes it

The node agent (`k2-node`, its own user), after the job's processes are gone
and its artifacts hashed, in the same step that signs the job's receipt. The
job never sees the node key or the statement. The statement is made from the
receipt's own signed body, so the two can't disagree: the same files with the
same digests, the same commit and tree, job, attempt, plan, times, secret
names, tools, log and node key.

It is written only when:

- the plan asks for it (`Plan.provenance = 1`), which a server sends only to a
  node offering the protocol-10 feature `provenance-v1`; and
- the job kept at least one artifact. **No artifact, no statement.** A CI check
  that only ran tests has a receipt, as before.

A node that can't make one (for example, a job adopted by a new k2-node
process after an update, whose start it didn't see) signs the receipt alone
and says so in the job's notes. It travels to the server as
`ReceiptFrame.provenance`, beside the receipt, and is kept in the node's job
ledger with it.

## The envelope

Exactly:

```json
{"payloadType":"application/vnd.in-toto+json","payload":"<base64>","signatures":[{"keyid":"<64 hex>","sig":"<base64>"}]}
```

- `payload`: the statement's canonical JSON (keys sorted by byte order, no
  whitespace, serde_json's escaping; no floats), standard base64 with padding.
  A reader refuses a payload that isn't its own canonical form, has unknown
  fields, holds a zero byte, or is over 384 KiB.
- `sig`: ECDSA P-256 with SHA-256, ASN.1 DER, standard base64 with padding,
  over the DSSE pre-authentication encoding:
  `"DSSEv1" SP len(payloadType) SP payloadType SP len(payload) SP payload`
  (lengths in ASCII decimal bytes).
- `keyid`: the node key's fingerprint, SHA-256 of its SubjectPublicKeyInfo
  DER, 64 lowercase hex (the same `node_fp` receipts carry).
- Exactly one signature.
- As a file: one envelope per line, `<label>.intoto.jsonl`.

The node key signs nothing else this way. Its other signatures are
`domain ‖ 0x00 ‖ length-prefixed parts`; no domain contains a space and a
statement payload holds no zero byte, so a PAE is never a K2 domain message
and no K2 domain message is a PAE.

## The statement

```json
{
  "_type": "https://in-toto.io/Statement/v1",
  "subject": [{"name": "dist/acme-linux-x86_64", "digest": {"sha256": "<64 hex>"}}],
  "predicateType": "https://slsa.dev/provenance/v1",
  "predicate": {
    "buildDefinition": {
      "buildType": "https://k2.dev/compute/buildtypes/job/v1",
      "externalParameters": {
        "source": {"uri": "git+https://git.example.test/acme/app.git", "digest": {"gitCommit": "<40 hex>"}},
        "argv": ["bash", "scripts/ci/build.sh", "x86_64"],
        "cwd": "app",
        "env": {"BUILD_COMMIT": "<40 hex>"},
        "secretEnv": ["BUILD_API_KEY"],
        "needs": ["rust@1.99.1"],
        "packages": [],
        "artifacts": ["dist/*"],
        "dirty": false
      },
      "internalParameters": {
        "planDigest": "sha256:<64 hex>", "jobId": "job-a", "attempt": 1, "generation": 1,
        "controllerFp": "<64 hex>", "workspaceId": "ws-acme",
        "tier": "ci", "lane": "ci-standard", "network": "none",
        "limits": {"maxSecs": 7200, "cpuMillis": 8000, "memBytes": 21474836480, "diskBytes": 107374182400},
        "exclusive": true, "isolation": "exclusive", "wall": "job_user", "tenantSlot": 0,
        "warmth": "private"
      },
      "resolvedDependencies": [
        {"uri": "git+https://git.example.test/acme/app.git@<40 hex>", "digest": {"gitCommit": "<40 hex>", "gitTree": "<40 hex>"}},
        {"name": "toolchain:rustc", "uri": "pkg:generic/rustc@1.99.1", "annotations": {"version": "1.99.1", "observedBy": "node-probe"}}
      ]
    },
    "runDetails": {
      "builder": {
        "id": "https://k2.dev/compute/builder/v1/owner/<roster key fp>",
        "version": {"k2-node": "0.46.13", "k2-node-commit": "<40 hex>", "protocol": "10"},
        "builderDependencies": [
          {"name": "k2-node-statement", "digest": {"sha256": "<self-statement digest>"}},
          {"name": "k2-node-key", "digest": {"sha256": "<node fp>"}, "annotations": {"keyEpoch": 3, "node": "test-node-01"}}
        ]
      },
      "metadata": {"invocationId": "job-a/1/1", "startedOn": "2027-01-15T08:12:11Z", "finishedOn": "2027-01-15T08:37:40Z"},
      "byproducts": [
        {"name": "log", "digest": {"sha256": "<64 hex>"}, "annotations": {"bytes": 812345, "truncated": false}},
        {"name": "k2-receipt", "digest": {"sha256": "<sha256 of the receipt body>"}, "annotations": {"v": 1}}
      ]
    }
  }
}
```

Lists are always present (`[]` when empty). Optional values are absent when
unset: `source` (a job with no code), `cwd`, `lane`, `warmth`, `cpuMillis`,
`memBytes`, `k2-node-commit`, `k2-node-statement`, `node`, `log`.

### Where each field comes from

| Field | From | Trust |
|---|---|---|
| `subject` | The receipt's `artifacts`, in its order: every kept file, hashed by the node after the job exited. The collector never follows or keeps a symlink | Node-computed over job-written bytes |
| `source.digest.gitCommit`, the first `resolvedDependencies` entry (`gitCommit`, `gitTree`) | The commit the node checked out and its tree (receipt `src`) | Node-checked |
| `source.uri` | The plan's remote: `git+<remote>` with any user name or password removed; `user@host:path` becomes `git+ssh://host/path`; a bare path `git+file://<path>`; no remote (bundles only) `k2-project:<project key>` | Server-named |
| `argv`, `cwd`, `env`, `needs`, `packages`, `artifacts` | The plan, which the node enforces (its digest is the receipt's `plan_digest`, its argv hashes to `argv_sha256`, and every `env` name is in the receipt's `env_names`) | Node-enforced |
| `secretEnv` | The receipt's names. Values never appear anywhere | Node-enforced |
| `dirty`, `dirty-patch` | The receipt's `src.dirty_sha256` | Node-computed |
| `with:<path>` entries | Sibling repos (`--with`), each at its commit | Node-checked commit, server-named uri |
| `internalParameters` ids, `controllerFp`, `workspaceId`, `warmth` | The receipt | Node |
| `tier`, `limits`, `exclusive` | The plan | Node-enforced |
| `network` | The network rule the job started under | Node-enforced |
| `isolation`, `wall`, `tenantSlot`, `lane` | Receipt v2's `run` facts; for a v1 receipt, the node's own wall and the isolation it started the job under | Node self-report |
| `toolchain:<tool>` | The receipt's `tools_used`: what the job's own PATH resolved to, probed by the node before the job started. Version text, no digest in v1, so each carries a `pkg:generic/<tool>@<version>` uri (percent-encoded). Absent when the node didn't probe | Node-observed |
| `cache:<lineage>` | A receipt v3's cache generation (`gen_digest`, `seq`, producer receipt), when the job forked one | Node-managed lineage |
| `job-cache:<name>` | The plan's `--cache` folder: mutable, so a uri (`k2-job-cache:<name>`) and `mutable: true`, no digest | Node-enforced |
| `builder.id` | The owner's fleet: `…/owner/<roster key fp>` when the node holds its owner's roster, else `…/owner-server/<owner server fp>`. One id per owner, so a SLSA verifier compares one string. The owner in the id is the node's claim; the roster proves it | Node claim |
| `builder.version`, `k2-node-statement`, `k2-node-key` | Receipt v2's `run` (version, commit, epoch, self-statement digest) or the node's own; the key digest is the node fingerprint | Node self-report |
| `metadata` | The receipt's `started_at` / `ended_at`, RFC 3339 UTC seconds, node clock | Node clock |
| `byproducts` | The receipt's log digest and size; the SHA-256 of the receipt's signed body and its `v` | Node-computed |

Anything a job says about itself (a `build-info.json`, a statement file it
wrote, test results) stays inside the subjects. A verifier that needs it reads
the file, matches its digest to a subject and treats its contents as the job's
word.

`env` holds **values**. Nothing secret may travel there; secrets come through
`secretEnv` (names only).

## Shape rules (every reader)

- `_type`, `predicateType`, `buildType` exactly as above.
- At least one subject, at most 1000; each a unique non-empty `name` and
  exactly one `sha256` digest, nothing else.
- Every ResourceDescriptor has a `uri` or a `digest`. Digest algorithms:
  `sha256` (64 lowercase hex), `gitCommit` and `gitTree` (40 lowercase hex).
- No floats anywhere.
- `builder.id` is one of the two owner forms with a 64-hex fingerprint.
- `k2-node-key` appears once in `builderDependencies`, `k2-receipt` once in
  `byproducts`.
- Times are `YYYY-MM-DDTHH:MM:SSZ`.

## Checking one

1. Verify the DSSE signature with the node's public key (`keyid` names it).
   Stock DSSE tools can do this step alone with the node's PEM.
2. Parse the payload with unknown fields refused; it must be its own canonical
   form and pass the shape rules.
3. With the receipt: the receipt verifies with the same key, and every field
   above agrees with it (`k2_node_proto::provenance::check_against_receipt`).
4. Which node keys to trust is the owner's roster, not this file: see
   `docs/ci-formats-v1.md` and prd-compute-build-attestations-v1 §3 and §7.
   `builder.id` names the owner whose roster to check. A verifier that pins
   an owner's roster key accepts only `…/owner/<that key's fp>`; the
   `…/owner-server/<fp>` form (a node that held no roster when it signed)
   names no roster key and is refused
   (`Statement::check_builder_owner`).

## On the server

A K2 server asks for a statement (`Plan.provenance = 1`) only of a node
offering `provenance-v1`, and only with its CI preview on; any other node's
plan has no `provenance` key, so its bytes and digest are as before. When
the receipt arrives the server checks the statement beside it: the plan
asked, it verifies with the node key the server pinned at enrolment, and it
agrees with the receipt (`Dsse::open_for_receipt`). It also refuses one
whose plain `env` names a secret in its vault, because plain env values are
written into the statement. The job's stored receipt keeps the envelope
under `provenance`, or the reason under `provenance_refused` ("no
statement"); the receipt itself stands either way.

Readers:

- `k2 compute provenance <job> [--out FILE]`: the `.intoto.jsonl` line.
- `k2 compute verify <job | FILE.intoto.jsonl> [--file PATH] [--owner-key FP]`:
  checks a statement against the server's own records (the node key it
  pinned, the job's receipt, the vault, the file's digest, the owner's
  roster it holds) and prints each check. A convenience for agents: a
  release signer checks again with its own pins.
- `k2 ci provenance <run> --out <dir>` and `k2 ci evidence <run> --out
  <dir>` (bundle v3, `docs/ci-formats-v1.md` §8).

## Files a client gets

`k2 ci provenance` and `k2 ci evidence` write:

| Path | What |
|---|---|
| `provenance/<label>.intoto.jsonl` | One statement per job, by its label (`L1`, `V1`) or check name |
| `trust/nodes/<fp>.pem` | Each node's public key (PEM); `<fp>` is the envelope's `keyid`. A stock DSSE tool checks the signature with it |
| `trust/statements/<sha256>.json` | The node self-statements the receipts name (`{body, sig}`; `sha256` of `body`) |
| `trust/owners.json` | `[{id, rosterKeyFp, rosterSeq}]`: each owner a statement names whose roster the server holds |
| `trust/rosters/<rosterKeyFp>.json` | That owner's newest roster held by the server (`{body, sig}`), as of the build |

A roster in `trust/` is the one the server held when the files were
written: a key revoked later is still listed there. A verifier that cares
fetches the owner's current roster.
