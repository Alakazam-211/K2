-- Replace boolean agent_enabled with agent_mode text field (off/agent/pod)
-- Migrate existing data: agent_enabled=1 becomes 'agent', agent_enabled=0 becomes 'off'
ALTER TABLE `projects` ADD COLUMN `agent_mode` text NOT NULL DEFAULT 'off';
UPDATE `projects` SET `agent_mode` = 'agent' WHERE `agent_enabled` = 1;
