-- 0332: the hello a template hire wrote, held by the daemon (agent
-- templates T9). At a template hire's first create K2 renders the
-- template's hello into the workspace's .k2/agent/template.lock and keeps
-- its sha256 here, keyed by the workspace id. At the agent's first start
-- the lock's hello is delivered as the owner only when its sha256 matches
-- this row, and the row is taken in the same step, so it goes once. A lock
-- with no row here (not written by a hire) or with another hello (changed
-- after the hire) is never delivered. Store: `crate::hire::template::hello_record`.
-- Safe to run again (IF NOT EXISTS).
CREATE TABLE IF NOT EXISTS template_hello_records (
  workspace_id TEXT    PRIMARY KEY,
  sha256       TEXT    NOT NULL,
  created_at   INTEGER NOT NULL
);
