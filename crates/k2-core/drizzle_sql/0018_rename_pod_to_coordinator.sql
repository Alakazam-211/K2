-- Rename 'pod' agent mode to 'coordinator'
UPDATE projects SET agent_mode = 'coordinator' WHERE agent_mode = 'pod';
