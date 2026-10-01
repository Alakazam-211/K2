-- Heartbeat S3 (prd-heartbeat-firing-v1.md HB18): the daemon owns
-- next-fire. Columns only, no SQL backfill — the daemon's first wait
-- pass after boot fills every non-archived row.
--
-- `next_fire_at`    UTC RFC3339 of the next fire the daemon expects
--                   (a past value means due / overdue). NULL = none
--                   (disabled, invalid schedule).
-- `wait_reason`     why the row is waiting — the fixed HB20 vocabulary
--                   (`k2_core::heartbeats::wait::ALL_REASONS`).
-- `wait_detail`     human detail: error text, window time, "failure 2 of 5".
-- `wait_since`      UTC RFC3339 when `wait_reason` last changed.
-- `overdue_noted_at` the open overdue episode (HB22): set when the one
--                   `overdue` audit row is written, cleared when the row
--                   is no longer overdue (a fire, an edit, a re-enable).
--
-- Numbered 0124: 0123 is heartbeat S2's `schedule_anchor_at`.
ALTER TABLE workspace_heartbeats ADD COLUMN next_fire_at TEXT;
ALTER TABLE workspace_heartbeats ADD COLUMN wait_reason TEXT;
ALTER TABLE workspace_heartbeats ADD COLUMN wait_detail TEXT;
ALTER TABLE workspace_heartbeats ADD COLUMN wait_since TEXT;
ALTER TABLE workspace_heartbeats ADD COLUMN overdue_noted_at TEXT;
