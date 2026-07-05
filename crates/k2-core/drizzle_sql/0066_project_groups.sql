-- Projects V1 P1 (prd-projects-v1 §3) — named groups of workspaces.
--
-- NOT the legacy `projects` table (that IS the workspace registry and
-- stays untouched — resolved §2b); nor the legacy `workspaces` table
-- (0000, per-project worktree rows). New tables use the
-- `project_group*` prefix. Timestamps are unix SECONDS (house
-- convention, cf. 0064).

-- The group. name is the CLI/UI address → UNIQUE COLLATE NOCASE.
-- poc_workspace_id → projects.id: exactly one PoC, required from the
-- moment the first member exists (NULL only while the group is empty).
-- PLAIN COLUMN, NO SQL-level FK (RESOLVED Q6 — same rationale as 0064:
-- workspace rows can be removed via remove_workspace_db_only). Instead,
-- EVERY workspace-removal path is route-guarded: refuse to remove a
-- workspace that is a PoC anywhere (§4.5). No auto-reassignment.
CREATE TABLE IF NOT EXISTS project_groups (
    id               TEXT PRIMARY KEY NOT NULL,          -- uuid
    name             TEXT NOT NULL UNIQUE COLLATE NOCASE,
    poc_workspace_id TEXT,                               -- projects.id; NULL only when memberless
    pinned           INTEGER NOT NULL DEFAULT 0,         -- nav Pinned section (canonical — RESOLVED
                                                         --   Q4; mirrors projects.pinned, 0004)
    sort_order       INTEGER NOT NULL DEFAULT 0,         -- nav ordering within its section
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);
--> statement-breakpoint
-- Membership. Many-to-many; UNIQUE pair. Deleting the group deletes
-- its memberships (never the workspaces themselves — locked default).
CREATE TABLE IF NOT EXISTS project_group_members (
    group_id     TEXT NOT NULL REFERENCES project_groups(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,                          -- projects.id (no FK, see above)
    created_at   INTEGER NOT NULL,
    UNIQUE (group_id, workspace_id)
);
--> statement-breakpoint
-- ONE chat stream per project. author = 'owner' | agent/workspace name
-- (same attribution convention as feedback_comments.author). NO status,
-- kind, options, or answer columns — deliberately simpler than feedback.
CREATE TABLE IF NOT EXISTS project_group_messages (
    id         TEXT PRIMARY KEY NOT NULL,                -- uuid
    group_id   TEXT NOT NULL REFERENCES project_groups(id) ON DELETE CASCADE,
    author     TEXT NOT NULL,
    body       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
--> statement-breakpoint
-- Named, SHARED (canonical) dashboards. V1 auto-creates one row named
-- 'Main' per group and the UI surfaces only it; create/rename/delete UI
-- is V1.1 — but everything is dashboard-id-addressed from day one.
-- layout_json is a versioned blob (shape in §6.3); revision is a
-- monotonic counter for last-write-wins + staleness signaling (same
-- idiom as workspace_layouts.revision, 0052).
CREATE TABLE IF NOT EXISTS project_group_dashboards (
    id          TEXT PRIMARY KEY NOT NULL,               -- uuid
    group_id    TEXT NOT NULL REFERENCES project_groups(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    layout_json TEXT NOT NULL,
    revision    INTEGER NOT NULL DEFAULT 0,
    position    INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    UNIQUE (group_id, name)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_pg_members_group ON project_group_members (group_id);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_pg_members_workspace ON project_group_members (workspace_id);
--> statement-breakpoint
-- The chat read hot path: one group's stream, chronological (rowid
-- tiebreak at read time, same-second ordering — feedback.rs idiom).
CREATE INDEX IF NOT EXISTS idx_pg_messages_group_created
    ON project_group_messages (group_id, created_at);
