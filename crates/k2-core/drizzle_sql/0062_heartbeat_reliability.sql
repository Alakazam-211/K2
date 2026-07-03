-- 0062 (heartbeat reliability overhaul): per-row failure accounting +
-- visible error states + the daemon-wide scheduler_meta KV.
--
-- `consecutive_failures` / `next_retry_at` back the exponential-backoff
-- retry policy for failed fire-attempts (spawn error, inject error, …).
-- A success resets the counter; 5 consecutive failures auto-disable the
-- row with `disabled_reason='failures'` so the UI can render a
-- "disabled after repeated failures" badge instead of a silent flag flip.
--
-- `disabled_reason` distinguishes WHY a row is disabled: NULL = user
-- choice, 'failures' = backoff exhaustion, 'wakeup_missing' = the
-- WAKEUP.md auto-disable. Manual re-enable clears it.
--
-- `schedule_error` is the visible error state for a spec_json the
-- evaluator can't parse — pre-0062 an unparseable spec meant silent
-- per-tick `not_due` audit rows while Settings showed enabled/healthy.
--
-- `scheduler_meta` is a one-row-per-key KV owned by the daemon's tick
-- path; first key is `last_tick_at` (RFC3339 of the most recent
-- /cli/scheduler-tick) so tick gaps — sleep, daemon downtime, a dead
-- launchd transport — are measurable and surfaceable in the UI.
ALTER TABLE workspace_heartbeats ADD COLUMN consecutive_failures INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workspace_heartbeats ADD COLUMN next_retry_at TEXT;
ALTER TABLE workspace_heartbeats ADD COLUMN disabled_reason TEXT;
ALTER TABLE workspace_heartbeats ADD COLUMN schedule_error TEXT;

CREATE TABLE IF NOT EXISTS scheduler_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
