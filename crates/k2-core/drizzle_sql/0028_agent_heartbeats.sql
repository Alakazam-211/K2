-- Multi-heartbeat architecture: one workspace can now have N named
-- heartbeats, each with its own schedule + wakeup.md. Replaces the
-- single-slot projects.heartbeat_schedule column (which remains
-- during transition, deprecated). See .k2so/prds/multi-schedule-heartbeat.md.
CREATE TABLE agent_heartbeats (
  id            TEXT PRIMARY KEY,                                          -- uuid
  project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name          TEXT NOT NULL,                                             -- lowercase + hyphens + digits, unique per project
  frequency     TEXT NOT NULL,                                             -- daily | weekly | monthly | yearly | hourly
  spec_json     TEXT NOT NULL,                                             -- {time, days, months, ...}
  wakeup_path   TEXT NOT NULL,                                             -- workspace-relative path to wakeup.md
  enabled       INTEGER NOT NULL DEFAULT 1,
  last_fired    TEXT,                                                      -- RFC3339 — stamped only on successful spawn
  created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE (project_id, name)
);
CREATE INDEX idx_agent_heartbeats_project_enabled
  ON agent_heartbeats(project_id, enabled);
