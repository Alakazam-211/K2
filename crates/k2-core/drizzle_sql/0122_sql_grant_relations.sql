-- Relation fence for a workspace sql_grants row (k2 db grant --relation).
-- Child of sql_grants. No rows means today's broad grant (ALL TABLES
-- plus default privileges). A later grant replaces these rows; it does
-- not append. Deleting the parent grant deletes the list.
CREATE TABLE IF NOT EXISTS sql_grant_relations (
    database_id   TEXT NOT NULL,
    project_id    TEXT NOT NULL,
    schema_name   TEXT NOT NULL,
    relation_name TEXT NOT NULL,
    position      INTEGER NOT NULL,
    PRIMARY KEY (database_id, project_id, schema_name, relation_name),
    FOREIGN KEY (database_id, project_id)
        REFERENCES sql_grants (database_id, project_id)
        ON DELETE CASCADE
);
