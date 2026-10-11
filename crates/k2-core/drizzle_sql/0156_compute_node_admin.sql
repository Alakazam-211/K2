-- 0156: K2 node admin (prd-k2-node-admin-v1 §14). Controller side.
-- Number 0156 belongs to node admin (0.45.5); the integrator renumbers
-- on slips. Lands BEFORE 0157 (notify prefs placeholder) in the list.
--
-- compute_admin_requests  proposed admin commands/plans/service specs held
--                         for the owner. `digest` = sha256 of the canonical
--                         body (what the owner approves). A request is
--                         decided once (UPDATE … WHERE state='pending'),
--                         so an approval can't be replayed.
--                         `commands_json` = the signed command envelopes
--                         (sealed at approval, sent while the node is
--                         online, one step at a time for a plan).
-- compute_node_services   declared long-running workloads (§9; S8).
-- compute_node_health     latest health facts per node (no history in v1).
-- compute_nodes.*         remote_admin / approval_key_fp as the node
--                         reports them; alert_after_secs (NULL = no offline
--                         alert); update_ring; update_json = last
--                         update_state; offline_alerted_at = when the
--                         offline alert went out (back-online clears it);
--                         policy_json/policy_at = the policy the node last
--                         reported (policy.show), for narrow-vs-widen.
--                         node_version already exists (0147).
-- projects.agents_can_operate_compute  the Agent switch "Allow this agent
--                         to operate compute nodes" (Q14), written only by
--                         /cli/agent-access/set toggle compute_operate.
CREATE TABLE IF NOT EXISTS compute_admin_requests (
  id TEXT PRIMARY KEY,
  node_id TEXT NOT NULL REFERENCES compute_nodes(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('command','plan','service')),
  class TEXT NOT NULL CHECK (class IN ('operate','privileged')),
  body_json TEXT NOT NULL,
  digest TEXT NOT NULL,
  summary TEXT NOT NULL,
  reason TEXT,
  proposed_by TEXT NOT NULL,
  proposer_workspace TEXT,
  proposer_session TEXT,
  proposed_at INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending','approved','denied','withdrawn','expired','sent','done','failed')),
  decided_by TEXT,
  decided_via TEXT,
  decided_at INTEGER,
  deny_reason TEXT,
  expires_at INTEGER NOT NULL,
  cmd_ids_json TEXT NOT NULL DEFAULT '[]',
  commands_json TEXT,
  next_step INTEGER NOT NULL DEFAULT 0,
  result_json TEXT
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_admin_requests_node ON compute_admin_requests(node_id, state);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_admin_requests_proposer ON compute_admin_requests(proposer_workspace, state);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_node_services (
  node_id TEXT NOT NULL REFERENCES compute_nodes(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  spec_json TEXT NOT NULL,
  spec_digest TEXT NOT NULL,
  label TEXT NOT NULL,
  adopted INTEGER NOT NULL DEFAULT 0,
  desired TEXT NOT NULL CHECK (desired IN ('running','stopped')),
  last_state_json TEXT,
  approved_request_id TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (node_id, name)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_node_health (
  node_id TEXT PRIMARY KEY REFERENCES compute_nodes(id) ON DELETE CASCADE,
  at INTEGER NOT NULL,
  facts_json TEXT NOT NULL
);
--> statement-breakpoint
-- Server-wide node admin settings (key/value). `alerts_to` = the workspace
-- id alerts go to (§10.2; the PRD's `compute.alertsTo`, kept in the DB so
-- it's daemon-owned and headless like the rest of compute).
CREATE TABLE IF NOT EXISTS compute_admin_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_by TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN remote_admin TEXT;
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN approval_key_fp TEXT;
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN alert_after_secs INTEGER;
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN update_ring TEXT NOT NULL DEFAULT 'normal';
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN update_json TEXT;
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN offline_alerted_at INTEGER;
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN policy_json TEXT;
--> statement-breakpoint
ALTER TABLE compute_nodes ADD COLUMN policy_at INTEGER;
--> statement-breakpoint
ALTER TABLE projects ADD COLUMN agents_can_operate_compute INTEGER NOT NULL DEFAULT 0;
