-- Audit trail gets a schedule_name column so `k2so heartbeat status <name>`
-- can filter to a specific heartbeat's history. Denormalized TEXT by design
-- (NOT a FK) — audit rows must survive heartbeat deletion.
ALTER TABLE heartbeat_fires ADD COLUMN schedule_name TEXT;
CREATE INDEX idx_heartbeat_fires_schedule
  ON heartbeat_fires(project_id, schedule_name, fired_at DESC);
