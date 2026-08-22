-- 0104: daemon-owned published services (prd-k2-publish-hosted-services-v1).
-- One row per (workspace, name). `project_id` is `projects.id` (the product
-- workspace), NOT the git-worktree `workspaces` table. Pid is runtime only —
-- `desired` is the SSOT of "should be up". `expose` is internal
-- (`tunnel` = default nested run; `local` = --no-tunnel). CLI has no --expose.
-- No FK: a dangling project_id is treated as a missing workspace at read time
-- (same posture as 0074 subdomain_workspaces).
CREATE TABLE IF NOT EXISTS published_services (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    name TEXT NOT NULL,
    cmd TEXT NOT NULL,
    cwd TEXT NOT NULL,
    port INTEGER NOT NULL,
    expose TEXT NOT NULL CHECK (expose IN ('local', 'tunnel')),
    desired TEXT NOT NULL CHECK (desired IN ('running', 'stopped')),
    pid INTEGER,
    last_exit_code INTEGER,
    last_started_at INTEGER,
    last_exited_at INTEGER,
    error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (project_id, name)
);
