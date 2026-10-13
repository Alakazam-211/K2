-- Agent hire through the v1 API (0.45.6, prd-agent-hire-templates-v1
-- "build A only": one inline-recipe hire call; no saved templates).
--
-- api_keys.cap_hire      = the hire door. DEFAULT 0 (fail closed, the
--                          cap_db pattern): existing keys never grow it.
-- api_keys.hire_limits   = JSON envelope a hire must fit in (quotas, DB
--                          ceiling, link/retire/reach switches, sub-folder).
--                          NULL or unparsable = no hire (fail closed).
-- api_keys.legacy_new_workspace = T19: new-workspace creation on POST /v1/w
--                          moved under the hire scope. Keys that could do it
--                          before (a `*` grant with host-sessions) keep it
--                          through this one-time flag; new mints never get it.
ALTER TABLE api_keys ADD COLUMN cap_hire INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE api_keys ADD COLUMN hire_limits TEXT;
--> statement-breakpoint
ALTER TABLE api_keys ADD COLUMN legacy_new_workspace INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
UPDATE api_keys SET legacy_new_workspace = 1
    WHERE revoked_at IS NULL
      AND cap_host_sessions = 1
      AND allowed_workspaces IS NOT NULL
      AND (TRIM(allowed_workspaces) = '*' OR allowed_workspaces LIKE '%"*"%');
--> statement-breakpoint
-- The owner-set folder every API-created workspace lives under (T2: one
-- root per box). One row; no row = the default (~/k2-agents).
CREATE TABLE IF NOT EXISTS hire_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    root TEXT,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
--> statement-breakpoint
-- One hire call = one instance (one agent or a team). api_key_id NULL =
-- the owner token or a Connect login. idem_key is the caller's
-- Idempotency-Key, scoped per API key; a failed hire clears it so the same
-- key can try again.
CREATE TABLE IF NOT EXISTS agent_instances (
    id TEXT PRIMARY KEY,
    api_key_id TEXT,
    key_label TEXT,
    idem_key TEXT,
    request_hash TEXT NOT NULL,
    actor TEXT NOT NULL,
    on_behalf_of TEXT,
    group_id TEXT,
    team_name TEXT,
    planned_members INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL CHECK (status IN ('applying', 'active', 'failed', 'retired', 'retire_blocked')),
    reason TEXT,
    result_json TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS agent_instances_idem
    ON agent_instances (COALESCE(api_key_id, ''), idem_key)
    WHERE idem_key IS NOT NULL;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS agent_instances_key ON agent_instances (api_key_id, created_at);
--> statement-breakpoint
-- Members keep their row after retire (no FK to projects): the ledger is
-- the audit trail and the quota counter.
CREATE TABLE IF NOT EXISTS agent_instance_members (
    instance_id TEXT NOT NULL REFERENCES agent_instances (id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    role TEXT NOT NULL,
    handle TEXT NOT NULL,
    path TEXT NOT NULL,
    lead INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'live' CHECK (status IN ('live', 'retired', 'rolled_back')),
    -- S5: {"app": <service id>, "username": <login>} when the member got an
    -- App login (an App grant, never the box login); teardown revokes it.
    app_login TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (instance_id, workspace_id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS agent_instance_members_ws ON agent_instance_members (workspace_id);
--> statement-breakpoint
-- Every hire, retire, link, unlink and hire-key mint (HT19). on_behalf_of
-- is app-asserted and never used for a decision.
CREATE TABLE IF NOT EXISTS hire_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at INTEGER NOT NULL DEFAULT (unixepoch()),
    action TEXT NOT NULL,
    actor TEXT NOT NULL,
    api_key_id TEXT,
    key_label TEXT,
    instance_id TEXT,
    link_id TEXT,
    on_behalf_of TEXT,
    result TEXT NOT NULL,
    reason TEXT,
    detail TEXT
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS hire_events_instance ON hire_events (instance_id);
