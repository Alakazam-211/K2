-- Phase 0b (prd-wiki-public-chat-api-loopback-v1): per-workspace owner
-- API guest policy text. Re-injected by the daemon on every host-session
-- spawn and message-live so external API callers are framed as non-owners.
--
-- Owner-writable via `update_project_setting` / `k2 workspace api-guest-policy`.
-- NULL or empty → platform default (see settings::DEFAULT_API_GUEST_POLICY).
-- Callers of /v1 host-sessions cannot set or override this field.
ALTER TABLE projects ADD COLUMN api_guest_policy TEXT;
