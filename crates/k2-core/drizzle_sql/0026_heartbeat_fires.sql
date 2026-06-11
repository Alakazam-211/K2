-- Audit log for heartbeat scheduler decisions. One row per agent per tick
-- (whether the agent was launched or skipped), so users can see exactly
-- why the scheduler did or didn't wake an agent.
--
-- Retention is whatever the caller cares to keep — no automatic pruning.
-- For typical workloads (a few projects × dozen agents × 60s ticks) this
-- table grows ~1MB per day per active workspace; expected to be pruned
-- manually or via a future retention setting.

CREATE TABLE IF NOT EXISTS heartbeat_fires (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id      TEXT NOT NULL,
  agent_name      TEXT,                  -- null for project-level decisions (e.g. schedule gate)
  fired_at        TEXT NOT NULL,         -- RFC3339
  mode            TEXT NOT NULL,         -- heartbeat | hourly | scheduled | off | locked
  decision        TEXT NOT NULL,         -- fired | skipped_schedule | skipped_locked | skipped_in_flight | skipped_user_session | skipped_quality_gate | skipped_custom_timing | no_work | error
  reason          TEXT,                  -- human-readable explanation
  inbox_priority  TEXT,                  -- highest priority in agent's inbox (critical|high|normal|low) or null
  inbox_count     INTEGER,               -- number of items in agent's inbox at decision time
  duration_ms     INTEGER,               -- time spent deciding (for debugging slow ticks)
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_heartbeat_fires_project_time
  ON heartbeat_fires(project_id, fired_at DESC);

CREATE INDEX IF NOT EXISTS idx_heartbeat_fires_agent_time
  ON heartbeat_fires(project_id, agent_name, fired_at DESC);
