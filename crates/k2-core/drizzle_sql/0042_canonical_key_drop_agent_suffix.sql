-- 0.37.5: drop the `:<agent_name>` suffix from
-- `workspace_sessions.terminal_id` so the canonical workspace
-- session is addressed purely by project_id.
--
-- Pre-0.37.5 the column held `agent-chat:<pid>:<name>` (e.g.
-- `agent-chat:266f0438-...:scout`). The `:<name>` suffix was
-- vestigial post-0.37.0 unification (one agent per workspace) and
-- caused the renderer to compute the wrong key when its mode→name
-- mapping disagreed with AGENT.md's `name:` field — see C3PO
-- `5c80bef1` (pinned-tab open spawned a duplicate `__lead__` PTY,
-- orphaned the canonical scout session).
--
-- Heartbeat-shaped (`agent-chat:<pid>:<agent>:hb:<schedule>`) and
-- worktree-shaped (`agent-chat:wt:<workspace_id>`) terminal_ids are
-- left untouched — those identify per-heartbeat-fire sessions and
-- worktree-scoped chats respectively, both of which legitimately
-- need a discriminator.
UPDATE workspace_sessions
SET terminal_id = printf('agent-chat:%s', project_id)
WHERE terminal_id IS NOT NULL
  AND terminal_id LIKE 'agent-chat:%'
  AND terminal_id NOT LIKE 'agent-chat:wt:%'
  AND terminal_id NOT LIKE 'agent-chat:%:hb:%';
