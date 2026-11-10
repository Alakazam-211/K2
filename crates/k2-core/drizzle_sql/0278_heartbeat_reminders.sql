-- 0278: one-time Reminders (0.46.11, prd-routines-reminders-and-agent-mail-v1
-- R1, vs-live W4 W6 W9). A reminder is a heartbeat that fires once and goes
-- away. One table, because everything a reminder needs already hangs off this
-- one: the lease, the four delivery paths, next_fire_at / wait_reason, OS wake
-- planning, the fires audit.
--
-- `kind` is heartbeat (every row that exists today), reminder, or mail_digest
-- (the digest row for an agent's mail notices; the other mail tables are in
-- the next migration).
-- `fire_at` UTC RFC3339, reminders only. `tz` the IANA zone it was set in,
-- NULL = this box's local time, which is today's behaviour for every existing
-- row. `body` the reminder's text (no WAKEUP.md folder for a reminder).
-- `retired_at` / `retired_reason` when and why a reminder finished.
-- `created_by` the actor who set it, as `request_actor` reports it.
-- Additive only, no data moves: every existing row reads as `heartbeat`.
ALTER TABLE workspace_heartbeats ADD COLUMN kind TEXT NOT NULL DEFAULT 'heartbeat'
    CHECK (kind IN ('heartbeat', 'reminder', 'mail_digest'));
--> statement-breakpoint
ALTER TABLE workspace_heartbeats ADD COLUMN fire_at TEXT;
--> statement-breakpoint
ALTER TABLE workspace_heartbeats ADD COLUMN tz TEXT;
--> statement-breakpoint
ALTER TABLE workspace_heartbeats ADD COLUMN body TEXT;
--> statement-breakpoint
ALTER TABLE workspace_heartbeats ADD COLUMN retired_at TEXT;
--> statement-breakpoint
ALTER TABLE workspace_heartbeats ADD COLUMN retired_reason TEXT
    CHECK (retired_reason IS NULL OR retired_reason IN ('fired', 'missed', 'failed', 'cancelled'));
--> statement-breakpoint
ALTER TABLE workspace_heartbeats ADD COLUMN created_by TEXT NOT NULL DEFAULT '';
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_workspace_heartbeats_kind
    ON workspace_heartbeats (project_id, kind, retired_at);
--> statement-breakpoint

-- W4: `frequency='scheduled'` could never fire (no cron case), and only old
-- rows and raw API writes ever had it. A row whose real frequency survives in
-- spec_json keeps it; anything else turns off with a visible reason. This
-- never migrates to `daily`, which would turn a weekly into a daily.
UPDATE workspace_heartbeats SET frequency = json_extract(spec_json, '$.frequency')
    WHERE frequency = 'scheduled' AND json_valid(spec_json)
      AND json_extract(spec_json, '$.frequency') IN ('daily', 'weekly', 'monthly', 'yearly');
--> statement-breakpoint
UPDATE workspace_heartbeats SET enabled = 0, disabled_reason = 'schedule_invalid',
       schedule_error = 'frequency "scheduled" retired in 0278; re-add this heartbeat'
    WHERE frequency = 'scheduled';
