-- 0213: App users' personal wallets (prd-app-user-llm-logins-v1 rev 2,
-- 0.46.3, Rosson 2026-10-20).
--
-- * `person_wallet_items`: one row per item in an App user's wallet. The
--   owner is a skin principal (skin.db `principals.id`; no FK across
--   files). `item_type` + `provider` keep the table open to other
--   third-party logins later; v1 writes `llm_subscription` (claude, codex,
--   grok) and `llm_api_key` (+ gemini). No secret is ever stored here:
--   subscriptions live in the item's slot (`~/.k2/llm-accounts/<tool>/<id>/`)
--   while a session runs and sealed in the vault's `person` scope
--   otherwise (`sealed` = 1); API keys only ever in the vault.
--   `dormant_since`: the person lost App access; deleted 30 days later.
--   Removed items are deleted (row, slot, vault secret), never Trash.
-- * `person_wallet_use`: the person pin, one item per (person, provider):
--   what their personal agents run on.
-- * `person_agents`: "this agent workspace is <person>'s own agent, used
--   through <App>". One person per workspace.
-- * `app_wallet_settings`: per App, wallet on/off (the `wallet:self` cap)
--   and the fallback to the company plan. Both default off.
CREATE TABLE IF NOT EXISTS person_wallet_items (
  id TEXT PRIMARY KEY,
  principal_id TEXT NOT NULL,
  item_type TEXT NOT NULL,
  provider TEXT NOT NULL,
  label TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'not_set_up',
  detail TEXT,
  plan TEXT,
  expires_at INTEGER,
  sealed INTEGER NOT NULL DEFAULT 0,
  added_via TEXT,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER,
  refreshed_at INTEGER,
  dormant_since INTEGER
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS person_wallet_items_label
  ON person_wallet_items (principal_id, provider, label COLLATE NOCASE);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS person_wallet_items_principal ON person_wallet_items (principal_id);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS person_wallet_use (
  principal_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  item_id TEXT NOT NULL,
  set_at INTEGER NOT NULL,
  PRIMARY KEY (principal_id, provider)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS person_agents (
  project_id TEXT PRIMARY KEY,
  principal_id TEXT NOT NULL,
  service_id TEXT NOT NULL,
  set_by TEXT,
  set_at INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS person_agents_principal ON person_agents (principal_id);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_wallet_settings (
  service_id TEXT PRIMARY KEY,
  enabled INTEGER NOT NULL DEFAULT 0,
  fallback INTEGER NOT NULL DEFAULT 0,
  set_by TEXT,
  set_at INTEGER NOT NULL
);
