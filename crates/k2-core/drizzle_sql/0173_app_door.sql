-- 0173: the 0.46.1 App door (prd-k2-app-runtime-v1 §5.3–5.6, S19, S20,
-- S42; plan-0461-lanes-v1 lane 2). 0171/0172 are lane 1's (app_walls,
-- app_resources), 0174 lane 3's (app_databases); the integrator renumbers
-- on slips (never below a shipped max).
--
-- app_widenings (rebuilt) = every 0.46.1 held-request kind. 0168's CHECK
--     can't be altered in SQLite, so the table is copied into a new one with
--     the wider CHECK. Nothing references app_widenings, so the rebuild
--     cascades nowhere. New kinds (k2_core::app_grace::requests):
--       deploy      a deploy whose caps go past the App's approved set
--       install     an App's first install (it sets the approved set)
--       db_raw      the raw private-database connection (§5.7)
--       overcommit  memory past the admission headroom (§6.6)
--       manage      an App change from an agent without "Allow this agent
--                   to manage Apps" (S42; §5.11.1 "every change is held")
--       quota       raising an App's §5.4 quotas past the defaults
-- apps.suspended_at / suspended_by / suspended_reason
--     = `k2 app suspend` (§5.4): non-NULL suspended_at = suspended. Every
--       live App pass was revoked in the same step; the gateway refuses the
--       App (401) and deploys are frozen until the owner resumes.
-- apps.approved_caps_json
--     = the App's approved cap set (§5.6): a JSON array of catalog cap
--       names the owner approved (first install or a held deploy). NULL =
--       none recorded: an App with a live Seed is grandfathered at its live
--       Seed's caps on first use; an App with none is a first install.
-- apps.quota_json
--     = the owner's per-App override of the §5.4 quota defaults
--       ({"rps":N,"burst":N,"sockets":N,"bodyBytes":N,"uploadBytes":N,
--       "subscriptions":N}); NULL = the defaults.
CREATE TABLE IF NOT EXISTS app_widenings_0173 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('config', 'publish_run', 'app_key', 'clean_mode', 'egress', 'egress_mode',
                                     'deploy', 'install', 'db_raw', 'overcommit', 'manage', 'quota')),
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
INSERT INTO app_widenings_0173 SELECT id, kind, app_id, service_id, app_label, workspace_id, body_json, diff_json, summary,
  reasons_json, reason, deployment_id, requested_by, requester_workspace, requester_session, requested_at, expires_at, status,
  decided_by, decided_at, confirm_via, decision_reason, result_json FROM app_widenings;
--> statement-breakpoint
DROP TABLE app_widenings;
--> statement-breakpoint
ALTER TABLE app_widenings_0173 RENAME TO app_widenings;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_widenings_ws_status ON app_widenings (workspace_id, status, requested_at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_widenings_app_status ON app_widenings (app_id, status);
--> statement-breakpoint
ALTER TABLE apps ADD COLUMN suspended_at INTEGER;
--> statement-breakpoint
ALTER TABLE apps ADD COLUMN suspended_by TEXT;
--> statement-breakpoint
ALTER TABLE apps ADD COLUMN suspended_reason TEXT;
--> statement-breakpoint
ALTER TABLE apps ADD COLUMN approved_caps_json TEXT;
--> statement-breakpoint
ALTER TABLE apps ADD COLUMN quota_json TEXT;
