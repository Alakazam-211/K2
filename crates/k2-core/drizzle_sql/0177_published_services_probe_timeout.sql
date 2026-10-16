-- 0177: per-service startup grace for `k2 publish run --probe-timeout`
-- (plan-0461-lanes-v1 "Found 2026-10-16: publish run startup probe").
--   probe_timeout_ms  NULL = the daemon default (15 s, or K2_PUBLISH_PROBE_MS);
--                     else how long the service may take to open its port
--                     (capped at 15 min by the daemon). Restarts and boot
--                     respawns use it too.
--   hostname_pending  1 while a tunnelled service waits inside its grace for
--                     the first port answer: its nested hostname is not
--                     published yet. Boot reattach finishes the attach.
-- 0171–0175 are the 0.46.1 runtime lanes' and 0176 is the integrator's spare
-- (plan-0461-lanes-v1); the integrator renumbers on slips.
ALTER TABLE published_services ADD COLUMN probe_timeout_ms INTEGER;
--> statement-breakpoint
ALTER TABLE published_services ADD COLUMN hostname_pending INTEGER NOT NULL DEFAULT 0;
