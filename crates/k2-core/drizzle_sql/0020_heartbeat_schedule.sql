-- Two-mode heartbeat scheduling: "scheduled" (cron-like) and "hourly" (work hours + frequency)
ALTER TABLE projects ADD COLUMN heartbeat_mode TEXT NOT NULL DEFAULT 'off';
ALTER TABLE projects ADD COLUMN heartbeat_schedule TEXT;
ALTER TABLE projects ADD COLUMN heartbeat_last_fire TEXT;

-- Backfill: existing heartbeat-enabled projects become hourly/5min/24hr
UPDATE projects SET heartbeat_mode = 'hourly',
  heartbeat_schedule = '{"start":"00:00","end":"23:59","every_seconds":300}'
  WHERE heartbeat_enabled = 1;
