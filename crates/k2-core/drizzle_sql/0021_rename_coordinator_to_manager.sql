-- Rename coordinator → manager (follows 0018 which renamed pod → coordinator)
UPDATE projects SET agent_mode = 'manager' WHERE agent_mode = 'coordinator';
UPDATE projects SET agent_mode = 'manager' WHERE agent_mode = 'pod';
