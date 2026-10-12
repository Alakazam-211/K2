-- Workspace handles are never reused (0.45.x, Rosson).
--
-- A workspace handle is its address (`k2 msg <handle>`, `<handle>::host`
-- on federated peers). Display names stay free to change and reuse; the
-- handle does not. Renames already keep the old handle as an alias while
-- the workspace exists (0103 `project_handle_aliases`). Deleting the
-- workspace used to cascade those rows away, which freed the names for a
-- new workspace to take.
--
-- retired_workspace_handles keeps them for good:
--   handle              = the retired name (unique, case-insensitive).
--   former_workspace_id = the projects.id that had it.
--   retired_at          = unix seconds.
--   reason              = 'delete' for the live handle at delete time,
--                         'rename' for its earlier names (its aliases).
--
-- The BEFORE DELETE trigger covers every delete path (agent retire,
-- workspace remove, any future one) inside the deleting statement's
-- transaction. DROP TABLE does not fire triggers, and the triggers are
-- dropped with the table: a future migration that rebuilds `projects`
-- must create both triggers again.
--
-- The AFTER INSERT trigger handles a delete + re-insert of the SAME id
-- (same workspace, not a different one): its names come back as aliases.
--
-- Nothing is backfilled: workspaces deleted before this migration left no
-- rows behind, so their names can't be recovered.
--
-- There is no release command (Rosson: "for good").
CREATE TABLE IF NOT EXISTS retired_workspace_handles (
    handle TEXT NOT NULL,
    former_workspace_id TEXT NOT NULL,
    retired_at INTEGER NOT NULL DEFAULT (unixepoch()),
    reason TEXT NOT NULL CHECK (reason IN ('rename', 'delete')),
    UNIQUE (handle COLLATE NOCASE)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS retired_workspace_handles_ws
    ON retired_workspace_handles(former_workspace_id);
--> statement-breakpoint
CREATE TRIGGER IF NOT EXISTS projects_retire_handles_on_delete
BEFORE DELETE ON projects
BEGIN
    INSERT OR IGNORE INTO retired_workspace_handles (handle, former_workspace_id, reason)
        SELECT TRIM(OLD.handle), OLD.id, 'delete'
        WHERE OLD.handle IS NOT NULL AND TRIM(OLD.handle) != '';
    INSERT OR IGNORE INTO retired_workspace_handles (handle, former_workspace_id, reason)
        SELECT TRIM(alias), OLD.id, 'rename'
        FROM project_handle_aliases
        WHERE project_id = OLD.id AND TRIM(alias) != '';
    DELETE FROM project_handle_aliases WHERE project_id = OLD.id;
END;
--> statement-breakpoint
CREATE TRIGGER IF NOT EXISTS projects_restore_handles_on_reinsert
AFTER INSERT ON projects
BEGIN
    INSERT OR IGNORE INTO project_handle_aliases (project_id, alias)
        SELECT NEW.id, handle
        FROM retired_workspace_handles
        WHERE former_workspace_id = NEW.id
          AND (NEW.handle IS NULL OR handle != NEW.handle COLLATE NOCASE);
    DELETE FROM retired_workspace_handles WHERE former_workspace_id = NEW.id;
END;
--> statement-breakpoint
-- Federation: a saved connection bound to a peer workspace id (0141) never
-- re-binds to a different workspace by name. When the bound id leaves the
-- peer's roster, the row keeps its id and is marked gone (unix seconds);
-- sends through it fail with `remote_workspace_gone` until a person
-- re-adds the connection. NULL = not gone. If the same id shows up in the
-- roster again, the mark is cleared.
ALTER TABLE workspace_remote_connections ADD COLUMN remote_workspace_gone_at INTEGER;
