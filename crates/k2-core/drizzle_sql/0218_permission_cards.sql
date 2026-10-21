-- 0218: permission cards (prd-agent-settings-cards-v1 rev 3.1; 0.46.3,
-- behind the "Permission cards" Beta switch, Rosson 2026-10-21).
--
-- Additive only. With the switches off (the default) nothing reads these.
-- 0214–0216 are compute sharing, 0217 is held by the disk guard (Slice B,
-- 0.46.4); lane I orders them (final order 0201, 0204–0206, 0211, 0213,
-- 0214–0216, 0218).
--
-- * `projects.agents_can_request_permissions` — the per-agent switch "Can
--   ask for permission cards" (DEFAULT 0 for new and existing workspaces).
--   Written only by POST /cli/agent-access/set {toggle: "permission_cards"}
--   (an Owner or Admin login, or the owner token; never an agent). NOT a
--   field of `Project` (vs-live C8): read by a dedicated getter.
-- * `permission_requests` — one ask for one action with its values (`pr_…`,
--   or the held request's own id). Values and the K2-written card text are
--   fixed at request time (params_hash / text_hash, C35). `state` follows
--   §17.2.4; `pending → applying` is a compare-and-set before any apply
--   (C12). The 24 h block, the 7-day ask limit and the 50-per-server cap
--   are SQL over this table, so they survive a restart.
-- * `permission_notes` — one note per person (or App guest) per request
--   (§4.3, §7.1); "N people asked" counts distinct guest authors.
-- * `addr` — the Thread address the card was posted at (the agent's own
--   conversation): the `[permission:]` outcome turn is delivered there.
ALTER TABLE projects ADD COLUMN agents_can_request_permissions INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS permission_requests (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL,
  session_id TEXT,
  conversation_id TEXT,
  addr TEXT,
  card_id TEXT,
  action TEXT NOT NULL,
  params_json TEXT NOT NULL DEFAULT '{}',
  params_hash TEXT NOT NULL,
  text_hash TEXT NOT NULL,
  why TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending',
  floor TEXT NOT NULL,
  held_ref TEXT,
  batch_id TEXT,
  ask_number INTEGER NOT NULL DEFAULT 1,
  asked_for TEXT,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  answered_at INTEGER,
  answered_by TEXT,
  answered_via TEXT,
  reason TEXT,
  result_message TEXT,
  updated_why TEXT,
  updated_at INTEGER
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_permission_requests_ws_state ON permission_requests (workspace_id, state);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_permission_requests_block ON permission_requests (workspace_id, action, params_hash, created_at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_permission_requests_state_expiry ON permission_requests (state, expires_at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS permission_notes (
  request_id TEXT NOT NULL,
  author TEXT NOT NULL,
  author_kind TEXT NOT NULL,
  author_role TEXT NOT NULL,
  text TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (request_id, author)
);
