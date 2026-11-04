-- 0304: one-click suite Apps (prd-one-click-suite-apps-v1 R41,
-- vs-live A7/A23). The store is `crate::suite::jobs`.
--
-- suite_install_jobs: one row per `k2 app install --catalog` (or the
-- Settings "Add an App" sheet). `step` is the next step to run; a daemon
-- restart resumes there (every earlier step is in steps_json). `who_json`
-- is the caller (kind, workspace or login), never a token. `pin_json` is
-- the catalog version the job started from (its sha256 and seedHash pins).
-- No secret is ever stored here.
CREATE TABLE IF NOT EXISTS suite_install_jobs (
  id TEXT PRIMARY KEY NOT NULL,
  entry TEXT NOT NULL,
  version TEXT NOT NULL,
  variant TEXT NOT NULL CHECK (variant IN ('k2', 'standalone')),
  app_id TEXT NOT NULL,
  workspace_id TEXT NOT NULL,
  address_kind TEXT NOT NULL CHECK (address_kind IN ('suite_label', 'label')),
  address TEXT NOT NULL,
  actor TEXT NOT NULL,
  who_json TEXT NOT NULL,
  pin_json TEXT NOT NULL,
  options_json TEXT NOT NULL DEFAULT '{}',
  step TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('running', 'waiting', 'failed', 'done')),
  wait_reason TEXT,
  wait_question TEXT,
  wait_requests TEXT NOT NULL DEFAULT '[]',
  wait_hint TEXT,
  error_code TEXT,
  error_hint TEXT,
  seed_hash TEXT,
  deployment_id TEXT,
  space_id TEXT,
  steps_json TEXT NOT NULL DEFAULT '[]',
  step_started_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  finished_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_suite_install_jobs_app ON suite_install_jobs (app_id, created_at);
CREATE INDEX IF NOT EXISTS idx_suite_install_jobs_state ON suite_install_jobs (state);

-- suite_apps: the catalog lineage of an App installed from the catalog
-- (R34 "Verified by K2", R36 follow, R39 forked, R38 bad versions). Goes
-- with the App on uninstall.
CREATE TABLE IF NOT EXISTS suite_apps (
  app_id TEXT PRIMARY KEY NOT NULL,
  entry TEXT NOT NULL,
  variant TEXT NOT NULL,
  version TEXT NOT NULL,
  seed_hash TEXT NOT NULL,
  follow INTEGER NOT NULL DEFAULT 1,
  forked INTEGER NOT NULL DEFAULT 0,
  bad_versions TEXT NOT NULL DEFAULT '[]',
  job_id TEXT NOT NULL,
  installed_by TEXT NOT NULL,
  installed_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_suite_apps_entry ON suite_apps (entry);

-- suite_labels: a reserved family label (chat, meet) bound box-level to
-- one App (R8, A7). The URL is derived from the box's current subdomain
-- when read, so a relabel needs no rewrite here. Released on uninstall.
-- The older `k2 hostchat meet bind` record (~/.k2/sew/meet.json) stays as
-- it is beside this table.
CREATE TABLE IF NOT EXISTS suite_labels (
  label TEXT PRIMARY KEY NOT NULL,
  app_id TEXT NOT NULL,
  front_port INTEGER NOT NULL,
  bound_at INTEGER NOT NULL,
  bound_by TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_suite_labels_app ON suite_labels (app_id);
