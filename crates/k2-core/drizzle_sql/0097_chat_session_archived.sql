ALTER TABLE chat_session_names ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chat_session_names ADD COLUMN archived_at INTEGER NULL;
ALTER TABLE chat_session_names ADD COLUMN archive_project_path TEXT NULL;
ALTER TABLE chat_session_names ADD COLUMN archive_title TEXT NULL;
ALTER TABLE chat_session_names ADD COLUMN archive_timestamp INTEGER NULL;
ALTER TABLE chat_session_names ADD COLUMN archive_source_path TEXT NULL;
