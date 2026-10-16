-- 0178: Changes tab v1.1 review notes (C5, prd-changes-tab-v1 "C5").
-- A person leaves a note on a line (or a range) of a changed file in the
-- Changes tab; "Send to agent" batches the open unsent notes into one
-- message to the workspace's agent, which resolves them when fixed.
-- New table only. 0171-0176 are held for 0.46.1 and 0177 is the publish
-- probe grace fix; additive, so the integrator may renumber on a slip.
--
-- workspace_id    the K2 workspace (projects.id) that owns workspace_path
-- workspace_path  the Changes tab's `path` as sent (project root or a
--                 worktree checkout), the spelling events carry
-- path            repo-relative file path, `/` separators
-- side            'new' (the changed text) or 'old' (the text before)
-- start_line/end_line  1-based, inclusive, in that side's text
-- author          credential-derived actor: `owner-token`, `user:<name>`
-- author_name     what the UI and the sent message show
-- sent_at/delivery_id  set once when the note went out to the agent
-- resolved_at/resolved_by  who closed it (person or `agent:<handle>`)
-- rev             the Changes snapshot rev the note was made on
-- blob            git blob id of that side's file text when made (or NULL)
-- anchor_text     the noted lines' text, so a moved line is followed and a
--                 rewritten one shows as outdated
CREATE TABLE git_review_notes (
  id TEXT PRIMARY KEY NOT NULL,
  workspace_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  workspace_path TEXT NOT NULL,
  path TEXT NOT NULL,
  side TEXT NOT NULL DEFAULT 'new' CHECK (side IN ('old', 'new')),
  start_line INTEGER NOT NULL CHECK (start_line >= 1),
  end_line INTEGER NOT NULL CHECK (end_line >= start_line),
  body TEXT NOT NULL,
  author TEXT NOT NULL,
  author_name TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  sent_at INTEGER,
  delivery_id TEXT,
  resolved_at INTEGER,
  resolved_by TEXT,
  rev TEXT,
  blob TEXT,
  anchor_text TEXT
);
--> statement-breakpoint
CREATE INDEX idx_git_review_notes_ws ON git_review_notes (workspace_path, resolved_at);
--> statement-breakpoint
CREATE INDEX idx_git_review_notes_project ON git_review_notes (workspace_id);
