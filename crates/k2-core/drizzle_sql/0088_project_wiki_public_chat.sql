-- Phase 1 (prd-wiki-public-chat-api-loopback-v1): per-workspace opt-in for
-- public wiki chat. When ON, visitors to the served/published wiki SPA may
-- ask the workspace agent (via Phase 2 same-origin /api/chat proxy).
--
-- Default 0 (OFF): serve alone never enables chat (D6).
-- Owner-writable via update_project_setting / k2 wiki chat on|off.
-- Additive; fail-closed for unknown paths.
ALTER TABLE projects ADD COLUMN wiki_public_chat INTEGER NOT NULL DEFAULT 0;
