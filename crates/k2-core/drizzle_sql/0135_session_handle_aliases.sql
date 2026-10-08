-- 0135: Thread survives a tab rename (prd-thread-survives-tab-rename-v1).
--
-- A sidecar's address is `ws/<ordinal>` (permanent, never recycled) or
-- `ws/<slug of its Chats name>`. A rename used to kill the old address.
-- Now every name a chat has had stays an alias of that chat, and is
-- reserved to it, until the chat is archived or the owner runs
-- "Release old names". The current name always wins; an all-digit
-- token is only ever an ordinal.
--
-- conversation_key matches workspace_session_handles: the provider
-- conversation id, or the pane key before one is known (rekeyed with
-- the handle row on adoption).
CREATE TABLE IF NOT EXISTS workspace_session_handle_aliases (
    project_id       TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    slug             TEXT NOT NULL,
    conversation_key TEXT NOT NULL,
    retired_at       INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (project_id, slug)
);

CREATE INDEX IF NOT EXISTS idx_workspace_session_handle_aliases_conversation
    ON workspace_session_handle_aliases(project_id, conversation_key);

-- A restored chat whose current name another chat took while it was
-- archived keeps its Chats label but does not answer at it (TR6b). It
-- answers at ws/<ordinal> until it is renamed.
CREATE TABLE IF NOT EXISTS workspace_session_unclaimed_names (
    project_id       TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    conversation_key TEXT NOT NULL,
    created_at       INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (project_id, conversation_key)
);
