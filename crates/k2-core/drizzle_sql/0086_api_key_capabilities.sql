-- Phase 0 (prd-wiki-public-chat-api-loopback-v1): per-key CAPABILITIES so
-- third-party / wiki-chat keys can open only the doors they need.
--
-- Three additive INTEGER (0/1) columns on `api_keys`:
--   • cap_host_sessions      — POST/GET /v1/w/<ws>/host-sessions*
--   • cap_canonical_message  — POST /v1/w/<ws>/message (canonical inject)
--   • cap_sandboxes          — /v1/sandboxes* and /v1/w/<ws>/sessions*
--
-- BACK-COMPAT: DEFAULT 1 so EXISTING rows (minted before this migration)
-- keep today's behavior = all doors on. NEW keys are written explicitly by
-- `create_api_key` with the Phase-0 defaults (host_sessions=1,
-- canonical_message=0, sandboxes=0) — the wiki-chat recipe.
--
-- Owner-token principals always have every capability (route layer).
-- A missing capability yields the same uniform 404 as a missing grant
-- (no existence oracle). Additive ADD COLUMNs, run once by name.
ALTER TABLE api_keys ADD COLUMN cap_host_sessions INTEGER NOT NULL DEFAULT 1;
--> statement-breakpoint
ALTER TABLE api_keys ADD COLUMN cap_canonical_message INTEGER NOT NULL DEFAULT 1;
--> statement-breakpoint
ALTER TABLE api_keys ADD COLUMN cap_sandboxes INTEGER NOT NULL DEFAULT 1;
