ALTER TABLE projects ADD COLUMN manually_active INTEGER NOT NULL DEFAULT 0;
ALTER TABLE projects ADD COLUMN last_interaction_at INTEGER;
