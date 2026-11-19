-- 0319: give hiring back to the workspaces that had it (Rosson, 0.46.16).
--
-- Before 0.46.12 nothing gated an agent's own `k2 agent hire`. 0299 made
-- "Let this agent hire agents" (`projects.agents_can_hire_agents`) its own
-- switch and turned it on only where the old "hire and manage agents"
-- switch was on, so an agent that had hired without that switch lost
-- hiring (`hire_off`). This turns the switch back on for every workspace
-- that existed before 0299 ran and has it off now, EXCEPT where a person
-- set it explicitly since.
--
-- "Existed before 0299": `projects.created_at` (unixepoch seconds, the
-- column's default; no writer sets it) earlier than 0299's
-- `_migrations.applied_at` on this database. A fresh install ran 0299
-- before any workspace existed, so it has no rows here.
--
-- "Set explicitly": two trails.
--   1. A permission card for `agents.hire` or `agents.hire-off` that a
--      person answered (approved or denied): readable here, left out below.
--   2. POST /cli/agent-access/set {toggle: "agents_hire"} (Settings, the
--      CLI, and a card's Allow too): only in the Access Audit file
--      (`~/.k2/auth-audit.jsonl`, `agent-access-set` lines ending
--      `agents_hire=<value>`), which SQL can't read. So this migration only
--      records the candidates; the daemon's one-time boot step
--      (`k2_core::migrations::agents_hire_restore_0319`, keyed on this
--      table and its `code_migrations` id) reads the audit, leaves those
--      off, turns the rest on and writes each row's outcome.
CREATE TABLE IF NOT EXISTS agents_hire_restore (
  workspace_id TEXT PRIMARY KEY,
  -- NULL until the boot step decides: 'restored', 'kept_off_explicit'
  -- (the audit names a person's set) or 'already_on' (on by then).
  outcome TEXT,
  decided_at INTEGER
);
--> statement-breakpoint
INSERT OR IGNORE INTO agents_hire_restore (workspace_id)
SELECT p.id FROM projects p
WHERE p.agents_can_hire_agents = 0
  AND p.created_at < (SELECT m.applied_at FROM _migrations m WHERE m.name = '0299_agents_hire')
  AND NOT EXISTS (
    SELECT 1 FROM permission_requests r
    WHERE r.workspace_id = p.id
      AND r.action IN ('agents.hire', 'agents.hire-off')
      AND r.state IN ('approved', 'denied')
  );
