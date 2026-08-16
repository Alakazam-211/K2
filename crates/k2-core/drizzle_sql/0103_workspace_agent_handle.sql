-- Workspace Agent Name vs Handle (prd-workspace-display-name-and-handle-v1).
-- projects.name stays the pretty display. projects.handle is the address
-- token (k2 msg, roster, name::host). Aliases persist pre-slug spellings
-- and previous handles so slug-equal federated lookups keep working.
--
-- handle is nullable at the column layer so existing test INSERTs and
-- mid-boot rows do not fail; writers + the 0103 Rust backfill mint a
-- value. Unique NOCASE: two NULLs are allowed (SQLite), two live
-- handles are not.

ALTER TABLE projects ADD COLUMN handle TEXT;

CREATE UNIQUE INDEX projects_handle_nocase
    ON projects (handle COLLATE NOCASE);

CREATE TABLE project_handle_aliases (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    alias TEXT NOT NULL,
    UNIQUE (alias COLLATE NOCASE)
);

CREATE INDEX project_handle_aliases_project
    ON project_handle_aliases(project_id);
