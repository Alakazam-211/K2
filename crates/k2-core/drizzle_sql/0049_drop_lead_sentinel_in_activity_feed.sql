-- 0049: rewrite `activity_feed.to_workspace = '__lead__'` rows and
-- drop the `OR (to_workspace IS NULL AND ?1 = '__lead__')` clause
-- in `get_unread_messages` / `mark_messages_read`.
--
-- Background. Pre-0.37.0 K2SO addressed the workspace manager as
-- `__lead__` everywhere — the activity_feed's `to_workspace` column
-- (renamed from `to_agent` in 0041) carried that sentinel for any
-- message destined for the workspace's manager-mode primary. Post-
-- unification the workspace has at most one primary agent and the
-- routing key in every other place became "workspace identity"; the
-- `'__lead__'` literal in activity_feed.to_workspace is the last
-- pre-unification routing string still in flight.
--
-- 0.39.0f cleanup. Rewrite any row whose `to_workspace = '__lead__'`
-- to point at the resolved workspace's primary agent name. We can
-- compute that two ways:
--
--   (1) projects.agent_mode = 'manager' — the workspace has a
--       primary agent on disk (or had one before deletion). Walk
--       `projects` and for each manager-mode workspace rewrite the
--       row to the resolved primary name. There's no canonical SQL
--       lookup for "primary agent name" (it lives in the workspace's
--       AGENT.md frontmatter, not in the DB), so we fall back to
--       `projects.name` which is the workspace folder basename and
--       matches the agent name in the overwhelming majority of
--       workspaces (the agent IS the workspace).
--   (2) projects.agent_mode != 'manager' OR project missing —
--       no primary agent to route to. These rows are orphans from
--       the pre-unification era. Set `to_workspace` to NULL so the
--       `get_unread_messages` join (which used to fall back to NULL
--       = `__lead__`) treats them as broadcast / unaddressed.
--       Operational tolerance: the message text is preserved, just
--       no longer addressed.
--
-- Idempotent. After this migration runs every row will satisfy
-- `to_workspace != '__lead__'`, so a re-run is a no-op.

-- Pass 1: manager-mode workspaces get rewritten to the workspace's name.
UPDATE activity_feed
SET to_workspace = (
    SELECT p.name FROM projects p WHERE p.id = activity_feed.project_id
)
WHERE to_workspace = '__lead__'
  AND project_id IN (
    SELECT id FROM projects WHERE agent_mode = 'manager'
  );

-- Pass 2: non-manager workspaces (or orphan rows whose project_id
-- doesn't resolve) get nulled out — they were addressed to a routing
-- key that no longer exists.
UPDATE activity_feed
SET to_workspace = NULL
WHERE to_workspace = '__lead__';
