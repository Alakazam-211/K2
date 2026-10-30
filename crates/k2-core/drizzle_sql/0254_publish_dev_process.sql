-- 0254: process state for `k2 publish run --dev`
-- (prd-publish-visibility-and-dev-mode-v1 DM-5, DM-6; Lane P2).
--   dev_mem_mb           memory cap in MiB for a dev server's systemd user
--                        scope. NULL = not a dev service.
--   dev_cpus             CPU cap in CPUs. NULL = not a dev service.
--   dev_idle_stopped_at  when the idle stop last stopped it (unix seconds).
--                        NULL = it wasn't. Set only by the idle stop: a dev
--                        site someone stopped by hand never wakes on a visit.
-- Whether a label is a dev site, and its idle limit, live in `site_visibility`
-- (0253); these columns are the process's, so they stay with the process's row.
-- ADD COLUMN only: an older daemon reads the table as before.
ALTER TABLE published_services ADD COLUMN dev_mem_mb INTEGER;
--> statement-breakpoint
ALTER TABLE published_services ADD COLUMN dev_cpus REAL;
--> statement-breakpoint
ALTER TABLE published_services ADD COLUMN dev_idle_stopped_at INTEGER;
