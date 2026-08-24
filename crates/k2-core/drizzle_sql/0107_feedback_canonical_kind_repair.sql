-- One-shot repair: AFSROW-style poison stamps (session_kind='sandbox' on
-- the workspace's pinned-chat conversation id) become canonical.
-- Idempotent. Correlated on project_id so a sandbox cell that happens
-- to reuse another workspace's conversation UUID is not rewritten.
UPDATE feedback SET session_kind='canonical'
WHERE session_kind='sandbox'
  AND session_id IN (
    SELECT session_id FROM workspace_sessions
    WHERE session_id IS NOT NULL AND project_id = feedback.project_id
  );
