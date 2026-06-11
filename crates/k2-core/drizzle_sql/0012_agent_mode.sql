-- Add agent mode columns to projects table
ALTER TABLE `projects` ADD COLUMN `agent_enabled` integer NOT NULL DEFAULT 0;
ALTER TABLE `projects` ADD COLUMN `heartbeat_enabled` integer NOT NULL DEFAULT 0;
