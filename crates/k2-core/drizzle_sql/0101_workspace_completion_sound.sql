-- Per-workspace completion chime mute. 1 = chime when an unwatched agent
-- in this workspace finishes (AND the global Settings → General toggle).
-- Default ON so existing workspaces keep today's behavior.
ALTER TABLE projects ADD COLUMN completion_sound_enabled INTEGER NOT NULL DEFAULT 1;
