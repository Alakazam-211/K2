-- Agent session tracking for persistent channel sessions.
-- Tags chat sessions so K2SO can recover persistent agents after crashes.

ALTER TABLE `chat_session_names` ADD COLUMN `is_agent_session` integer NOT NULL DEFAULT 0;
ALTER TABLE `chat_session_names` ADD COLUMN `agent_name` text;
ALTER TABLE `chat_session_names` ADD COLUMN `agent_project_id` text;
