-- 0205: compute jobs carry their tenant (prd-compute-shared-queue-v1 S1,
-- V30; 0.46.3). ALTER only: every existing row is tenant `local`.
--
-- tenant_id        'local' or a compute_tenants id. A server tenant's job
--                  has workspace_id = 't/<tenant>/<remote workspace id>' (a
--                  hub-side key, so the per-workspace queue, grants and
--                  counts never mix tenants) and keeps the submitter's ids
--                  in remote_ws_id / remote_ws_label / remote_session.
-- class            gate | interactive | batch (S3 orders by it).
-- preemptible      S4.
-- est_secs         the dispatcher's estimate (S3).
-- The old UNIQUE (workspace_id, client_job_id) stays (tenant keys are
-- namespaced, so it never collides across tenants); the tenant-scoped
-- unique below is the rule new code reads.
ALTER TABLE compute_jobs ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'local';
--> statement-breakpoint
ALTER TABLE compute_jobs ADD COLUMN class TEXT NOT NULL DEFAULT 'interactive';
--> statement-breakpoint
ALTER TABLE compute_jobs ADD COLUMN preemptible INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE compute_jobs ADD COLUMN est_secs INTEGER;
--> statement-breakpoint
ALTER TABLE compute_jobs ADD COLUMN remote_ws_id TEXT;
--> statement-breakpoint
ALTER TABLE compute_jobs ADD COLUMN remote_ws_label TEXT;
--> statement-breakpoint
ALTER TABLE compute_jobs ADD COLUMN remote_session TEXT;
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS compute_jobs_tenant_client ON compute_jobs(tenant_id, workspace_id, client_job_id);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_jobs_tenant_state ON compute_jobs(tenant_id, state);
