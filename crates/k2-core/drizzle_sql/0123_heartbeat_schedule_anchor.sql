-- Heartbeat S2 (prd-heartbeat-firing-v1.md, D4 + D7): the point a
-- heartbeat's schedule counts from when it has not fired since.
-- Set to "now" when a heartbeat is enabled, its schedule is edited, it
-- is unarchived, or a missed slot older than 12 h is skipped. The due
-- check counts from the later of last_fired and this value, so none of
-- those actions fires a surprise catch-up. NULL = count from last_fired,
-- else created_at (the pre-S2 rule).
ALTER TABLE workspace_heartbeats ADD COLUMN schedule_anchor_at TEXT;
