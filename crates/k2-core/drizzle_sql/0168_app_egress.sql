-- 0168: App egress (prd-k2-app-runtime-v1 §4.1, §5.6, §9.2, S15).
-- New tables only; dark until the App runtime preview is on.
--
-- app_egress_policy  one row per App that left the default: mode
--                    `allowlist` (guarded: only listed hosts) or `broad`
--                    (open path: any public host, first-seen audited).
--                    No row = allowlist.
-- app_egress_hosts   the per-App host list. `status`:
--                      allowed  on the allowlist (owner-approved);
--                      pending  asked for, waiting on the owner
--                               (`request_id` names the widening);
--                      seen     the proxy saw the App try it (refused on
--                               an allowlist App; allowed and audited on a
--                               broad one). Narrowing deletes the row.
--                    `host` is a lowercase A-label name, an IP literal or
--                    `*.<suffix>` (any name under the suffix, not the
--                    suffix itself). `hits` = connections let through,
--                    `refused` = connections refused.
-- app_widenings      held widening requests (§5.6): kind `egress` (new
--                    hosts) or `egress_mode` (broad), the diff, who asked,
--                    and the owner's decision (approve needs a fresh owner
--                    confirm, vault action `app-approve` bound to the id).
--                    Expire after 7 days. Shared with the later S20 kinds
--                    (caps, verbs, isolation, db.raw).
CREATE TABLE IF NOT EXISTS app_egress_policy (
  app_id TEXT PRIMARY KEY REFERENCES apps(app_id) ON DELETE CASCADE,
  mode TEXT NOT NULL CHECK (mode IN ('allowlist','broad')),
  updated_by TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_egress_hosts (
  app_id TEXT NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
  host TEXT NOT NULL,
  port INTEGER NOT NULL,
  source TEXT NOT NULL CHECK (source IN ('manifest','request','first_seen')),
  status TEXT NOT NULL CHECK (status IN ('allowed','pending','seen')),
  request_id TEXT,
  added_by TEXT,
  added_at INTEGER NOT NULL,
  first_seen_at INTEGER,
  last_seen_at INTEGER,
  hits INTEGER NOT NULL DEFAULT 0,
  refused INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (app_id, host, port)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_widenings (
  id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  diff_json TEXT NOT NULL,
  summary TEXT NOT NULL,
  reason TEXT,
  deployment_id TEXT,
  requested_by TEXT NOT NULL,
  requester_workspace TEXT,
  requester_session TEXT,
  requested_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending','approved','denied','withdrawn','expired')),
  decided_by TEXT,
  decided_at INTEGER,
  confirm_via TEXT,
  decision_reason TEXT
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_widenings_app_status ON app_widenings (app_id, status);
