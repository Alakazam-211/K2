-- 0303: signed approvals, the daemon's side (prd-signed-approvals-v1 rev 2,
-- slice S1). Reserved by K2 for this lane; renumber freely on a slip (and
-- edit the migration-order tests). The store is `crate::approve_requests`;
-- the routes are k2-daemon's `approve`.
--
-- The daemon is only the carrier: it keeps the payload bytes exactly as the
-- agent sent them (never re-written, never parsed for meaning beyond its
-- shape), shows a card, pushes to the person's phone and hands the
-- approval record back. It never decides whether an approval is valid:
-- the relying party verifies the signature against keys it pinned itself.
--
-- `approval_requests`: one row per ask.
--   `person_id`   the asked person's stable login id (`ConnectUser.id`,
--                 never a username: a removed and re-added name inherits
--                 nothing). `person_login` is the name when asked (display
--                 and push routing only).
--   `payload`     the bytes, exactly as received (at most 16 KiB).
--   `kind`, `rp`, `nonce`, `answer_by` copied from the payload's shape
--                 check, for lists, the card and expiry. Not trusted.
--   `state`       waiting → approved | declined | expired | cancelled.
--   `record`      the approval record exactly as the person's device
--                 posted it (approved only). `key_id`, `alg` copied from it.
--   `device`      the device's own name for itself (display only).
--   UNIQUE (rp, nonce): a relying party's nonce is carried once.
CREATE TABLE IF NOT EXISTS approval_requests (
  id              TEXT PRIMARY KEY,
  workspace_id    TEXT NOT NULL,
  session_id      TEXT,
  person_id       TEXT NOT NULL,
  person_login    TEXT NOT NULL,
  payload         BLOB NOT NULL,
  payload_sha256  TEXT NOT NULL,
  kind            TEXT NOT NULL,
  rp              TEXT NOT NULL,
  nonce           TEXT NOT NULL,
  answer_by       INTEGER NOT NULL,
  state           TEXT NOT NULL DEFAULT 'waiting'
                  CHECK (state IN ('waiting','approved','declined','expired','cancelled')),
  conversation_id TEXT,
  card_id         TEXT,
  record          TEXT,
  key_id          TEXT,
  alg             TEXT,
  device          TEXT,
  answered_by     TEXT,
  answered_via    TEXT,
  created_at      INTEGER NOT NULL,
  answered_at     INTEGER,
  UNIQUE (rp, nonce)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_approval_requests_person ON approval_requests (person_id, state);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_approval_requests_workspace ON approval_requests (workspace_id, created_at);
--> statement-breakpoint
-- `approval_agents`: the `approve.request` permission. A workspace listed
-- here may ask people for signed approvals; off (absent) by default. Set by
-- an Owner or Admin (`POST /cli/approve/agents`, or the permission card
-- `approve.request`).
CREATE TABLE IF NOT EXISTS approval_agents (
  workspace_id TEXT PRIMARY KEY,
  granted_by   TEXT NOT NULL,
  granted_at   INTEGER NOT NULL
);
--> statement-breakpoint
-- `approval_ask_rules`: who an agent may ask (Rosson's question 3). By
-- default an agent may ask only the person it works for (the owner: the
-- logins the owner linked). Anyone else allows it here, for themselves;
-- anyone (the owner included) can block an agent from asking them.
CREATE TABLE IF NOT EXISTS approval_ask_rules (
  person_id    TEXT NOT NULL,
  workspace_id TEXT NOT NULL,
  rule         TEXT NOT NULL CHECK (rule IN ('allow','block')),
  set_at       INTEGER NOT NULL,
  PRIMARY KEY (person_id, workspace_id)
);
