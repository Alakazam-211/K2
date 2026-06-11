-- 0036: heartbeat active-session tracking + per-session surfaced flag.
--
-- Replaces the args-matching approach in find_live_for_resume with an
-- explicit FK-style pointer from the heartbeat row to its current PTY's
-- terminal_id. Adds a per-session "surfaced" flag so the renderer can
-- summon a heartbeat session into a tab on demand instead of relying on
-- the workspace-wide show_heartbeat_sessions gate.
--
-- See `.k2so/prds/heartbeat-active-session-tracking.md` for the design.

ALTER TABLE agent_heartbeats ADD COLUMN active_terminal_id TEXT;
--> statement-breakpoint
ALTER TABLE agent_sessions ADD COLUMN surfaced INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
-- Existing user-initiated sessions (chat tab, persona editor, etc.) should
-- show as already surfaced — backfill anything user-owned to surfaced=1
-- so the post-migration UI doesn't lose visibility on existing tabs.
UPDATE agent_sessions SET surfaced = 1 WHERE owner = 'user';
