-- 0157_notify_prefs — Thread push prefs (prd-thread-push-notifications-v1 §4).
-- principal is a person key (§3): 'owner' | a connect username | '*'.
-- '*' = the workspace default an Admin sets; no FK on principal because
-- connect users live in connect-users.json (TP17). Number 0157 is a
-- placeholder: the integrator renumbers it in release order (TP18).
CREATE TABLE IF NOT EXISTS notify_workspace_prefs (
    principal   TEXT NOT NULL,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    thread_push INTEGER NOT NULL CHECK (thread_push IN (0, 1)),
    updated_at  INTEGER NOT NULL,
    updated_by  TEXT NOT NULL,
    PRIMARY KEY (principal, project_id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_notify_workspace_prefs_project ON notify_workspace_prefs(project_id);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS notify_server_prefs (
    principal  TEXT PRIMARY KEY NOT NULL,
    enabled    INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at INTEGER NOT NULL,
    updated_by TEXT NOT NULL
);
