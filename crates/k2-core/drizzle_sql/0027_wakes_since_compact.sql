-- Track how many times we've woken this agent since we last told Claude to
-- /compact. Drives the "compact every N wakes" rule in k2so_agents_build_launch
-- so long-running heartbeat sessions don't accumulate unbounded history.
ALTER TABLE agent_sessions ADD COLUMN wakes_since_compact INTEGER NOT NULL DEFAULT 0;
