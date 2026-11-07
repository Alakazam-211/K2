# k2-signer

K2's release signer for clients (prd-ci-on-compute-for-clients-v1, slice C4).
It runs as k2sign's `_k2sign` role user and shares k2sign's registries
(vs-live V28). It checks K2 Compute evidence bundles v2 against a per-project
policy, the person's pins and the owners' node rosters, and then signs
minisign / Tauri updater files. The crate docs (`src/lib.rs`, `src/verbs.rs`,
`src/pairing.rs`) describe the layout, the verbs and the pairing protocol.

Install: `scripts/k2sign/install-k2-signer.sh --binary <build> --expect-sha256 <independent build's sha256>`.

## Known limits (0.46.10)

These were accepted at the 2026-11-07 security review. They are not fixed in
0.46.10:

- **L2: revocation can lag.** A revocation can take up to
  `roster_max_age_hours` (default 168, max 720) to reach the signer. Roster
  marks move only when evidence registers, and a receipt's `roster_seq_seen`
  is optional.
- **L5: the shared lock is held during slow work.** It is held while up to
  1 GiB of stdin is read and during GitHub calls, so a caller who never closes
  stdin stalls k2sign too. This affects availability only.
- **L7: marks are kept per owner.** Evidence for project A that carries a
  newer roster makes project B's already-registered evidence (with the older
  roster) fail at sign time. It fails safe (B re-registers); this affects
  availability only.
- **L8: "one evidence per version" holds one way only.** k2sign doesn't look
  for k2-signer's v2 evidence (k2-signer refuses a version that has k2sign's
  S3b evidence). The shared one-sha-per-asset registry still prevents the two
  from signing different bytes.
- **L11: GitHub answers are trusted over TLS** with the system trust store,
  so an installed corporate CA could fake them. The read-only token's scope
  isn't checked.
- (L4, `source.repo` / `tag_prefix` reaching GitHub API paths, is fixed: both
  are checked against a strict charset.)
- **Baked-in secrets (S9).** The policy's `secret_sha256` is not checked.
  Receipt v2 records nothing about what a build baked in, so evidence that
  uses it only gets a note.
- **Env values (M1).** Signer policy v1 lists env *names* only. Any env value
  other than `BUILD_COMMIT` / `BUILD_TREE` must therefore be empty, because
  the policy can't pin one.
- **Seat expiry (L3).** Seat statement v1 has no expiry field, so a seat
  expires after the policy's `roster_max_age_hours`.
- **Pairing-pin switch (M7).** It lives in `trust.json`
  (`k2-signer-admin pairing-pins <project> required|optional`), not in the
  frozen policy schema. By default it is required for client projects and
  optional for `k2`.
- **One key per asset.** The policy names one key per asset. K2's U1/U2
  bridge (until SS6) is therefore special-cased: `u1` is allowed only for
  project `k2`.
- **Not built yet:**
  - `check = "inner_binary"` (C8, Q11);
  - the `k2 signer` wrapper in `cli/k2`;
  - the server side of pairing (builder O; see `pairing::server_*`);
  - Linux (`k2signer` user, FIDO2 approval).
