-- 0168: the 0.46.0 App "grace" release (prd-k2-app-runtime-v1 vs-live 2
-- §6 S45, §8.2, S13; Rosson's cross-over plan 2026-10-14: 0.46.0 adds the
-- new ways and removes nothing; 0.46.1 is the cut-over). 0166/0167 are
-- Sew's; the integrator renumbers on slips (never below a shipped max).
--
-- api_keys.app_service_id   = the App-scoped key (S45): bound to exactly one
--                             published service (K2_APP_ID). NULL = an
--                             ordinary /v1 key. A bound key never resolves
--                             as an ordinary /v1 principal; it opens only
--                             /v1/apps/<that id>/… .
-- api_keys.app_key          = JSON {scopes, rooms, roles}: what the key may
--                             do (users | rooms | fetch) and the ceiling of
--                             rooms/roles it may hand out. NULL on a bound
--                             key = nothing (fail closed).
-- projects.agents_can_publish_open_apps
--                           = the nested switch "…and publish open Apps"
--                             (§5.11.1). DEFAULT 0. Written only by
--                             agent-access/set {toggle: apps_open}; off
--                             whenever "Allow this agent to manage Apps" is.
-- apps.config_json          = the §4.4 App config fields that have no column
--                             (resources, egress, ports, recycle, risk, db).
--                             NULL = the defaults.
-- app_widenings             = held App requests (§5.6), one table for every
--                             kind: the grace kinds (config, publish_run,
--                             app_key, clean_mode) and S15's egress kinds
--                             (egress, egress_mode; their host list is in
--                             0169). An agent or a non-owner proposes; the
--                             owner approves in person with a fresh confirm
--                             (vault action `app-approve` bound to the id);
--                             an Owner/Admin denies; the requester may
--                             withdraw its own. Expire after 7 days.
--                             app_id NULL = a `--cmd` service with no
--                             runtime App row (service_id names it).
-- app_owner_token_uses      = per published service: which routes it called
--                             with the owner token (Linux attribution), so
--                             the 0.46.0 warnings can name them before the
--                             0.46.1 wall takes the token away.
ALTER TABLE api_keys ADD COLUMN app_service_id TEXT;
--> statement-breakpoint
ALTER TABLE api_keys ADD COLUMN app_key TEXT;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_api_keys_app ON api_keys (app_service_id);
--> statement-breakpoint
ALTER TABLE projects ADD COLUMN agents_can_publish_open_apps INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE apps ADD COLUMN config_json TEXT;
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_widenings (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('config', 'publish_run', 'app_key', 'clean_mode', 'egress', 'egress_mode')),
  app_id TEXT REFERENCES apps(app_id) ON DELETE CASCADE,
  service_id TEXT,
  app_label TEXT NOT NULL,
  workspace_id TEXT NOT NULL,
  body_json TEXT NOT NULL,
  diff_json TEXT NOT NULL,
  summary TEXT NOT NULL,
  reasons_json TEXT NOT NULL,
  reason TEXT,
  deployment_id TEXT,
  requested_by TEXT NOT NULL,
  requester_workspace TEXT,
  requester_session TEXT,
  requested_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'withdrawn', 'expired', 'failed')),
  decided_by TEXT,
  decided_at INTEGER,
  confirm_via TEXT,
  decision_reason TEXT,
  result_json TEXT
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_widenings_ws_status ON app_widenings (workspace_id, status, requested_at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_widenings_app_status ON app_widenings (app_id, status);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_owner_token_uses (
  service_id TEXT NOT NULL,
  route TEXT NOT NULL,
  count INTEGER NOT NULL DEFAULT 0,
  refused INTEGER NOT NULL DEFAULT 0,
  first_at INTEGER NOT NULL,
  last_at INTEGER NOT NULL,
  PRIMARY KEY (service_id, route)
);
