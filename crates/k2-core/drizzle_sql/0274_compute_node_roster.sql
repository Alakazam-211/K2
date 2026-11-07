-- 0274: node rosters, lanes, seats and signer pairing (0.46.10,
-- prd-ci-on-compute-for-clients-v1 C1 + C3a, vs-live V3, V11–V13, V17,
-- V19–V22, V25). Renumber freely on a slip (and edit the migration-order
-- tests). The store is `crate::compute::trust`.
--
-- Node side of this server (the node OWNER):
--   compute_node_statements  the latest self-statement each node sent
--                            (verified against the pinned key on arrival)
--   compute_roster_entries   the owner's working list of node keys: one row
--                            per key, what the next signed roster says
--   compute_node_rosters     every signed node roster this server issued
--                            (source 'self') or received from an owner it
--                            trusts (source 'node' / 'peer'), by roster key
--   compute_lanes            approved lane templates
-- Both sides:
--   compute_trust_requests   held changes a person approves in person:
--                            roster changes and lane templates (owner),
--                            seat offers (owner), seat requests (client)
--   compute_seats            seats this server offered (side 'owner') or
--                            holds on another owner's nodes (side 'client')
-- Client side:
--   compute_owner_trust      node owners whose roster key a person pinned
--                            here, the high-water roster seq (V13), and the
--                            secrets consent (V25, `secret_owner_not_trusted`)
--   compute_key_revocations  node keys an owner's roster revoked
--   ci_signers               signing machines paired with this server (V17)
CREATE TABLE IF NOT EXISTS compute_node_statements (
  node_id      TEXT PRIMARY KEY NOT NULL,
  node_fp      TEXT NOT NULL,
  body         TEXT NOT NULL,
  sig          TEXT NOT NULL,
  digest       TEXT NOT NULL,
  received_at  INTEGER NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_roster_entries (
  node_fp            TEXT PRIMARY KEY NOT NULL,
  node_id            TEXT,
  name               TEXT NOT NULL,
  node_spki          TEXT NOT NULL,
  key_epoch          INTEGER NOT NULL DEFAULT 0,
  statement_digest   TEXT NOT NULL,
  status             TEXT NOT NULL CHECK (status IN ('active','retired','revoked')),
  revoked_reason     TEXT CHECK (revoked_reason IS NULL OR revoked_reason IN ('compromised','key_lost','other')),
  valid_from         INTEGER NOT NULL,
  valid_to           INTEGER,
  taint_basis        TEXT CHECK (taint_basis IS NULL OR taint_basis IN ('self_clean','rekeyed','owner_attested')),
  attestation_body   TEXT,
  attestation_sig    TEXT,
  host               TEXT NOT NULL,
  provider           TEXT,
  site               TEXT,
  admin_reach_json   TEXT NOT NULL DEFAULT '[]',
  pod                TEXT,
  point              INTEGER,
  updated_at         INTEGER NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS compute_roster_entries_active_name ON compute_roster_entries(name) WHERE status = 'active';
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_node_rosters (
  key_fp       TEXT NOT NULL,
  seq          INTEGER NOT NULL,
  owner_id     TEXT NOT NULL,
  body         TEXT NOT NULL,
  sig          TEXT NOT NULL,
  digest       TEXT NOT NULL,
  issued_at    INTEGER NOT NULL,
  received_at  INTEGER NOT NULL,
  source       TEXT NOT NULL CHECK (source IN ('self','node','peer')),
  PRIMARY KEY (key_fp, seq)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_lanes (
  name           TEXT PRIMARY KEY NOT NULL,
  template_json  TEXT NOT NULL,
  approved_by    TEXT NOT NULL,
  approved_via   TEXT NOT NULL,
  approved_at    INTEGER NOT NULL,
  request_id     TEXT
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_trust_requests (
  id                  TEXT PRIMARY KEY NOT NULL,
  kind                TEXT NOT NULL CHECK (kind IN ('roster','lane','seat_offer','seat_request')),
  subject             TEXT NOT NULL,
  body_json           TEXT NOT NULL,
  digest              TEXT NOT NULL,
  summary             TEXT NOT NULL,
  reason              TEXT,
  proposed_by         TEXT NOT NULL,
  proposer_workspace  TEXT,
  proposed_at         INTEGER NOT NULL,
  state               TEXT NOT NULL CHECK (state IN ('pending','approved','denied','withdrawn','expired','failed')),
  decided_by          TEXT,
  decided_via         TEXT,
  decided_at          INTEGER,
  deny_reason         TEXT,
  expires_at          INTEGER NOT NULL,
  result_json         TEXT
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_trust_requests_state ON compute_trust_requests(state, kind);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_seats (
  id               TEXT PRIMARY KEY NOT NULL,
  side             TEXT NOT NULL CHECK (side IN ('owner','client')),
  peer_fp          TEXT NOT NULL,
  peer_host        TEXT NOT NULL,
  owner_id         TEXT NOT NULL,
  owner_key_fp     TEXT,
  client_fp        TEXT NOT NULL,
  client_label     TEXT NOT NULL,
  client_url       TEXT NOT NULL DEFAULT '',
  lane             TEXT NOT NULL,
  nodes_json       TEXT NOT NULL DEFAULT '[]',
  state            TEXT NOT NULL CHECK (state IN ('requested','offered','active','suspended','ended','refused')),
  statement_body   TEXT,
  statement_sig    TEXT,
  roster_seq       INTEGER,
  code_binding     TEXT,
  request_id       TEXT,
  batch_id         TEXT,
  note             TEXT,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_owner_trust (
  key_fp           TEXT PRIMARY KEY NOT NULL,
  roster_key       TEXT NOT NULL,
  owner_id         TEXT NOT NULL,
  peer_fp          TEXT NOT NULL,
  pinned_by        TEXT NOT NULL,
  pinned_via       TEXT NOT NULL,
  pinned_at        INTEGER NOT NULL,
  secrets_consent  INTEGER NOT NULL DEFAULT 0,
  consent_by       TEXT,
  consent_at       INTEGER,
  high_seq         INTEGER NOT NULL DEFAULT 0
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_key_revocations (
  node_fp     TEXT PRIMARY KEY NOT NULL,
  key_fp      TEXT NOT NULL,
  reason      TEXT NOT NULL CHECK (reason IN ('compromised','key_lost','other')),
  roster_seq  INTEGER NOT NULL,
  at          INTEGER NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS ci_signers (
  signer_fp    TEXT PRIMARY KEY NOT NULL,
  signer_spki  TEXT NOT NULL,
  label        TEXT NOT NULL,
  paired_by    TEXT NOT NULL,
  paired_via   TEXT NOT NULL,
  paired_at    INTEGER NOT NULL,
  removed_at   INTEGER
);
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN key_epoch INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN prev_fingerprint TEXT;
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN rekeyed_at INTEGER;
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN seat_id TEXT;
