-- Project-namespace agent-chat terminal IDs.
--
-- Pre-0.36.0 terminal IDs for the agent Chat tab were keyed only by
-- agent name (`agent-chat-<agent>`), which collided across workspaces
-- that shared the same agent (e.g. six projects all using `manager`).
-- Effect: `terminal_manager.exists(id)` returned the SAME PTY for all
-- six, so whichever project spawned first won and the other five
-- attached to its session.
--
-- Post-0.36.0 the format becomes `agent-chat:<project_id>:<agent>` so
-- each (project, agent) pair gets its own PTY. Worktree-scoped form
-- gets a parallel rename (`agent-chat-wt-<wsid>` →
-- `agent-chat:wt:<wsid>`) so the separator is consistent.
--
-- Format reference: `crates/k2so-core/src/agents/terminal_id.rs` and
-- `src/renderer/lib/terminal-id.ts` (must agree).
--
-- session_id rows are preserved as-is. Each agent_sessions row already
-- has its own session_id (UNIQUE(project_id, agent_name)); orphaned
-- ids spawn fresh on next launch when --resume errors.

-- Step 1: Worktree form rename. Run before step 2 so the second pattern
-- doesn't accidentally re-rewrite the worktree rows.
UPDATE agent_sessions
SET terminal_id = replace(terminal_id, 'agent-chat-wt-', 'agent-chat:wt:')
WHERE terminal_id LIKE 'agent-chat-wt-%';

-- Step 2: Project-namespace the unscoped form. Excludes anything already
-- in the new format (`agent-chat:%`) so re-running the migration is a
-- no-op and worktree rows from step 1 are not touched.
UPDATE agent_sessions
SET terminal_id = printf('agent-chat:%s:%s', project_id, agent_name)
WHERE terminal_id IS NOT NULL
  AND terminal_id LIKE 'agent-chat-%'
  AND terminal_id NOT LIKE 'agent-chat:%';
