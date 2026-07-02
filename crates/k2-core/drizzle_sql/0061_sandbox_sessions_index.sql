-- Sandbox v2 (fs-mirror PRD §5) — the host BRIDGE index for workspace-scoped
-- sandbox sessions. One row per (mirror) sandbox session: the host knows the
-- REAL paths, the cell is where claude's relative resolution works, and this
-- table is the source of truth that (a) lets the per-workspace session LIST
-- surface sandbox sessions for audit, and (b) tells the RESUME path which
-- sandbox home + /work layer to re-mount.
--
-- The claude-session-id == K2_SESSION_ID (the daemon injects it, so the
-- guest-init runs `claude --session-id <it>`), therefore the `.jsonl` real path
-- is KNOWN at register time: <sandbox_home_path>/projects/<slug>/<session_id>.jsonl.
--
-- Separate table (not `workspace_sessions`, which is the OFF-LIMITS canonical
-- cockpit session, 1-row-per-workspace) — a sandbox session is a distinct,
-- many-per-workspace, api-spawned entity, so it gets its own table rather than
-- overloading the canonical one.
CREATE TABLE IF NOT EXISTS sandbox_sessions (
    session_id        TEXT PRIMARY KEY,          -- the forced SessionId == K2_SESSION_ID == the .jsonl key
    workspace_slug    TEXT NOT NULL,             -- the URL slug the session was addressed under (e.g. 'ai')
    workspace_path    TEXT NOT NULL,             -- the REAL workspace path == the in-cell cwd (mirror)
    sandbox_home_path TEXT NOT NULL,             -- host: ~/.k2/sandbox-homes/<ws>/.claude (the per-ws sandbox home)
    jsonl_path        TEXT NOT NULL,             -- <sandbox_home_path>/projects/<slug>/<session_id>.jsonl
    layer_path        TEXT NOT NULL,             -- host: ~/.k2/sandbox-overlays/<ws>/<sid>/work-scratch (the /work layer)
    slug              TEXT NOT NULL,             -- claude project slug = workspace_path with '/'→'-'
    created_at        INTEGER NOT NULL DEFAULT (unixepoch()),
    last_active_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

-- List/audit is per-workspace + newest-first.
CREATE INDEX IF NOT EXISTS idx_sandbox_sessions_ws
    ON sandbox_sessions (workspace_slug, created_at DESC);
