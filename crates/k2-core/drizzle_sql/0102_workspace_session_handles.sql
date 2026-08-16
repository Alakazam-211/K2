-- Sidecar identity (prd-sidecar-identity-and-addressing-v1): durable
-- per-workspace ordinals for extra harness sessions. Canonical (pinned)
-- chat is NOT stored here — its address is just the workspace name.
--
-- conversation_key is durable: prefer the provider conversation id
-- (workspace_tab_sessions.session_id / claude --resume uuid); else the
-- pane_group_id / tab key. Resume / re-wake must reuse the same row.
-- Unique (project_id, ordinal) so we never double-issue. v1 never
-- recycles ordinals (do not fill holes).

CREATE TABLE workspace_session_handles (
    project_id TEXT NOT NULL,
    conversation_key TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (project_id, conversation_key)
);

CREATE UNIQUE INDEX idx_workspace_session_handles_ordinal
    ON workspace_session_handles(project_id, ordinal);
