-- 0105: daemon-owned workspace resources (prd-workspace-resources-v1).
-- One row per (workspace, file). `workspace_id` is `projects.id` (the product
-- workspace), NOT the git-worktree `workspaces` table. `file_path` is the
-- absolute daemon-host path (same as layouts / fs/read-file).
-- No FK: a dangling workspace_id is treated as a missing workspace at read
-- time (same posture as 0104 published_services / 0074 subdomain_workspaces).
CREATE TABLE IF NOT EXISTS workspace_resources (
    workspace_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, file_path)
);
