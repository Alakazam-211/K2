-- 0209: Sew chat for agents (`k2 chat`), slice S30 core: K16 data model
-- (plan-sew-s30-s32-on-k2-v1 §1 S30, §4 K16/K18). Built 2026-10-15 as
-- 0167; renumbered for 0.46.4 (0204–0208 are the compute lanes'). The
-- "Manage Sew" switch `projects.agents_can_manage_chat` is NOT here: it
-- shipped with S29's 0166 (`sew_server`). Renumber freely on a slip.
--
-- Two locks: Sew roles decide WHERE an agent may post (act-as, Sew's own
-- check); these tables decide WHAT it may do in each Space (its level and
-- whether its posts wait for an approval).
--
-- sew_chat_access   One row per (agent workspace, Space). No row = no chat
--     in that Space. level: read < reply < send < manage. approval:
--     'required' queues every send/reply/dm/edit for a decision; 'none'
--     posts at once. Never part of update_project_setting's allowlist.
--
-- sew_chat_actions  Every chat WRITE attempt, before anything reaches Sew
--     ("no row, no send"), plus K2's audit. NO message content: only the
--     body's sha256 and byte count. `client_key` is the idempotency key the
--     CLI repeats on a retry; a live attempt (sending, posted, queued,
--     approved, denied, cancelled) is unique per (workspace, key), so a
--     retry replays its result. A refused or failed attempt may be retried
--     under the same key (Sew dedups on `Idempotency-Key: k2c-<key>`).
--     `action` leaves room for KM1 (answer|decline|void|ask).
--
-- sew_chat_outbox   The approval queue (`out_…`). `body` exists only while
--     the item is queued and is cleared on every decision (K2 keeps no chat
--     text after a decision). status: queued → approved | denied |
--     cancelled (an expired item is denied by `system:expired`). After an
--     approval `delivery` says whether the post reached Sew. decided_kind
--     is 'person' (vault confirm, `confirm_method`) or 'agent' (labelled as
--     an agent's approval, never person proof: KM13 rev 148). on_behalf_of
--     is the person's chat username, on_behalf_person their Sew external
--     id (`k2:owner` / `k2:principal:<uuid>`): that person never approves
--     the item (builder decision 3). message_kind,
--     wake and response are KM1-ready optional fields (NULL = plain text).
--
-- sew_chat_settings  Per-box chat policy. `approvals.by` = 'people'
--     (default when absent) or 'people+agents'.
--
-- sew_chat_on_behalf  The people who let one assistant workspace post as
--     them without an approval (builder decision 9, provisional). No row =
--     every post an assistant makes as that person queues. `person` is the
--     Sew external id (`k2:owner` or `k2:principal:<uuid>`). Written only
--     by that person with a vault confirm (`chat-act-as`).
--
-- sew_chat_audit  K2's record of chat administration: agent access grants
--     and changes (K19), community management through act-as (K20) and
--     on-behalf opt-ins. No message content. `actor_kind` says what the
--     caller was: a Connect login (a person channel), the owner token (not
--     proof a person acted), an agent passport, or the system.
CREATE TABLE IF NOT EXISTS sew_chat_access (
  project_id TEXT NOT NULL,
  space_id   TEXT NOT NULL,
  space_slug TEXT NOT NULL,
  level      TEXT NOT NULL CHECK (level IN ('read','reply','send','manage')),
  approval   TEXT NOT NULL DEFAULT 'none' CHECK (approval IN ('none','required')),
  updated_by TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (project_id, space_id)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS sew_chat_actions (
  id           TEXT PRIMARY KEY NOT NULL,
  project_id   TEXT NOT NULL,
  k2_session   TEXT NOT NULL,
  action       TEXT NOT NULL CHECK (action IN
               ('send','reply','dm','edit','delete','answer','decline','void','ask')),
  space_id     TEXT,
  channel_id   TEXT,
  target_id    TEXT,
  addr         TEXT NOT NULL,
  on_behalf_of TEXT,
  level        TEXT,
  approval     TEXT,
  addressed    INTEGER,
  status       TEXT NOT NULL CHECK (status IN
               ('refused','sending','posted','failed','queued','approved','denied','cancelled')),
  reason       TEXT,
  client_key   TEXT NOT NULL,
  body_sha256  TEXT,
  body_bytes   INTEGER,
  outbox_id    TEXT,
  message_id   TEXT,
  sew_status   INTEGER,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_sew_chat_actions_live_key
  ON sew_chat_actions (project_id, client_key)
  WHERE status IN ('sending','posted','queued','approved','denied','cancelled');
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sew_chat_actions_project_created
  ON sew_chat_actions (project_id, created_at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS sew_chat_outbox (
  id             TEXT PRIMARY KEY NOT NULL,
  action_id      TEXT NOT NULL,
  project_id     TEXT NOT NULL,
  k2_session     TEXT NOT NULL,
  agent_name     TEXT NOT NULL,
  action         TEXT NOT NULL,
  space_id       TEXT,
  channel_id     TEXT NOT NULL,
  target_id      TEXT,
  addr           TEXT NOT NULL,
  on_behalf_of   TEXT,
  on_behalf_person TEXT,
  broadcast      INTEGER NOT NULL DEFAULT 0,
  client_key     TEXT NOT NULL,
  body           TEXT,
  body_sha256    TEXT NOT NULL,
  body_bytes     INTEGER NOT NULL,
  message_kind   TEXT,
  wake           INTEGER,
  response       TEXT,
  status         TEXT NOT NULL CHECK (status IN ('queued','approved','denied','cancelled')),
  decided_by     TEXT,
  decided_kind   TEXT CHECK (decided_kind IS NULL OR decided_kind IN ('person','agent','system')),
  confirm_method TEXT,
  note           TEXT,
  delivery       TEXT CHECK (delivery IS NULL OR delivery IN ('posted','failed')),
  message_id     TEXT,
  sew_code       TEXT,
  queue_reason   TEXT NOT NULL DEFAULT 'approval_required'
                 CHECK (queue_reason IN ('approval_required','on_behalf')),
  created_at     INTEGER NOT NULL,
  expires_at     INTEGER NOT NULL,
  decided_at     INTEGER
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sew_chat_outbox_status_created
  ON sew_chat_outbox (status, created_at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sew_chat_outbox_project
  ON sew_chat_outbox (project_id, created_at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS sew_chat_settings (
  key        TEXT PRIMARY KEY NOT NULL,
  value      TEXT NOT NULL,
  updated_by TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS sew_chat_on_behalf (
  person         TEXT NOT NULL,
  project_id     TEXT NOT NULL,
  set_by         TEXT NOT NULL,
  confirm_method TEXT NOT NULL,
  created_at     INTEGER NOT NULL,
  PRIMARY KEY (person, project_id)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS sew_chat_audit (
  id          TEXT PRIMARY KEY NOT NULL,
  project_id  TEXT,
  actor       TEXT NOT NULL,
  actor_kind  TEXT NOT NULL CHECK (actor_kind IN ('login','owner_token','agent','system')),
  op          TEXT NOT NULL,
  space_id    TEXT,
  target      TEXT,
  detail      TEXT,
  outcome     TEXT NOT NULL CHECK (outcome IN ('ok','refused','failed')),
  reason      TEXT,
  created_at  INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sew_chat_audit_created ON sew_chat_audit (created_at);
