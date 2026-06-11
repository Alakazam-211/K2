-- Per-heartbeat concurrency, deadlines, and crash-recovery lease.
--
-- Vocabulary mirrors Kubernetes CronJob (concurrency_policy,
-- starting_deadline, active_deadline) — battle-tested names beat
-- inventing our own. River/Oban's lease pattern handles crash
-- recovery via a single timestamp column swept on boot.
--
-- Columns:
--
--   agent_heartbeats.concurrency_policy
--     'forbid' (default) — skip new fire if previous still in flight.
--     'allow'            — every fire spawns regardless of in-flight.
--     'replace'          — kill prior in-flight spawn, start new one.
--     Default 'forbid' preserves today's implicit behavior (one spawn
--     per heartbeat at a time).
--
--   agent_heartbeats.starting_deadline_secs
--     Skip-if-late window. If now() - scheduled_fire_at > this, the
--     tick is logged as `skipped_deadline` and not spawned. 600s (10
--     minutes) tolerates one missed launchd tick at the post-P5.7
--     60-second cadence; defends against thundering-herd-on-recovery
--     after the daemon was asleep all day.
--
--   agent_heartbeats.active_deadline_secs
--     Per-spawn timeout. The async wrapper around smart_launch wraps
--     each call in tokio::time::timeout(active_deadline). Default 30s
--     because today's spawn (PTY allocate + Claude Code boot) measures
--     1-3s; 30s catches truly hung forks while leaving headroom.
--     Long-running sessions are unaffected — the deadline only covers
--     spawn, not the resulting process.
--
--   agent_heartbeats.in_flight_started_at
--     RFC3339 timestamp written by try_acquire_heartbeat on entry,
--     cleared by stamp_heartbeat_fired on success/failure. NULL = idle.
--     Boot-time sweep clears stale leases (older than 5 minutes) so a
--     crashed-mid-spawn row doesn't stay locked forever. This is the
--     River/Oban pattern.

ALTER TABLE agent_heartbeats ADD COLUMN concurrency_policy TEXT NOT NULL DEFAULT 'forbid';
ALTER TABLE agent_heartbeats ADD COLUMN starting_deadline_secs INTEGER NOT NULL DEFAULT 600;
ALTER TABLE agent_heartbeats ADD COLUMN active_deadline_secs INTEGER NOT NULL DEFAULT 30;
ALTER TABLE agent_heartbeats ADD COLUMN in_flight_started_at TEXT;
