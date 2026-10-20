-- 0206: this server submits to a compute hub (prd-compute-shared-queue-v1
-- S2, §6.5; 0.46.3). New tables only; with no active row here the server
-- runs its own controller exactly as before.
--
-- compute_hubs         "Use compute hub <host>": the hub's federation
--                      fingerprint (a Trusted, pairing-v2 peer), its label and
--                      dial URL. At most one row is `active`.
-- compute_remote_jobs  jobs this server sent to a hub: the hub's job id, the
--                      LOCAL workspace and session that asked (the hub only
--                      sees them as remote ids), what the poller last saw, and
--                      `notified_at` so each completion is delivered once,
--                      locally (V24).
CREATE TABLE IF NOT EXISTS compute_hubs (
  hub_fp TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  base_url TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL CHECK (state IN ('active','off')),
  since INTEGER NOT NULL,
  set_by TEXT NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS compute_hubs_one_active ON compute_hubs(state) WHERE state = 'active';
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_remote_jobs (
  hub_job_id TEXT PRIMARY KEY,
  hub_fp TEXT NOT NULL,
  workspace_id TEXT NOT NULL,
  session_id TEXT,
  client_job_id TEXT NOT NULL,
  detach INTEGER NOT NULL DEFAULT 0,
  state TEXT NOT NULL,
  node TEXT,
  summary_json TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  ended_at INTEGER,
  notified_at INTEGER
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_remote_jobs_ws ON compute_remote_jobs(workspace_id, created_at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_remote_jobs_open ON compute_remote_jobs(state);
