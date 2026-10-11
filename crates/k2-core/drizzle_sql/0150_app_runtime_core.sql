-- 0150: App runtime core (prd-k2-app-runtime-v1 §9.1, stage 1, 0.46.0).
-- New tables only. Everything here is dark until K2_APPS / Settings
-- "Apps (preview)" (`appsPreview`) is on.
--
-- seeds         one row per Seed: an immutable, content-addressed tree in
--               ~/.k2/seeds/<hash>/ (code + manifest + validate report +
--               signer + createdWith). Never users, grants or secrets.
-- apps          one row per App (runtime + deploy truth). Keyed to its
--               published_services row by `service_id` (AR5): that row
--               stays the routing / URL / frame-policy / skin.db truth.
--               `memory_limit_bytes` NULL = no per-App limit (Rosson
--               2026-10-10: the user decides App memory; stage R owns the
--               protected reserve). `front_port` = the daemon-owned stable
--               loopback port the control-plane target points at (AR4).
-- deployments   every event that moves (or tries to move) the live
--               pointer: who, from, to, why, result, commit (L3).
-- deploy_audit  hash-chained audit shared by Apps and Garden widgets
--               (`subject_kind`). hash = sha256(prev_hash || canonical row).
CREATE TABLE IF NOT EXISTS seeds (
  hash TEXT PRIMARY KEY,
  seed_id TEXT NOT NULL,
  version TEXT,
  host_mode TEXT NOT NULL CHECK (host_mode IN ('static','cmd','next')),
  manifest_json TEXT NOT NULL,
  created_with TEXT NOT NULL,
  report_json TEXT NOT NULL,
  signer TEXT NOT NULL,
  origin TEXT NOT NULL DEFAULT 'local',
  built_for TEXT,
  built_from TEXT,
  source_ref TEXT,
  data_revision INTEGER NOT NULL DEFAULT 0,
  size_bytes INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS apps (
  app_id TEXT PRIMARY KEY,
  service_id TEXT NOT NULL UNIQUE,
  seed_id TEXT,
  owner_workspace TEXT NOT NULL,
  path TEXT NOT NULL DEFAULT 'guarded' CHECK (path IN ('guarded','open')),
  source TEXT NOT NULL DEFAULT 'seed' CHECK (source IN ('seed','folder')),
  folder_path TEXT,
  live_seed TEXT REFERENCES seeds(hash),
  live_tree_hash TEXT,
  live_deployment TEXT,
  preview_seed TEXT REFERENCES seeds(hash),
  preview_weight INTEGER,
  isolation TEXT NOT NULL DEFAULT 'process' CHECK (isolation IN ('container','vm','process','none')),
  memory_limit_bytes INTEGER,
  front_port INTEGER,
  state TEXT NOT NULL,
  state_reason TEXT,
  created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS deployments (
  id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  from_seed TEXT,
  to_seed TEXT,
  tree_hash TEXT,
  commit_ref TEXT,
  actor TEXT NOT NULL,
  reason TEXT,
  cap_diff_json TEXT,
  result TEXT NOT NULL,
  error TEXT,
  port INTEGER,
  pid INTEGER,
  started_at INTEGER NOT NULL,
  live_at INTEGER,
  finished_at INTEGER,
  watch_until INTEGER
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS deployments_app_started ON deployments (app_id, started_at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS deploy_audit (
  seq INTEGER PRIMARY KEY,
  subject_kind TEXT NOT NULL CHECK (subject_kind IN ('app','garden_widget')),
  subject_id TEXT NOT NULL,
  event TEXT NOT NULL,
  actor TEXT NOT NULL,
  on_behalf_of TEXT,
  payload_json TEXT NOT NULL,
  prev_hash TEXT NOT NULL,
  hash TEXT NOT NULL,
  at INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS deploy_audit_subject ON deploy_audit (subject_kind, subject_id, seq);
