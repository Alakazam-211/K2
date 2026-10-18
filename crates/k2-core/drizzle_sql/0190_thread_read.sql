-- thread-read-v1 (0.46.1, Rosson 2026-10-17, ticket 8d8d3d37): unread
-- counts for Threads and project chats, K2-native, shaped like Sew's
-- read model (S14). Research: .k2/prds/research-unread-model-v1.md.
--
-- One read CURSOR per (person, conversation). Counts are never stored:
-- they are derived from the cursor when read. A cursor is a comparator
-- (seq), never a foreign key, so a cursor past a deleted or trimmed item
-- still works. It only moves forward, except when a conversation move
-- renumbers its seqs (k2_core::read_state remaps it then).
--
-- `person` is notify_prefs::People::person_key: `owner` for the owner
-- token and every login the owner linked, else the Connect username.
-- Agents and app guests have no read state.
--
-- `read_people` records when a person was first seen. Seeing a person
-- writes a cursor at the head of every conversation that exists then
-- ("everything starts read"); a conversation created later has no row,
-- which reads as cursor 0 (its first message is unread).
CREATE TABLE IF NOT EXISTS read_people (
    person      TEXT PRIMARY KEY NOT NULL,
    baseline_at INTEGER NOT NULL
);
--> statement-breakpoint
-- kind: 'thread' (conv_id = overlay conversationId) | 'project'
-- (conv_id = project_groups.id). `muted` keeps the dot but drops the
-- conversation from the badge total and stops its pushes;
-- `muted_until` (unix secs, NULL = until unmuted) ends it on its own.
CREATE TABLE IF NOT EXISTS read_cursors (
    person        TEXT NOT NULL,
    kind          TEXT NOT NULL,
    conv_id       TEXT NOT NULL,
    last_read_seq INTEGER NOT NULL DEFAULT 0,
    muted         INTEGER NOT NULL DEFAULT 0,
    muted_until   INTEGER,
    updated_at    INTEGER NOT NULL,
    PRIMARY KEY (person, kind, conv_id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_read_cursors_conv ON read_cursors (kind, conv_id);
--> statement-breakpoint
-- Project chat gains a per-group seq (the public cursor) and who wrote
-- each line: 'person' (owner token / a Connect login) or 'agent'.
ALTER TABLE project_group_messages ADD COLUMN seq INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE project_group_messages ADD COLUMN author_kind TEXT NOT NULL DEFAULT 'agent';
--> statement-breakpoint
ALTER TABLE project_groups ADD COLUMN last_msg_seq INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
-- Number existing lines 1..n per group in their read order
-- (created_at, rowid), the order `list_messages` shows.
UPDATE project_group_messages SET seq = t.n
FROM (
    SELECT id AS mid,
           ROW_NUMBER() OVER (PARTITION BY group_id ORDER BY created_at, rowid) AS n
    FROM project_group_messages
) AS t
WHERE t.mid = project_group_messages.id;
--> statement-breakpoint
UPDATE project_group_messages SET author_kind = 'person' WHERE author = 'owner';
--> statement-breakpoint
UPDATE project_groups SET last_msg_seq = COALESCE(
    (SELECT MAX(m.seq) FROM project_group_messages m WHERE m.group_id = project_groups.id), 0);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_pg_messages_group_seq ON project_group_messages (group_id, seq);
--> statement-breakpoint
-- Backfill: the owner starts fully read. Connect users are seen at daemon
-- boot (their names live in connect-users.json, not in this DB) and start
-- fully read then.
INSERT OR IGNORE INTO read_people (person, baseline_at)
VALUES ('owner', CAST(strftime('%s', 'now') AS INTEGER));
--> statement-breakpoint
INSERT OR IGNORE INTO read_cursors (person, kind, conv_id, last_read_seq, updated_at)
SELECT 'owner', 'thread', conversation_id, last_thread_seq, CAST(strftime('%s', 'now') AS INTEGER)
FROM overlay_conversations;
--> statement-breakpoint
INSERT OR IGNORE INTO read_cursors (person, kind, conv_id, last_read_seq, updated_at)
SELECT 'owner', 'project', id, last_msg_seq, CAST(strftime('%s', 'now') AS INTEGER)
FROM project_groups;
