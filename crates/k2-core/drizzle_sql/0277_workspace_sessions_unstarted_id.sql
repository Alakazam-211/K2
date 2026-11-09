-- 0277: the pre-allocated id of a main chat that has not started yet (0.46.11).
-- When K2 pre-allocates a `--session-id` the transcript appears only after the
-- first turn, so a missing file is normal for that id and that id alone. Any
-- other pinned id whose transcript is gone was a real conversation: K2 keeps
-- the pin and reports `main_chat_missing` rather than choosing another chat.
-- '' once a transcript has been seen on disk. NULL on rows from before this
-- column: those count as real unless the workspace has no transcript in any
-- home (an id allocated for a chat that never ran). Renumber freely on a slip
-- (and edit the migration-order tests). The reader is `workspace::resume_chat`.
ALTER TABLE workspace_sessions ADD COLUMN unstarted_session_id TEXT;
