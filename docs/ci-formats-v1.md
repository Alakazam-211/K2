# CI on compute: the data formats (v1)

These are the frozen formats for CI and signed release builds on shared
K2 Compute nodes: what nodes, node owners, client servers and the signer
exchange. Node, signer, `k2 ci` and server work all build against them.

- **Rust types:** `crates/k2-node-proto` (`ci`, `statement`, `node_roster`,
  `evidence`, `signer_policy`, `ci_config`, plus `frames::Plan.receipt_v`
  and `frames::Receipt.run`).
- **Golden vectors:** `tests/release/fixtures/ci-formats-v1.json`, with
  `ci-formats-v1-signer-policy.toml` and `ci-formats-v1-ci.toml` (§10).
- **Checks:** `crates/k2-node-proto/tests/ci_formats_v1.rs` (Rust) and
  `tests/release/k2sign-ci-formats.sh` (Python and OpenSSL).

## 1. Rules for every format

### Signing

A signed document travels as `{"body": "<text>", "sig": "<base64>"}`. The
body is the document's **canonical JSON**, and the signature covers:

```
domain || 0x00 || u32_be(len(body)) || body
```

That is `crypto::signed_bytes(domain, [body])`, the same layout as receipts
and node admin.

- Canonical JSON (`canonical.rs`, and `canonical_json` in `k2sign.py`):
  object keys sorted by byte order, no whitespace, and serde_json's string
  escaping (non-ASCII written as UTF-8, not `\u` escapes). There are no
  floats anywhere. Times are unix seconds (`i64`).
- **Node documents** are signed with the node key: ECDSA P-256, SHA-256,
  DER signature, base64. This signature is randomized.
- **Owner documents** are signed with the owner's **node-roster key**:
  Ed25519, raw 64-byte signature, base64. This signature is deterministic.
  The key is the vault engine key `compute/node-roster`. It sits beside the
  node-admin approval key, and is never the tunnel key or a passport.
- A verifier works in this order:
  1. Check the signature over the received text.
  2. Parse the body. Unknown keys are refused.
  3. Refuse a body that isn't its own canonical form. Whitespace,
     reordering and duplicate keys all fail here.
- Documents name each other by **digest**: `sha256:<hex>` of the body
  bytes (`ci::digest`).
- A key **fingerprint** is 64 lowercase hex characters:
  - P-256: SHA-256 of the SPKI DER.
  - Ed25519: SHA-256 of the raw 32-byte key (`ApprovalKey::fingerprint`).

### Domains

| Domain | Document | Signed by |
|---|---|---|
| `k2-node-statement-v1` | node self-statement (§2) | node key (P-256) |
| `k2-node-rotation-v1` | key rotation (§3) | the **old** node key (P-256) |
| `k2-node-roster-v1` | node roster (§4) | owner node-roster key (Ed25519) |
| `k2-node-seat-v1` | seat statement (§5) | owner node-roster key (Ed25519) |
| `k2-node-attest-v1` | owner attestation (§6) | owner node-roster key (Ed25519) |
| `k2-compute-receipt-v1` | receipt v1 **and** v2 (§7) | node key (P-256) |
| `k2-signer-sas-v1` | signer ↔ server pairing SAS | (the `pairing.rs` SAS scheme) |

Receipt v2 keeps the v1 domain. The domain names the document (a receipt),
and the signed `v` names its schema. "Node roster" is never shortened to
"roster", because federation already uses that word.

### Versioning and compatibility

- **New documents (§2–§6, §8 evidence):** every key is always present. An
  unset optional is `null` and an empty list is `[]`. Each document carries
  a `v`. Adding, removing or changing a field is a **new `v`**, and
  verifiers refuse a `v` they don't know. Optional fields 0.46.11 needs
  (`pod`, `point`, `line`) exist already as `null`, so pods need no new
  `v`.
- **Grown structs (`Plan`, `Receipt`):** they keep their old rule. A new
  field is **absent** when unset, so older plans, digests and receipts are
  byte-identical (§7).
- **TOML files (§8 policy, §9 ci.toml):** they carry `version = 1`. Unknown
  keys are refused, and enum values are `snake_case` like the wire.
- **Protocol 9** is `frames::PROTOCOL_CI`. `frames::PROTOCOL` moved to 9
  with slice C0, the first node part that offers a protocol-9 feature
  (one bump for all node parts). How the node's documents travel:
  - the self-statement (§2): the `node_statement` frame, once per session
    and again whenever it changes;
  - receipt v2 (§7): the `receipt` frame, with the statement its
    `run.statement_digest` names beside it (`ReceiptFrame.statement`).
    The server keeps that statement with the receipt, and
    `/cli/compute/jobs/receipt` returns it (`statement`) next to `body`,
    `signature`, `node_public_key_pem` and `plan`;
  - a submit asks for v2 with `receiptV: 2` (`k2 compute run
    --receipt-v2`).
- The k2-node build commit (§2 `k2node_commit`, V8) is its own marker,
  `K2_NODE_COMMIT c=<40 hex>;`, beside `K2_NODE_BUILD …;`. It is not a
  field of the build marker, because older nodes and `sign-k2-node.sh`
  parse that marker strictly.
- The rule is additive, as in every protocol before it:
  - A node offers a protocol-9 feature only to a server that said protocol
    ≥ 9.
  - A server sends a protocol-9 field or frame only to a node offering
    that feature.

| Feature | Meaning |
|---|---|
| `node-statement` | the node signs and sends its self-statement |
| `receipt-v2` | the node emits receipt v2 when the plan has `receipt_v: 2` |
| `node-roster` | the node holds its owner's pinned roster key and relays the roster |
| `node-rekey` | node and server understand key rotation; a node re-keys only when every active server offers it |
| `lanes` | `/etc/k2-node-lanes.toml` and its verb |
| `job-uid` | secret jobs may run as a per-job uid from the slot's pool |
| `lane-secrets` | `trust team` tenants may get secret env under lane rules |
| `artifact-delete` | the controller may delete a secret job's artifacts after fetching them |

Refusal codes are frozen in `ci::codes`:

- `no_independent_nodes`, `seat_degraded`, `seat_moving`, `no_free_seat`,
  `node_client_cap`;
- `node_key_tainted`, `rekey_blocked_old_server`,
  `secret_owner_not_trusted`, `mac_seat_not_offered`;
- `node_roster_stale`, `node_roster_replayed`.

### Shared enums (wire spelling)

| Enum | Values |
|---|---|
| `wall` | `job_user`, `seatbelt`, `none` |
| `isolation` | `job_uid` (the job's own uid from the pool), `exclusive` (nothing else on the node), `tenant_uid` (the tenant's `k2job-<slot>`), `shared_uid` (one uid for every tenant) |
| `taint` | `clean`, `tainted`, `unknown` (the key predates the flag) |
| `status` | `active`, `retired`, `revoked` |
| `revoked_reason` | `compromised`, `key_lost`, `other` |
| `taint_basis` | `self_clean`, `rekeyed`, `owner_attested` |
| rotation `reason` | `scheduled`, `owner`, `wall`, `compromise` |
| seat `state` | `offered`, `active`, `suspended`, `ended` |
| `independent` | `node`, `host`, `site`, `provider`, `admin_reach`, `owner` |

Names (`[a-z0-9._-]`, 1–63 bytes, starting with a letter or digit) are
used for nodes, lanes, builds, projects, seats, owners and run ids.
Receipt labels match `^[A-Z][A-Z0-9]{0,7}$`: `L1`, `V1`, `L3`.

## 2. Node self-statement (`statement::NodeStatement`, v1)

The node signs this statement with its own key. It names that key, so it
is self-certifying: `node_fp` = SHA-256 of `node_spki`. The node signs a
new statement whenever a fact changes, and the newest one counts.

| Field | Type | Meaning |
|---|---|---|
| `v` | 1 | |
| `node_fp` | fp | the signing key |
| `node_spki` | base64 | P-256 SPKI DER |
| `key_epoch` | u32 | 0 = the enrolment key, +1 per rotation |
| `key_born_at` | i64 | |
| `prev_fp` | fp \| null | the replaced key; set exactly when `key_epoch > 0` |
| `issued_at` | i64 | not before `key_born_at` |
| `wall` | enum | `seatbelt` only on macOS, `job_user` only on Linux |
| `dev` | bool | a `--dev` node |
| `taint` | enum | V10: any job start under wall `none` with this key |
| `key_exposure` | object | `store` (`file`\|`keychain`); `owner` (OS user); `mode` (file mode in decimal, `null` for the keychain); `job_readable` (jobs run as the key's uid) |
| `k2node_version` | string | |
| `k2node_commit` | 40 hex \| null | V8: from the build marker (a candidate build reports the old version) |
| `protocol` | u32 | |
| `os`, `arch` | name | |
| `distro`, `distro_version`, `kernel` | string \| null | |
| `ptrace_scope` | 0–3 \| null | V9: Yama; `null` on macOS |
| `host_id` | fp \| null | SHA-256 of `"k2-node-host-v1" 0x00 <machine id>` (salted, not the raw id); self-reported |

The node's word is a self-report. The owner's roster `host` (§4) is what
`independent = ["host"]` uses.

## 3. Key rotation (`statement::RotationStatement`, v1)

The **old** key signs the rotation. Then the new key signs a §2 statement
with `prev_fp` = the old key and `key_epoch` = `new_epoch`. Verifiers open
the rotation with the old key they already trust, then check
`continued_by(new statement)`.

| Field | Type |
|---|---|
| `v` | 1 |
| `old_fp`, `new_fp` | fp (different) |
| `new_spki` | base64 P-256 SPKI of `new_fp` |
| `new_epoch` | u32 ≥ 1 |
| `at` | i64 |
| `reason` | enum |

## 4. Node roster (`node_roster::NodeRoster`, v1)

The owner keeps **one** node roster, signed with its node-roster key.

| Field | Type | Meaning |
|---|---|---|
| `v` | 1 | |
| `owner_id` | name | the owner server's host |
| `owner_server_fp` | fp | its tunnel key (what its nodes pin) |
| `roster_key` | `ed25519:<b64>` | must equal the pinned key |
| `seq` | u64 ≥ 1 | strictly rising per owner |
| `issued_at` | i64 | |
| `prev_digest` | digest \| null | the previous roster; set exactly when `seq > 1` |
| `entries` | list | one per **key**, sorted by `node_fp` |

**Freshness (V13).** The signer keeps the highest `seq` it has seen per
owner and refuses a lower one (`node_roster_replayed`). It also refuses a
roster issued longer ago than the policy's `roster_max_age_hours`
(`node_roster_stale`).

An entry has these fields:

| Field | Type | Meaning |
|---|---|---|
| `node_fp`, `node_spki` | fp, base64 | the key |
| `name` | name | the node's stable name, the same across re-keys; at most one `active` entry per name |
| `key_epoch` | u32 | |
| `statement_digest` | digest | the §2 statement the owner approved |
| `status` | enum | |
| `revoked_reason` | enum \| null | set exactly when `revoked` |
| `valid_from` | i64 | |
| `valid_to` | i64 \| null | required when `retired`, `null` while `active`, never before `valid_from` |
| `taint_basis` | enum \| null | `null` means the owner vouches for nothing, and the key never counts for a CI lane |
| `attestation_digest` | digest \| null | set exactly when `taint_basis` is `owner_attested` |
| `host` | name | the owner's word for the physical machine |
| `provider`, `site` | name \| null | |
| `admin_reach` | names, sorted | who holds root (for example a fleet SSH key) |
| `pod`, `point` | name/u32 \| null | 0.46.11 |

A rotated node keeps its old key's entry (`retired`, `valid_to` = the
rotation) so its old receipts stay checkable.

What the entry's status means for receipts:

- `revoked` with `compromised`: every receipt from that key is void, past
  and future.
- `retired`: receipts that started after `valid_to` are void.

## 5. Seat statement (`node_roster::SeatStatement`, v1)

There is one seat statement per client, so no client sees another client's
identity. It is signed with the same key as the roster.

| Field | Type | Meaning |
|---|---|---|
| `v` | 1 | |
| `owner_id`, `roster_key` | | as in §4 |
| `seat_id` | name | |
| `client_fp` | fp | the client server's tunnel key (the controller) |
| `client_label` | text | |
| `lane` | name | |
| `nodes` | names, sorted, 2–16 | roster entry **names**, not keys, so a re-key needs a new roster but no new seat |
| `roster_seq` | u64 | every name above has an `active` entry in that roster |
| `state` | enum | |
| `issued_at` | i64 | |
| `pod`, `line` | name \| null | 0.46.11 |

## 6. Owner attestation (`node_roster::OwnerAttestation`, v1)

This is V12: the owner vouches for an existing key instead of re-keying it.
The key must have been installed with the wall and must never have started
a job under wall `none`. A person approves it on a card. The roster entry
then carries `taint_basis: owner_attested` and this document's digest.
Each signer policy decides whether it accepts `owner_attested` (§8).

| Field | Type | Meaning |
|---|---|---|
| `v` | 1 | |
| `owner_id`, `roster_key` | | as in §4 |
| `node_fp`, `key_epoch` | | the key vouched for |
| `statement_digest` | digest | its statement (which may say `taint: unknown`) |
| `installed_with_wall` | bool | must be `true` |
| `unwalled_jobs_seen` | bool | must be `false`; otherwise it is refused with `node_key_tainted`, and the node must re-key |
| `checked` | text ≤ 500, non-empty | what the owner checked |
| `approved_by` | text, non-empty | the person |
| `at` | i64 | |

## 7. Receipt v2 (`frames::Receipt`, `v: 2`)

- **Sent only when the plan asks.** `Plan.receipt_v: 2` is absent unless
  set, and is sent only to a node offering `receipt-v2`. A plan without it
  gets today's receipt v1, **byte-identical** (V4/V7). The S3b path
  (`k2sign`, `K2_CI_PATH=scripts`) therefore never sees v2.
- `k2sign.py`'s `PLAN_KEYS` lists `receipt_v`, so its plan-field drift test
  keeps passing. Its `check_plan` refuses a plan that sets it ("this
  signer checks v1 receipts only"). k2sign already refuses a receipt
  `v != 1` and a bundle `v != 1`.
- **A v2 job builds private** (0.46.11): no warm slot, its own
  `CARGO_HOME`, sccache dir and server, with or without secrets, so its
  receipt says `warmth: private` (the signer requires it). The plan field
  is the same `receipt_v`; nothing else changes on the wire.
- **v2 = v1 + `run`.** `Receipt::check_version` enforces both directions:
  v1 has no `run`, and v2 has one. `run` (`ci::RunFacts`) has every key
  present:

| Field | Type | Meaning |
|---|---|---|
| `isolation` | enum | |
| `uid_index` | u32 \| null | V15 pool index; set exactly for `job_uid` |
| `wall` | enum | `job_uid` needs `job_user` |
| `tenant_slot` | u32 | 0 = the owner server |
| `k2node_version`, `k2node_commit` | | as in §2 |
| `key_epoch` | u32 | |
| `statement_digest` | digest | the §2 statement in force when the job started |
| `roster_seq_seen` | u64 \| null | the highest owner roster `seq` the node holds |
| `lane` | name \| null | |

## 8. Evidence bundle v2 and v3 (`evidence::EvidenceBundle`) and signer policy

### Evidence bundle

`k2 ci evidence` writes this file, and `k2 signer sign --evidence` reads
it. It is not signed itself: each part carries its own signature, and the
signer trusts only its own pins. It is closed JSON, at most 8 MiB.

| Field | Type | Meaning |
|---|---|---|
| `v`, `kind` | 2 or 3, `"k2-ci-evidence"` | `k2 ci evidence` writes 3; readers take 2 and 3; a v1 bundle is refused by name |
| `project` | name | the signer project |
| `version` | string \| null | `null` for a CI run that signs nothing |
| `run_id` | name | |
| `controller_fp` | fp | |
| `created_at` | i64 | |
| `source` | object | `commit` and `tree` (40 hex), plus `commit_object` (base64 `git cat-file commit`, or `null`); when present it must hash to `commit` |
| `receipts` | label → receipt | 1–32 entries |
| `statements`, `rotations`, `attestations`, `rosters`, `seats` | lists of `{body, sig}` | |
| `pairing_pins` | fp → base64 SPKI | what the client server pinned at pairing: the PRD §4.1.6 cross-check |
| `provenance` | v3 only, always there: label → DSSE envelope | each job's build statement (`docs/provenance-v1.md`); `{}` when no node signed one. Every label is a `receipts` label; each envelope has one signature and a 64-hex `keyid`. Nothing in it is trusted for being here: the signer checks each against its receipt (prd-compute-build-attestations-v1 S4) |
| `roster_approvals` | v3 only, optional: a list of objects | reserved for the node owner's signed approvals of the rosters (prd §18, slice A2); absent until then. A reader that doesn't know them carries them unread |

A v2 bundle has neither `provenance` nor `roster_approvals`; a v3 bundle
without `provenance` is refused.

Each receipt entry has these fields:

- `build`: the policy's build name.
- `role`: `build` (whose files are signed) or `verify` (the dual build).
  Each build has exactly one `build` receipt.
- `body`: the receipt text exactly as signed.
- `sig`: the node's signature.
- `plan`: the job plan as JSON. The signer hashes its canonical form to
  `plan_digest`.

The v1 bundle (`compute_release.py evidence-bundle`, `{"v":1,…}`) is
unchanged and stays k2sign's.

### Signer policy (`signer_policy::SignerPolicy`, TOML `version = 1`)

There is one root-owned file per project. The signer never reads it from
the repo at sign time.

**Top level:**

| Key | Rule |
|---|---|
| `project` | a name |
| `controllers` | fps, sorted, at least one |
| `owners` | node-roster key fps, sorted, at least one |
| `workspaces` | at least one (V30) |
| `receipt_max_hours` | 1–168 |
| `roster_max_age_hours` | 1–720 (V13; 168 recommended) |
| `roster_changes`, `seat_moves` | `ask` or `auto` (S1: `ask` for 0.46.10) |
| `taint_basis` | the accepted bases, at least one, no duplicates (V12) |

**`[source]`:**

- `kind = "github_tag"` needs `repo = "owner/repo"`; `tag_prefix` defaults
  to `v`.
- `kind = "approval_commit"` has no repo.

**`[build.<name>]`:**

| Key | Rule |
|---|---|
| `argv`, `needs`, `artifacts` | required; `{rust_pin}` in `needs` is replaced by the commit's pinned channel |
| `env`, `secrets`, `secrets_optional` | job env names; `K2_` and reserved names are refused |
| `isolation` | required |
| `walls` | default `["job_user"]` |
| `dual` | artifact names the verify build must reproduce |
| `independent` | required exactly when `dual` is set |
| `secret_sha256` | S9: SHA-256 of values baked into the binary, keyed by secret name |

Rules for builds:

- A secret build accepts only `job_uid` or `exclusive`, behind `job_user`.
- `none` is never accepted.
- A dual `seatbelt` build needs `independent` to include `owner` (S6).

**`[assets.<file>]`:**

- `key` names a `[keys.<slot>]` entry.
- `build` names a `[build.<name>]` entry.
- `artifact` is the file in the build receipt.
- `check = "artifact"`, or `"inner_binary"` with a relative `inner` path
  (Q11), which needs a dual build.

**`[keys.<slot>]`:** `kind = "minisign"` and `public` (the `RW…` line).

## 9. `.k2/ci.toml` (`ci_config::CiConfig`, `version = 1`)

`k2 ci init` writes `.k2/ci.toml`, one file (V32). The controller reads
it. The signer never trusts it: a release build here that differs from
the policy's build fails at signing.

- **`[watch]`:** `remote`, `branches` (names or `prefix/*`), `poll_secs`
  (30–3600, default 60).
- **`[statuses]`:**
  - `token_secret`: the vault name, for example `K2_CI_STATUSES_TOKEN`.
  - `env_name`: default `CI_STATUSES_TOKEN`. It must not start with `K2_`.
  - `context`: default `k2-ci`.
- **`[checks.<name>]`:** `argv`, plus optional `needs`, `os` (`linux` or
  `macos`), `artifacts`, `timeout_secs`, `cpus` and `mem_gb`.
- **`[release]`:** `project`, `owner` (the node owner), `lane`.
- **`[release.builds.<policy build name>]`:**
  - `argv`, `needs`, `artifacts`;
  - `env` and `secrets` (lane grants);
  - `label` (`L1`), and `verify_label` (`V1`) for a dual build;
  - `independent` (0.46.11, a dual build only): the roster dimensions the
    verify node must differ in, default `["host"]` (`node` is implied).
    `k2 ci release` places by it; the signer checks its own policy's list;
  - `timeout_secs`, `cpus`, `mem_gb` (0.46.11): the jobs' limits, as for
    a check. The signer doesn't read them.

  Labels are unique across builds. Both additions are optional and absent
  when unset, so a file without them parses as before.

## 10. Golden vectors

`tests/release/fixtures/ci-formats-v1.json` holds the vectors. **Its keys
are test keys only.**

- **`keys`:**
  - P-256 keys `node_a_epoch0`, `node_a_epoch1`, `node_b` and `controller`
    (PKCS#8 PEM, SPKI, fp);
  - the Ed25519 `roster` key (`seed_hex` = SHA-256 of
    `"k2-ci-formats-v1 test node-roster key"`, plus `public` and `fp`).
- **`vectors`:** each vector has `name`, `kind`, `domain`, `alg`,
  `signer`, `body`, `signed_bytes_hex` (the exact signed bytes), `digest`
  and `sig`:
  - `node_statement_a_epoch0`: `taint: unknown`;
  - `rotation_a`: A's old key → new key, reason `wall`;
  - `node_statement_a_epoch1`;
  - `node_statement_b`: `unknown`;
  - `owner_attestation_b`: its `checked` text includes non-ASCII;
  - `node_roster`: `seq` 2; A's old key retired with `taint_basis: null`,
    A's new key `rekeyed`, B `owner_attested`;
  - `seat_statement`;
  - `plan_receipt_v2`: unsigned;
  - `receipt_v2_l1`: on A, `job_uid`;
  - `receipt_v2_v1`: on B, `exclusive`.
- **`evidence_bundle`:** a complete v2 bundle built from the vectors above.
  Its commit object hashes to the receipts' commit, its plans hash to their
  `plan_digest`, and its receipts verify with its `pairing_pins`.
- **`ci-formats-v1-signer-policy.toml` and `ci-formats-v1-ci.toml`:** a
  policy and a ci.toml that match that bundle.

How to use them:

- **P-256 signatures are randomized:** verify them, don't compare them.
- **Ed25519 signatures are deterministic:** re-sign with the seed and
  compare the bytes.
- Check every body for: canonical JSON of itself, `signed_bytes_hex`,
  `digest`, and the typed shape rules.

`tests/release/fixtures/make-ci-formats-v1.py` writes the files:

- canonical JSON from `k2sign.py`, P-256 from `/usr/bin/openssl`, Ed25519
  from the RFC 8032 reference (`ed25519_ref.py`; OpenSSL 3 gives the same
  bytes).
- Re-running it keeps the keys, and keeps the P-256 signature of any body
  that didn't change.
- Change a format on purpose (a new `v`, this doc, the generator), never
  the vector file alone.

The receipt v1 vector (`receipt-vector.json`) is unchanged and still
passes in Rust and in k2sign.

## 11. Cache generations, boxes, test history (protocol 10 contracts)

prd-compute-blacksmith-class-v1 §6, slice B0. These formats exist in
`k2-node-proto` (`gens`, `boxes`, `ci::CacheFacts`, `frames::Receipt`
v3, `ci_config` version 2, `lanes::LaneTemplate.release_cache`,
`signer_policy::BuildRule.warm`). `frames::PROTOCOL` is 10 since slice R2:
k2-node offers `cpu-burst` (while its owner's `cpu_burst` policy is on) and
`job-stats` to a server that said protocol ≥ 10; the other features follow
in the same k2-node release as their slices land (one bump for every node
part, like protocol 9). A protocol-9 peer sees exactly 0.46.12's frames.

R2 (`cpu-burst`): the node's unit gets `CPUQuota` at `cpus_burst` and a
`CPUWeight` of 100 per guaranteed core, never the tier, so every job
always gets its `--cpus` on a full node and spare cores are shared by
guarantee (root's executor says `cpu_weight` in `Ready`, else no unit
bursts); the job's env gets `K2_JOB_CPUS_MAX` (the burst, or
`K2_JOB_CPUS` without one). O2 (`job-stats`): `Outputs.stats` is kept by
the controller in `compute_job_stats` (migration 0296), not in the job's
summary.

### Plan and result fields: features, not a protocol bump (BK-19)

Each is absent when unset (old plans, digests and results are
byte-identical) and is sent only to a node offering its feature
(`gens::plan_features`, `gens::missing_feature`):

| Field | Feature | Meaning |
|---|---|---|
| `Plan.cache_commit` | `cache-gens` | `trusted`, `scratch`, `release` or `none`: what the node does with the job's fork |
| `Plan.cache_ref` | `cache-gens` | the scratch ref (`main`, `wt:<worktree>`) or the release build name; none for `trusted`/`none` |
| `Plan.setup[]` | `setup-snapshots` | `{name, key, argv, paths}` per `[setup.*]` step |
| `Plan.cpus_burst` | `cpu-burst` | the CPU ceiling in thousandths of a core, above `limits.cpu_millis` |
| `Plan.receipt_v: 3` | `receipt-v3` | ask for receipt v3 |
| `Outputs.stats` | `job-stats` | the 5 s series (≤ 720 points, ≤ 64 KB), throttle and OOM counters |
| `Outputs.cache` | `cache-gens` | the forked lineage/seq/commit, `fork_ms`, `committed`, `commit_lineage`, `why` |
| `Outputs.setup[]` | `setup-snapshots` | `{name, key, hit, ms}` |
| `HostFacts.cache_fs` | `cache-gens` | `{fs, clone: subvolume\|reflink\|clonefile\|copy, volume}` |

The sixth feature, `box`, gates the box frames below. A node offers all six
only to a server that said protocol ≥ 10 (`frames::PROTOCOL_GENS`).

### Protocol 10 frames

- Controller → node: `box_up` (`{assign, idle_secs, max_secs, from_job}`;
  the assign's plan has an empty `argv` and its `job_id` is the box id),
  `box_run` (`{box_id, generation, n, sync: {commit, digest, dirty},
  argv, env, cwd, max_secs, stall_secs}`; the command's job id is
  `<box>-c<n>`), `box_down` (`{box_id, generation, reason}`).
- Node → controller: `box_state` (`{box_id, generation, phase:
  preparing|held|running|ended, reason, at, idle_until, max_until, runs,
  sync_digest}`; `reason: "idle_soon"` is the idle notice) and
  `lineage_status` (`{at, lineages: [{key, name, lineage, head_seq,
  head_commit, bytes, last_used, forks}], truncated}`, at most 16 scratch
  lineages per project, newest first).
- Lineage names: `trusted`, `scratch/<ref>`, `release/<build>`,
  `setup/<64 hex>` (`gens::parse_lineage`). A ref is a branch name or
  `wt:<worktree>`; on disk it is `r-<32 hex of its SHA-256>`.

### Receipt v3 (`frames::Receipt`, `v: 3`, BK-5)

v3 = v2 plus two keys, **always present**: `kind` (`job` or `box`) and
`cache` (`null` when the job built cold). `Receipt::check_version` refuses
a v3 without `run`, `kind` or the `cache` key, refuses `kind`/`cache` on
v1 or v2, and ties `cache` to the warmth: a generation means `warmth:
gen`, and `gen` means a generation. v1 and v2 bodies are unchanged.

`cache` (`ci::CacheFacts`, every key present, unknown keys refused):

| Field | Type | Meaning |
|---|---|---|
| `lineage` | text | as above |
| `seq` | u64 ≥ 1 | the generation's place in its lineage |
| `gen_digest` | digest | provenance digest, below |
| `parent_digest` | digest \| null | null for a chain's first (cold) generation |
| `producer` | object | `{job_id, attempt, receipt_digest, commit}`: the job whose fork became the generation; `receipt_digest` is `sha256:` + the signer's receipt id (SHA-256 of the signed body) |
| `ref_checked` | `node` \| `controller` \| null | set exactly for `trusted` |

`gen_digest = "sha256:" + hex(SHA-256(canonical JSON of {lineage,
parent_digest, producer_receipt_digest, seq}))`. It is not a hash of the
cache's bytes. Vectors (Rust `ci::tests::gen_digest_golden`, computed with
Python's `hashlib`):

- `trusted`, seq 41, parent `sha256:f…f`, producer `sha256:e…e` →
  `sha256:d2004a08caa54f8433ab2b293c9a0b480a74e04004a78de88a552ec3d2b095a6`;
- `trusted`, seq 1, parent `null`, producer `sha256:e…e` →
  `sha256:58c923c7b71374b674ad35d3f13c49f6acafe2d5cdec57f5e3d9eb64692c6238`.

Today's signers refuse v3: an older one by its version check, this one by
name until B3 walks the chain ("receipt v3: this k2-signer checks receipt
v2 only"), and always `kind: box`. Both signers refuse a plan that sets
`cache_commit`, `cache_ref` or `setup`; `k2sign.py` also refuses `gpu`,
`gpu_mem` and `cpus_burst` (its `PLAN_KEYS` lists every `Plan` field
again, so the drift test in `k2sign-s3b.sh` passes).

### Signer policy: `warm`

`[build.<name>]` gains `warm = "private"` (the default, never written) or
`"signed-chain"`, and `warm_max_chain` (1–50, default 10, only with
`signed-chain`). An older signer refuses a policy that sets either
(unknown key).

### Lanes: `release_cache`

`LaneTemplate.release_cache` (absent = false, and absent from the file
unless set). Turning it on is a widening, so it is the owner's, in person.
A release fork is kept on the node only when this **and** the client's
`ci.toml` `[release.builds.X] warm = true` say so (BK-7).

### `.k2/ci.toml` version 2 (BK-9)

`version = 2` allows the new keys; `version = 1` parses exactly as before
and refuses each new key by name (`checks.unit.mem_auto needs version =
2`). `k2 ci init` still writes version 1. An older daemon refuses a
version 2 file (its strict parse names the key, or says "version 2 (this
build knows 1)"), so the daemon ships before any repo uses the keys.

| Key | Rule |
|---|---|
| `[setup.<name>]` | `argv` (non-empty), `inputs` (repo files that key it, at least one), `paths` (repo-relative or `$CARGO_HOME/…`, at least one), `needs` |
| `checks.<c>.setup` | `[setup.*]` names; an unknown one is `setup_not_found` |
| `checks.<c>.mem_auto` | bool; `mem_gb` is the fallback |
| `checks.<c>.cpus_burst` | above `cpus`, which it needs |
| `checks.<c>.tests_failed_exit` | 1–255 (nextest: 100) |
| `[[quarantine]]` | `check` (a `[checks.*]`), `test` (1–500 bytes), `why` (1–300), `until` (`YYYY-MM-DD`, at most 30 days ahead at verdict time); a bad entry is `ci_quarantine_invalid` |
| `statuses.pr_comment` | bool; `true` needs `pr_comment_token_secret` (its own token, never `token_secret`) and a `[watch]` |
| `release.builds.<b>.warm` | bool (one of BK-7's two opt-ins) |

There is no key for trusted branches: they are server state
(`ci_workspaces`), so a PR can't name its own branch trusted. The default
is the first `[watch]` branch without a glob, else `main`
(`CiConfig::default_trusted_branch`). A bare `*` stays refused.

### Migrations (reserved)

| Number | Name | Slice |
|---|---|---|
| 0295 | `ci_test_history` | T1 |
| 0296 | `compute_job_stats` | O2 |
| 0297 | `ci_monitors` | O1 |
| 0298 | `compute_boxes` | X1 |
| 0300 | `ci_workspaces.trusted_branches_json` | B2 |

### Refusal codes

`gens::codes`: `cache_trust_needs_person`, `cache_gens_needs_wall`,
`warm_cache_unproven`, `mem_auto_no_history`, `setup_not_found`,
`ci_quarantine_invalid`. `boxes::codes`: `box_limit`, `box_no_secrets`,
`box_needs_node_update`, `hold_needs_no_secrets`, `box_ended`,
`node_lost`.
