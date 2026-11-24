-- 0331: project chats on the Thread store (project-chat-thread-v1).
--
-- Each project's chat is the Thread conversation `project:<group id>`. Its
-- catalog row in `overlay_conversations` says so: `kind = 'project'` and
-- `group_id`. Its `project_id` is the group id too (the column has had no
-- FK since 0322; it is NOT a workspace id, and the daemon's project_addr
-- overrides every use that reads it as one). The chat lives as long as its
-- PROJECT, never its members or its point of contact:
--   * the workspace-delete trigger now takes `kind = 'thread'` rows only
--     (it took every kind but 'ticket');
--   * a project's row goes with the project
--     (`overlay_conversations_group_deleted`, same transaction as the
--     project's DELETE). A daily sweep in `k2_core::project_chat_thread`
--     removes any project row whose project is gone.
-- The rebuild derives kind from the key, so a second run keeps every row.
-- `read_retire_replaced_pin` (0190) names the catalog: it is dropped first
-- and created again unchanged after the rename.
--
-- `project_groups` gains `messages_in_thread`: 1 once the project's lines
-- are in the Thread store (new projects start at 1; the background
-- backfill moves the rest). `project_group_messages` and
-- `project_group_client_ids` are read-only from here apart from the
-- rollback window's dual write, and are dropped in a later migration.
DROP TRIGGER IF EXISTS read_retire_replaced_pin;
--> statement-breakpoint
DROP TRIGGER IF EXISTS overlay_conversations_project_deleted;
--> statement-breakpoint
DROP TRIGGER IF EXISTS overlay_conversations_ticket_deleted;
--> statement-breakpoint
DROP TRIGGER IF EXISTS overlay_conversations_group_deleted;
--> statement-breakpoint
DROP TABLE IF EXISTS overlay_conversations_0331;
--> statement-breakpoint
CREATE TABLE overlay_conversations_0331 (
    conversation_id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    last_thread_seq INTEGER NOT NULL DEFAULT 0,
    last_chatter_seq INTEGER NOT NULL DEFAULT 0,
    kind TEXT NOT NULL DEFAULT 'thread' CHECK (kind IN ('thread', 'ticket', 'project')),
    ticket_id TEXT,
    group_id TEXT,
    CHECK ((kind = 'ticket') = (ticket_id IS NOT NULL)),
    CHECK ((kind = 'project') = (group_id IS NOT NULL))
);
--> statement-breakpoint
INSERT INTO overlay_conversations_0331
    (conversation_id, project_id, last_thread_seq, last_chatter_seq, kind, ticket_id, group_id)
SELECT conversation_id, project_id, last_thread_seq, last_chatter_seq,
       CASE WHEN conversation_id LIKE 'ticket:%' THEN 'ticket'
            WHEN conversation_id LIKE 'project:%' THEN 'project'
            ELSE 'thread' END,
       CASE WHEN conversation_id LIKE 'ticket:%' THEN substr(conversation_id, 8) ELSE NULL END,
       CASE WHEN conversation_id LIKE 'project:%' THEN substr(conversation_id, 9) ELSE NULL END
FROM overlay_conversations;
--> statement-breakpoint
DROP TABLE overlay_conversations;
--> statement-breakpoint
ALTER TABLE overlay_conversations_0331 RENAME TO overlay_conversations;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS overlay_conversations_project ON overlay_conversations(project_id);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS overlay_conversations_ticket ON overlay_conversations(ticket_id) WHERE ticket_id IS NOT NULL;
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS overlay_conversations_group ON overlay_conversations(group_id) WHERE group_id IS NOT NULL;
--> statement-breakpoint
CREATE TRIGGER IF NOT EXISTS read_retire_replaced_pin
AFTER UPDATE OF session_id ON workspace_sessions
WHEN OLD.session_id IS NOT NULL AND TRIM(OLD.session_id) != ''
     AND (NEW.session_id IS NULL OR NEW.session_id != OLD.session_id)
BEGIN
    UPDATE read_cursors
       SET last_read_seq = MAX(last_read_seq,
               (SELECT last_thread_seq FROM overlay_conversations WHERE conversation_id = OLD.session_id)),
           updated_at = CAST(strftime('%s', 'now') AS INTEGER)
     WHERE kind = 'thread' AND conv_id = OLD.session_id
       AND EXISTS (SELECT 1 FROM overlay_conversations WHERE conversation_id = OLD.session_id);
    -- No OR IGNORE: inside a trigger the outer statement's conflict policy
    -- (an upsert on workspace_sessions) would override it. Skip existing
    -- rows instead.
    INSERT INTO read_cursors (person, kind, conv_id, last_read_seq, updated_at)
        SELECT p.person, 'thread', OLD.session_id, c.last_thread_seq, CAST(strftime('%s', 'now') AS INTEGER)
        FROM read_people p, overlay_conversations c
        WHERE c.conversation_id = OLD.session_id
          AND NOT EXISTS (SELECT 1 FROM read_cursors r
                          WHERE r.person = p.person AND r.kind = 'thread' AND r.conv_id = OLD.session_id);
END;
--> statement-breakpoint
CREATE TRIGGER IF NOT EXISTS overlay_conversations_project_deleted
AFTER DELETE ON projects
BEGIN
    DELETE FROM overlay_conversations WHERE project_id = OLD.id AND kind = 'thread';
END;
--> statement-breakpoint
CREATE TRIGGER IF NOT EXISTS overlay_conversations_ticket_deleted
AFTER DELETE ON feedback
BEGIN
    DELETE FROM overlay_conversations WHERE kind = 'ticket' AND ticket_id = OLD.id;
END;
--> statement-breakpoint
CREATE TRIGGER IF NOT EXISTS overlay_conversations_group_deleted
AFTER DELETE ON project_groups
BEGIN
    DELETE FROM overlay_conversations WHERE kind = 'project' AND group_id = OLD.id;
END;
--> statement-breakpoint
ALTER TABLE project_groups ADD COLUMN messages_in_thread INTEGER NOT NULL DEFAULT 0;
