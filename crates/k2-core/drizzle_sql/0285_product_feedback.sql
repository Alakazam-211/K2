-- 0285: Send feedback to K2 (0.46.11, prd-k2-product-feedback-v1 PF15).
-- The daemon's durable outbox for feedback people and agents send to
-- Alakazam Labs (`k2 send-feedback`, the top-bar button). Not the tickets
-- tables (`feedback*`, an agent asking its own human). `id` is the
-- clientId (`pf_` + 22 base62) the intake dedupes on. `sender` stays on
-- this box (`owner`, `login:<user>`, `agent:<project id>`,
-- `shell:<project id>`); only `sender_kind` leaves. `body` is already
-- vault-scrubbed. `diagnostics_json` is the exact sections sent (the
-- person's consent), cleared 7 days after `sent`; rows go 90 days after a
-- final state. `product_feedback_prefs` holds a person's remembered
-- "start with the diagnostics box ticked" (PF27), never an agent's.
-- Renumber freely on a slip (and edit the migration-order tests). The
-- store is `crate::product_feedback::store`.
CREATE TABLE IF NOT EXISTS product_feedback_outbox (
  id TEXT PRIMARY KEY NOT NULL,
  created_at INTEGER NOT NULL,
  queued_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  sender_kind TEXT NOT NULL CHECK (sender_kind IN ('person', 'agent')),
  sender TEXT NOT NULL,
  client TEXT NOT NULL CHECK (client IN ('desktop', 'web', 'cli')),
  category TEXT NOT NULL CHECK (category IN ('bug', 'idea', 'praise', 'other')),
  body TEXT NOT NULL,
  contact_email TEXT,
  meta_json TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('queued', 'sending', 'sent', 'failed', 'refused', 'cancelled')),
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER NOT NULL,
  last_error TEXT,
  remote_id TEXT,
  sent_at INTEGER,
  diagnostics_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_product_feedback_outbox_due ON product_feedback_outbox (state, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_product_feedback_outbox_sender ON product_feedback_outbox (sender, created_at);
CREATE TABLE IF NOT EXISTS product_feedback_prefs (
  username TEXT PRIMARY KEY NOT NULL,
  remember_diagnostics INTEGER NOT NULL
);
