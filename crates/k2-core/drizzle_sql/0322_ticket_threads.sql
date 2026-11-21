-- 0322: ticket discussions on the Thread store (ticket-thread-v1, 0.46.17).
--
-- Each ticket's discussion is the Thread conversation `ticket:<id>`. Its
-- catalog row in `overlay_conversations` says so: `kind = 'ticket'` and
-- `ticket_id`. A ticket conversation lives as long as its TICKET, not its
-- workspace (tickets outlive their workspace, as before):
--   * the catalog loses its FOREIGN KEY to projects. Deleting a project now
--     deletes its Thread rows through `overlay_conversations_project_deleted`,
--     which leaves `kind = 'ticket'` rows alone;
--   * a ticket's row goes only with its ticket (`overlay_conversations_ticket_deleted`,
--     in the same transaction as the ticket's DELETE). A daily sweep in
--     `k2_core::feedback_thread` removes any ticket row whose ticket is gone.
-- The rebuild derives kind from the key, so a second run keeps every row.
-- `read_retire_replaced_pin` (0190) names the catalog: it is dropped first
-- and created again unchanged after the rename.
--
-- `feedback` gains:
--   * comment_count / file_count: the list's message and file counts, kept
--     on the row when a message is written (were SQL subqueries);
--   * comments_in_thread: 1 once the ticket's messages are in the Thread
--     store (new tickets start at 1; the background backfill moves the
--     rest). `feedback_comments` is read-only from here, kept one release
--     for rollback.
DROP TRIGGER IF EXISTS read_retire_replaced_pin;
--> statement-breakpoint
DROP TABLE IF EXISTS overlay_conversations_0322;
--> statement-breakpoint
CREATE TABLE overlay_conversations_0322 (
    conversation_id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    last_thread_seq INTEGER NOT NULL DEFAULT 0,
    last_chatter_seq INTEGER NOT NULL DEFAULT 0,
    kind TEXT NOT NULL DEFAULT 'thread' CHECK (kind IN ('thread', 'ticket')),
    ticket_id TEXT,
    CHECK ((kind = 'ticket') = (ticket_id IS NOT NULL))
);
--> statement-breakpoint
INSERT INTO overlay_conversations_0322
    (conversation_id, project_id, last_thread_seq, last_chatter_seq, kind, ticket_id)
SELECT conversation_id, project_id, last_thread_seq, last_chatter_seq,
       CASE WHEN conversation_id LIKE 'ticket:%' THEN 'ticket' ELSE 'thread' END,
       CASE WHEN conversation_id LIKE 'ticket:%' THEN substr(conversation_id, 8) ELSE NULL END
FROM overlay_conversations;
--> statement-breakpoint
DROP TABLE overlay_conversations;
--> statement-breakpoint
ALTER TABLE overlay_conversations_0322 RENAME TO overlay_conversations;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS overlay_conversations_project ON overlay_conversations(project_id);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS overlay_conversations_ticket ON overlay_conversations(ticket_id) WHERE ticket_id IS NOT NULL;
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
    DELETE FROM overlay_conversations WHERE project_id = OLD.id AND kind <> 'ticket';
END;
--> statement-breakpoint
CREATE TRIGGER IF NOT EXISTS overlay_conversations_ticket_deleted
AFTER DELETE ON feedback
BEGIN
    DELETE FROM overlay_conversations WHERE kind = 'ticket' AND ticket_id = OLD.id;
END;
--> statement-breakpoint
ALTER TABLE feedback ADD COLUMN comment_count INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE feedback ADD COLUMN file_count INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE feedback ADD COLUMN comments_in_thread INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
UPDATE feedback SET
    comment_count = (SELECT COUNT(*) FROM feedback_comments c WHERE c.feedback_id = feedback.id),
    file_count = (SELECT COALESCE(SUM(CASE WHEN json_valid(c.attachments_json) THEN json_array_length(c.attachments_json) ELSE 0 END), 0)
                  FROM feedback_comments c WHERE c.feedback_id = feedback.id AND c.attachments_json IS NOT NULL)
WHERE comments_in_thread = 0;
