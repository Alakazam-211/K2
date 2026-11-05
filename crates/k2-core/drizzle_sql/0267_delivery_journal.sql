-- 0267: the per-agent delivery lanes' journal (0.46.9). A Thread or
-- project chat post answers once it is stored; its wake + inject runs
-- later on the agent's lane (`k2-daemon` `delivery_queue`). Each queued
-- delivery is written here first, so a daemon restart doesn't lose it:
-- boot re-queues the rows that never started. `started_at` is set when the
-- lane picks the row up; a row that started but never finished is not
-- typed in again (it may already have reached the agent), the Thread says
-- so instead. A finished delivery's row goes. `kind` is `thread` or
-- `project`; `body` is the delivery's JSON (the line, the sender, the
-- address). The store is `crate::delivery_journal`.
CREATE TABLE IF NOT EXISTS delivery_journal (
  id TEXT PRIMARY KEY NOT NULL,
  lane TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('thread', 'project')),
  body TEXT NOT NULL,
  queued_at INTEGER NOT NULL,
  started_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_delivery_journal_queued ON delivery_journal (queued_at);
