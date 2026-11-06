-- 0272: compute git access (0.46.10, prd-compute-git-access-v1, GH#78).
-- `compute_git_grants`: which workspace's compute jobs may read which repo
-- (`github.com/<owner>/<repo>`, lowercase) on which nodes: `nodes` is '*'
-- (every node this server owns, now and later) or one node id. A node
-- another server owns is covered only by a row naming it with `shared` = 1.
-- `compute_git_credentials`: the server's forge credential, facts only
-- (kind, app id); the key or token itself is in the vault (system
-- namespace, engine scope 'compute-git'). Renumber freely on a slip (and
-- edit the migration-order tests).
CREATE TABLE IF NOT EXISTS compute_git_grants (
  workspace_id TEXT NOT NULL,
  forge        TEXT NOT NULL DEFAULT 'github' CHECK (forge IN ('github')),
  repo         TEXT NOT NULL,
  nodes        TEXT NOT NULL,
  shared       INTEGER NOT NULL DEFAULT 0,
  granted_by   TEXT NOT NULL DEFAULT '',
  granted_at   INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, repo, nodes)
);
CREATE INDEX IF NOT EXISTS idx_compute_git_grants_repo ON compute_git_grants(repo);

CREATE TABLE IF NOT EXISTS compute_git_credentials (
  forge   TEXT PRIMARY KEY NOT NULL CHECK (forge IN ('github')),
  kind    TEXT NOT NULL CHECK (kind IN ('app','token')),
  app_id  TEXT,
  set_by  TEXT NOT NULL DEFAULT '',
  set_at  INTEGER NOT NULL
);
