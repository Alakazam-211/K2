-- 0147: K2 compute nodes (prd-k2-compute-nodes-v1 §13, §21.3).
-- New tables only, plus one projects column. Everything here is dark
-- until K2_COMPUTE / Settings "Compute nodes (preview)" is on (CN30).
--
-- compute_nodes        one row per paired machine. `state` pending → active
--                      (human SAS confirm) → revoked. Name and fingerprint
--                      are unique among rows that aren't revoked.
-- compute_node_grants  owner/admin-set permission for a workspace to use a
--                      node, with limits. `warm_at` = optional local HH:MM
--                      for the controller's own nightly warm job (CN8).
-- compute_jobs         one row per job; `generation` bumps on retry (the
--                      FICC fence). `request_sha256` backs the
--                      idempotency rule (same key + other body = 409).
-- compute_usage        bytes per node per day per route (relay|lan|tailnet|
--                      loopback), metered only by the controller (CN17).
-- compute_events       the audit log (CN21), shape of remote_session_events.
-- projects.agents_can_use_compute  the Agent-tab "Allow compute nodes"
--                      switch, written only by /cli/agent-access/set.
CREATE TABLE IF NOT EXISTS compute_nodes (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  public_key_pem TEXT NOT NULL,
  os TEXT,
  arch TEXT,
  labels_json TEXT NOT NULL DEFAULT '{}',
  state TEXT NOT NULL CHECK (state IN ('pending','active','revoked')),
  enrolled_via TEXT NOT NULL CHECK (enrolled_via IN ('code','ticket')),
  enrolled_by TEXT,
  enrolled_at INTEGER NOT NULL,
  sas TEXT,
  confirmed_by TEXT,
  confirmed_at INTEGER,
  revoked_by TEXT,
  revoked_at INTEGER,
  controller_pause TEXT CHECK (controller_pause IS NULL OR controller_pause IN ('paused','draining')),
  controller_pause_by TEXT,
  controller_pause_at INTEGER,
  last_seen INTEGER,
  last_route TEXT,
  protocol INTEGER,
  node_version TEXT,
  routes_json TEXT NOT NULL DEFAULT '[]',
  last_offer_json TEXT
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS compute_nodes_live_name ON compute_nodes(name) WHERE state != 'revoked';
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS compute_nodes_live_fp ON compute_nodes(fingerprint) WHERE state != 'revoked';
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_node_grants (
  node_id TEXT NOT NULL REFERENCES compute_nodes(id) ON DELETE CASCADE,
  workspace_id TEXT NOT NULL,
  scopes TEXT NOT NULL DEFAULT 'run,sync,artifacts',
  max_job_secs INTEGER NOT NULL DEFAULT 7200,
  max_disk_gb INTEGER NOT NULL DEFAULT 60,
  max_parallel INTEGER NOT NULL DEFAULT 1,
  max_queued INTEGER NOT NULL DEFAULT 10,
  warm_at TEXT,
  expires_at INTEGER,
  granted_by TEXT NOT NULL,
  granted_at INTEGER NOT NULL,
  PRIMARY KEY (node_id, workspace_id)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_jobs (
  id TEXT PRIMARY KEY,
  client_job_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  workspace_id TEXT NOT NULL,
  session_id TEXT,
  requested_by TEXT NOT NULL,
  argv_json TEXT NOT NULL,
  env_names_json TEXT NOT NULL,
  request_sha256 TEXT NOT NULL,
  plan_json TEXT NOT NULL,
  src_commit TEXT,
  src_tree TEXT,
  dirty_sha256 TEXT,
  state TEXT NOT NULL,
  reason TEXT,
  detail TEXT,
  attempt INTEGER NOT NULL DEFAULT 1,
  generation INTEGER NOT NULL DEFAULT 1,
  plan_digest TEXT NOT NULL,
  exclusive INTEGER NOT NULL DEFAULT 0,
  detach INTEGER NOT NULL DEFAULT 0,
  retry_interrupted INTEGER NOT NULL DEFAULT 0,
  exit_code INTEGER,
  signal INTEGER,
  created_at INTEGER NOT NULL,
  assigned_at INTEGER,
  started_at INTEGER,
  ended_at INTEGER,
  log_seq INTEGER NOT NULL DEFAULT 0,
  log_bytes INTEGER NOT NULL DEFAULT 0,
  summary_json TEXT,
  artifacts_json TEXT,
  receipt_json TEXT,
  receipt_ok INTEGER,
  notified_at INTEGER,
  UNIQUE (workspace_id, client_job_id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_jobs_node_state ON compute_jobs(node_id, state);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_jobs_workspace_created ON compute_jobs(workspace_id, created_at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_usage (
  node_id TEXT NOT NULL,
  day TEXT NOT NULL,
  route TEXT NOT NULL,
  bytes_in INTEGER NOT NULL DEFAULT 0,
  bytes_out INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (node_id, day, route)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  at INTEGER NOT NULL,
  actor TEXT NOT NULL,
  kind TEXT NOT NULL,
  node_id TEXT,
  workspace_id TEXT,
  job_id TEXT,
  detail_json TEXT NOT NULL DEFAULT '{}'
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_events_at ON compute_events(at);
--> statement-breakpoint
ALTER TABLE projects ADD COLUMN agents_can_use_compute INTEGER NOT NULL DEFAULT 0;
