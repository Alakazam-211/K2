-- 0286: the node key chain and the taint memory (0.46.11, the 11-07 trust
-- and secrets review, fix list ci-trust-review-fixes-0.46.11 H2 + M4).
-- Renumber freely on a slip (and edit the migration-order tests). The store
-- is `crate::compute::trust`.
--
--   compute_key_rotations  every node re-key this server applied (after a
--                          person approved it, or, for a seat node, after the
--                          owner's roster listed the new key): old key → new
--                          key. A key revoked as compromised revokes every key
--                          it handed over to, and a key whose ancestor was
--                          revoked as compromised counts as revoked (H2).
--   compute_tainted_keys   node keys this server ever saw say they ran a job
--                          without the job wall (or ran with --dev, no wall,
--                          or a job-readable key). Never forgotten: a later
--                          statement saying "clean" doesn't clear it; only a
--                          new key does (M4).
--   compute_owner_attestations  (client) the owners' attestations that came
--                          with a seat offer, verified on arrival.
CREATE TABLE IF NOT EXISTS compute_key_rotations (
  new_fp     TEXT PRIMARY KEY NOT NULL,
  old_fp     TEXT NOT NULL,
  node_id    TEXT,
  new_epoch  INTEGER NOT NULL DEFAULT 0,
  at         INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_key_rotations_old ON compute_key_rotations(old_fp);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_tainted_keys (
  node_fp  TEXT PRIMARY KEY NOT NULL,
  why      TEXT NOT NULL,
  at       INTEGER NOT NULL
);
--> statement-breakpoint
-- Client side: the node owners' attestations (V12) this server received with
-- a seat offer, each verified on arrival with the owner's pinned roster key
-- and named by an entry of that owner's roster (an evidence bundle carries
-- them; the signer refuses an owner_attested node without one).
CREATE TABLE IF NOT EXISTS compute_owner_attestations (
  digest       TEXT PRIMARY KEY NOT NULL,
  key_fp       TEXT NOT NULL,
  node_fp      TEXT NOT NULL,
  body         TEXT NOT NULL,
  sig          TEXT NOT NULL,
  received_at  INTEGER NOT NULL
);
