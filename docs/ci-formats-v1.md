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
- **Protocol 9** is `frames::PROTOCOL_CI`. `frames::PROTOCOL` stays 8 until
  the commit that makes k2-node offer the first protocol-9 feature: one
  bump for all node parts. Until then nothing sends these formats, and
  protocol 8 is exactly 0.46.9's.
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

## 8. Evidence bundle v2 (`evidence::EvidenceBundle`) and signer policy

### Evidence bundle

`k2 ci evidence` writes this file, and `k2 signer sign --evidence` reads
it. It is not signed itself: each part carries its own signature, and the
signer trusts only its own pins. It is closed JSON, at most 8 MiB.

| Field | Type | Meaning |
|---|---|---|
| `v`, `kind` | 2, `"k2-ci-evidence"` | a v1 bundle is refused by name |
| `project` | name | the signer project |
| `version` | string \| null | `null` for a CI run that signs nothing |
| `run_id` | name | |
| `controller_fp` | fp | |
| `created_at` | i64 | |
| `source` | object | `commit` and `tree` (40 hex), plus `commit_object` (base64 `git cat-file commit`, or `null`); when present it must hash to `commit` |
| `receipts` | label → receipt | 1–32 entries |
| `statements`, `rotations`, `attestations`, `rosters`, `seats` | lists of `{body, sig}` | |
| `pairing_pins` | fp → base64 SPKI | what the client server pinned at pairing: the PRD §4.1.6 cross-check |

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
  - `label` (`L1`), and `verify_label` (`V1`) for a dual build.

  Labels are unique across builds.

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
