-- 0263: "Can refresh its own pinned chat" (prd answers-agent-chat-refresh-v1,
-- Rosson 2026-11-03; 0.46.8). DEFAULT 0 for new and existing workspaces.
-- Written only by POST /cli/agent-access/set {toggle: "chat_refresh"} (an
-- Owner or Admin login, or the owner token; never an agent) and by "Always
-- allow" on the chat.refresh card, through that same route entry. Read by
-- its own query, never in Project's main SELECT, so a database restored from
-- before this column still lists its projects.
ALTER TABLE projects ADD COLUMN agents_can_refresh_chat INTEGER NOT NULL DEFAULT 0;
