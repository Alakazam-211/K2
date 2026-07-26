-- Context hamburger (prd-context-hamburger-v1): optional AGENTS.md layers.
-- SSOT for ordered, toggleable path references per workspace/project.
-- Bodies are NEVER stored — only path + order + enabled + source + label.
-- Empty stack = today's compose (AGENT + PROJECT + Tooling).

CREATE TABLE IF NOT EXISTS project_context_layers (
  id TEXT PRIMARY KEY NOT NULL,
  project_id TEXT NOT NULL,
  path TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  position INTEGER NOT NULL,
  source TEXT NOT NULL DEFAULT 'user',
  label TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_project_context_layers_project
  ON project_context_layers(project_id, position);
