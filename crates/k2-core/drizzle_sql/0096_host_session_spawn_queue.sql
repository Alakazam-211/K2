-- Durable host-session spawn queue (prd-host-session-spawn-queue-v1).
-- Path-keyed FIFO of cold / dead-resume jobs deferred when at cell cap.
-- Feature default OFF (K2_HOST_SESSION_SPAWN_QUEUE). Prompt is purged on
-- terminal states (completed/failed/expired/cancelled). Capability JWTs are
-- NEVER stored — only the raw request specs JSON (capabilities_json).

CREATE TABLE IF NOT EXISTS host_session_spawn_queue (
  job_id TEXT PRIMARY KEY NOT NULL,
  workspace_path TEXT NOT NULL,
  workspace_slug TEXT NOT NULL,
  principal_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('cold', 'dead_resume')),
  session_id TEXT,
  prompt TEXT,
  timeout_secs INTEGER,
  capabilities_json TEXT,
  cols INTEGER,
  rows INTEGER,
  client_request_id TEXT,
  status TEXT NOT NULL CHECK(status IN (
    'queued', 'running', 'completed', 'failed', 'expired', 'cancelled'
  )),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  started_at INTEGER,
  finished_at INTEGER,
  result_session_id TEXT,
  fail_code TEXT,
  fail_message TEXT
);

-- FIFO head: oldest queued job per workspace.
CREATE INDEX IF NOT EXISTS idx_hssq_ws_status_created
  ON host_session_spawn_queue (workspace_path, status, created_at, job_id);

-- Idempotency: same clientRequestId under one principal + workspace → same job
-- while still open (queued/running). Enforced in application code for partial
-- uniqueness of non-NULL client_request_id among open rows.

CREATE INDEX IF NOT EXISTS idx_hssq_client_req
  ON host_session_spawn_queue (workspace_path, principal_id, client_request_id)
  WHERE client_request_id IS NOT NULL;

-- Dead-resume: at most one open job per (workspace, session_id).
CREATE INDEX IF NOT EXISTS idx_hssq_dead_resume
  ON host_session_spawn_queue (workspace_path, session_id, status)
  WHERE kind = 'dead_resume' AND session_id IS NOT NULL;
