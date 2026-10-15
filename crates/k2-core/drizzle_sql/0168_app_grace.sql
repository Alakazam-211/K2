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
-- app_requests              = held requests (§5.6): what an agent (or a
--                             non-owner) proposed; the owner approves with a
--                             fresh confirm, or denies; the requester may
--                             withdraw. Expire after 7 days.
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
CREATE TABLE IF NOT EXISTS app_requests (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('config', 'publish_run', 'app_key', 'clean_mode')),
  service_id TEXT,
  app_label TEXT NOT NULL,
  workspace_id TEXT NOT NULL,
  body_json TEXT NOT NULL,
  diff_json TEXT NOT NULL,
  reasons_json TEXT NOT NULL,
  requested_by TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('held', 'approved', 'denied', 'withdrawn', 'expired', 'failed')),
  result_json TEXT,
  decided_by TEXT,
  decided_via TEXT,
  decided_at INTEGER,
  reason TEXT,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_requests_ws_state ON app_requests (workspace_id, state, created_at);
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
