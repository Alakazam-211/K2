-- 0171: App box services (0.46.0; prd-k2-app-runtime-v1 §5.6, plan-0461
-- "walled Apps can't reach box-local services"). A walled App reaches a
-- service on this box's loopback (Sew's client API, its voice signalling)
-- only through an owner-approved forward into its wall: pasta forwards the
-- container's 127.0.0.1:<app_port> to host 127.0.0.1:<host_port>, and the
-- App's own nft rule admits that host port only while it is approved.
--
-- 0.46.0 takes 0171; the 0.46.1 lanes (held 0171-0175) shift up one. The
-- 0.46.1 `app_widenings` rebuild (lane 2, `app_door`) must keep
-- 'box_service' in its kind CHECK, or its INSERT … SELECT fails on a box
-- that holds one.
--
-- app_widenings (rebuilt) = 0168's table with one more kind:
--     box_service   new box services for an App ({"boxServices": {"add":
--                   [{name, hostPort, appPort, proto}]}}); approving one
--                   opens its host port in the App's nft rule at once.
--     0168's CHECK can't be altered in SQLite, so the table is copied into
--     a new one with the wider CHECK. Nothing references app_widenings.
-- app_box_services = per App, per named service. `status`:
--     allowed  approved by the owner (the nft rule admits host_port);
--     pending  asked for, waiting on the owner (`request_id` names the
--              `app_widenings` row, kind `box_service`). Its forward is
--              already in the wall (rendered at start) but the door is shut.
--     Revoking deletes the row (applies at once). `preset` names the known
--     service the port was read from (`sew-api`, `sew-signal`), if any.
CREATE TABLE IF NOT EXISTS app_widenings_0171 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('config', 'publish_run', 'app_key', 'clean_mode', 'egress', 'egress_mode', 'box_service')),
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
INSERT INTO app_widenings_0171 SELECT id, kind, app_id, service_id, app_label, workspace_id, body_json, diff_json, summary,
  reasons_json, reason, deployment_id, requested_by, requester_workspace, requester_session, requested_at, expires_at, status,
  decided_by, decided_at, confirm_via, decision_reason, result_json FROM app_widenings;
--> statement-breakpoint
DROP TABLE app_widenings;
--> statement-breakpoint
ALTER TABLE app_widenings_0171 RENAME TO app_widenings;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_widenings_ws_status ON app_widenings (workspace_id, status, requested_at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_widenings_app_status ON app_widenings (app_id, status);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_box_services (
  app_id TEXT NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  host_port INTEGER NOT NULL CHECK (host_port BETWEEN 1024 AND 65535),
  app_port INTEGER NOT NULL CHECK (app_port BETWEEN 1024 AND 65535),
  proto TEXT NOT NULL DEFAULT 'tcp' CHECK (proto IN ('tcp')),
  preset TEXT,
  source TEXT NOT NULL CHECK (source IN ('manifest','request')),
  status TEXT NOT NULL CHECK (status IN ('allowed','pending')),
  request_id TEXT,
  added_by TEXT NOT NULL,
  added_at INTEGER NOT NULL,
  approved_by TEXT,
  approved_at INTEGER,
  PRIMARY KEY (app_id, name)
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS app_box_services_app_port ON app_box_services (app_id, app_port);
