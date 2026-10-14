-- 0164_garden_data — Garden data grants and widget authorship
-- (prd-gardens-local-services-v1 rev 3 §5.5, §4.2; contract-garden-data-v1 §2.1).
-- Times: INTEGER seconds since the Unix epoch (UTC).
-- No foreign keys: workspace delete, project-group delete and space delete
-- remove these rows explicitly in their own transactions (GL33).

CREATE TABLE IF NOT EXISTS zen_data_grants (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  space         TEXT NOT NULL CHECK (length(space) BETWEEN 6 AND 64 AND space LIKE 'data:%'),
  prefix        TEXT NOT NULL DEFAULT '' CHECK (length(prefix) <= 256),
  subject_kind  TEXT NOT NULL CHECK (subject_kind IN ('workspace', 'project')),
  subject_id    TEXT NOT NULL CHECK (length(subject_id) BETWEEN 1 AND 64),
  level         TEXT NOT NULL CHECK (level IN ('read', 'write')),
  admin         INTEGER NOT NULL DEFAULT 0 CHECK (admin IN (0, 1)),
  via_grant     INTEGER,
  granted_by    TEXT NOT NULL,             -- 'owner' | 'agent:<projects.id>'
  granted_at    INTEGER NOT NULL,
  CHECK (admin = 0 OR (subject_kind = 'workspace' AND via_grant IS NULL AND granted_by = 'owner')),
  CHECK ((via_grant IS NULL) = (granted_by = 'owner')),
  UNIQUE (space, prefix, subject_kind, subject_id)
);
CREATE INDEX IF NOT EXISTS zen_data_grants_subject ON zen_data_grants(subject_kind, subject_id);
CREATE INDEX IF NOT EXISTS zen_data_grants_space ON zen_data_grants(space);
CREATE INDEX IF NOT EXISTS zen_data_grants_via ON zen_data_grants(via_grant) WHERE via_grant IS NOT NULL;

CREATE TABLE IF NOT EXISTS zen_widget_authors (
  widget             TEXT NOT NULL CHECK (length(widget) BETWEEN 1 AND 60),
  hash               TEXT NOT NULL CHECK (length(hash) = 64),
  author_kind        TEXT NOT NULL CHECK (author_kind IN ('owner', 'agent')),
  author_project_id  TEXT,
  source             TEXT NOT NULL CHECK (source IN ('new', 'sign', 'claim', 'import')),
  caller_kind        TEXT NOT NULL CHECK (caller_kind IN ('owner', 'agent', 'shell')),
  signed_at          INTEGER NOT NULL,
  CHECK ((author_kind = 'agent') = (author_project_id IS NOT NULL)),
  PRIMARY KEY (widget, hash)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS zen_widget_authors_by_widget ON zen_widget_authors(widget, signed_at);
CREATE INDEX IF NOT EXISTS zen_widget_authors_by_project ON zen_widget_authors(author_project_id)
  WHERE author_project_id IS NOT NULL;
