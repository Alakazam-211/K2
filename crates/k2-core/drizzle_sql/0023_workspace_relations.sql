-- Cross-workspace relationships: custom agents overseeing workspace managers.
-- source_project_id = the custom agent workspace
-- target_project_id = the workspace being overseen
CREATE TABLE IF NOT EXISTS workspace_relations (
  id TEXT PRIMARY KEY,
  source_project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  target_project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  relation_type TEXT NOT NULL DEFAULT 'oversees',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(source_project_id, target_project_id, relation_type)
);
CREATE INDEX IF NOT EXISTS idx_relations_source ON workspace_relations(source_project_id);
CREATE INDEX IF NOT EXISTS idx_relations_target ON workspace_relations(target_project_id);
