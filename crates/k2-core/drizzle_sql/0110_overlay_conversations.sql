-- Overlay threads catalog (prd-overlay-threads-v1 S1).
-- conversation_id is the named session: workspace_session_handles.conversation_key
-- or pinned workspace_sessions.session_id. Never v2_session_map.
-- Message bodies live in redb (~/.k2/k2-threads.redb), not SQL columns.

CREATE TABLE overlay_conversations (
    conversation_id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    last_thread_seq INTEGER NOT NULL DEFAULT 0,
    last_chatter_seq INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE INDEX overlay_conversations_project
    ON overlay_conversations(project_id);

-- Host-global ChatterLog sequence (not per conversation).
CREATE TABLE overlay_host (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    last_chatterlog_seq INTEGER NOT NULL DEFAULT 0
);

INSERT INTO overlay_host (id, last_chatterlog_seq) VALUES (1, 0);
