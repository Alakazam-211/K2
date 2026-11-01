-- 0258: post idempotency for project chats (post-idempotency-v1, 0.46.8;
-- .k2/prds/vslive-phone-speed-b1-b2-rev-v1.md §4). Thread posts keep their
-- keys in redb (k2-threads.redb `client_ids`), not here.
--
-- `project_group_client_ids` — one row per client key a project chat post
-- carried: (sender namespace, clientMsgId) → the stored message. The
-- namespace comes from the sender's credential (`p:<person>`, `g:<App
-- guest>`, `a:<workspace>`), never the body. The key is the primary key,
-- so the first write wins (it is written in the post's own savepoint), and
-- a key already used in ANOTHER project is found (409
-- `client_msg_id_reused`). `created_ms` is unix milliseconds; the daemon
-- sweeps rows older than 24 h (hourly), so a key lives 24–25 h.
CREATE TABLE IF NOT EXISTS project_group_client_ids (
  ns            TEXT    NOT NULL,
  client_msg_id TEXT    NOT NULL,
  group_id      TEXT    NOT NULL,
  message_id    TEXT    NOT NULL REFERENCES project_group_messages(id) ON DELETE CASCADE,
  created_ms    INTEGER NOT NULL,
  PRIMARY KEY (ns, client_msg_id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS project_group_client_ids_created ON project_group_client_ids (created_ms);
--> statement-breakpoint
-- The id echoed on the message (its row, its frames), so the sender's
-- outbox can settle. NULL on lines posted without one.
ALTER TABLE project_group_messages ADD COLUMN client_msg_id TEXT;
