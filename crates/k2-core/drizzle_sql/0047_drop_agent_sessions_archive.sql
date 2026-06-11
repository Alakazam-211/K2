-- 0047: drop agent_sessions migration archive.
--
-- workspace_sessions_legacy_archive was created in migration 0039 as
-- a one-time backup before collapsing multi-agent agent_sessions rows
-- into single-row workspace_sessions (post-0.39 unification). Archive
-- served its purpose; no ongoing reference in code; no user data to
-- preserve. Schema necessity audit confirms it's mothballed.

DROP TABLE IF EXISTS workspace_sessions_legacy_archive;
