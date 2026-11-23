-- 0330: what a template hire left for later, held by the daemon
-- (agent templates, the permissions step). Two kinds:
--   card      a switch the hirer couldn't give, asked of a person on a
--             permission card: `pending` until the new agent first starts
--             (it had no Thread at the hire), then `filed` (with the
--             request id) or `refused`; a re-apply never asks it again.
--   declined  a suggestion the person left off at the hire; a re-apply
--             leaves it off unless a person names it again (--allow), and
--             only one who could grant it with a role at least the
--             decliner's (`rank`: 3 owner, 2 admin, 1 member, 0 agent/key).
-- The new agent can write its own folder, its template.lock included, so
-- neither list is ever read from there. Store: `crate::hire::permission_records`.
-- Safe to run again (IF NOT EXISTS).
CREATE TABLE IF NOT EXISTS hire_permission_records (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  workspace_id TEXT    NOT NULL,
  action       TEXT    NOT NULL,
  kind         TEXT    NOT NULL CHECK (kind IN ('card', 'declined')),
  state        TEXT    NOT NULL CHECK (state IN ('pending', 'filed', 'refused', 'declined')),
  template     TEXT    NOT NULL DEFAULT '',
  hirer        TEXT    NOT NULL DEFAULT '',
  created_at   INTEGER NOT NULL,
  done_at      INTEGER,
  card         TEXT,
  rank         INTEGER NOT NULL DEFAULT 0,
  UNIQUE (workspace_id, action, kind)
);
CREATE INDEX IF NOT EXISTS idx_hire_permission_records_ws ON hire_permission_records (workspace_id, kind, state);
