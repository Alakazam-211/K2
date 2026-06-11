-- 0.38.5 — workspace_tab_sessions: daemon-side persistence of per-pane
-- session metadata so Cmd+T terminal tabs survive daemon restart (app
-- update, manual kickstart, crash recovery).
--
-- Pre-0.38.5 problem
-- ------------------
-- Schema v2 (0.38.0) stripped command / args / sessionId from terminal
-- items in `workspace_layouts.layout_json` because the daemon's
-- in-memory `v2_session_map` was supposed to be the source of truth.
-- That holds while the daemon stays alive — close + reopen Tauri works
-- great. It breaks on daemon restart: `v2_session_map` evaporates with
-- the process, the renderer's spawn request for the same paneGroupId
-- arrives at the new daemon with no command, daemon defaults to shell,
-- the user's `claude --resume <id>` tab becomes a generic shell.
--
-- Why a new table instead of reusing legacy `terminal_panes`
-- ---------------------------------------------------------
-- `terminal_tabs` + `terminal_panes` were created in migration 0000
-- (`0000_lethal_scalphunter.sql`) for a renderer-normalized layout
-- design that never shipped — the renderer went with the JSON blob in
-- `workspace_layouts.layout_json` instead, and both tables have sat at
-- 0 rows ever since. Their column shape is renderer layout metadata
-- (`split_direction`, `split_ratio`, `pane_order`) plus an enforced
-- FK chain `terminal_panes → terminal_tabs → workspaces` that doesn't
-- match what the daemon needs to write. Repurposing would require
-- dismantling the FK chain (full SQLite table rebuild) for a fit
-- that's still awkward. Cleaner to drop the unused legacy tables and
-- create a purpose-built one.
--
-- Architecture
-- ------------
-- - DAEMON owns this table. The renderer never writes to it.
-- - `v2_session_map::register` upserts on every PTY registration.
-- - `v2_session_map::unregister` leaves the row alone (we want it for
--   next-restart recovery; "session is gone" is signaled by absence
--   from `v2_session_map`, not absence from this table).
-- - `v2_spawn::handle_v2_spawn` consults this table before spawning.
--   If a row exists for `(project_id, pane_group_id)`, the saved
--   command + args + (optional) `session_id` drive the spawn —
--   `claude --resume <session_id>` instead of a generic shell.
-- - The renderer continues to know nothing about session ids for
--   terminal items in `layout_json` v2; the daemon handles continuity
--   under the hood.

DROP TABLE IF EXISTS terminal_panes;
DROP TABLE IF EXISTS terminal_tabs;

CREATE TABLE workspace_tab_sessions (
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,

    -- The canonical key the daemon uses in v2_session_map. For tab-
    -- driven sessions this is the renderer's paneGroupId; the
    -- agent_name in v2_session_map is `tab-<pane_group_id>`. For
    -- pinned chat / heartbeat sessions, this equals the agent_name
    -- (bare project UUID or heartbeat name) since their canonical
    -- key isn't `tab-`-prefixed.
    pane_group_id TEXT NOT NULL,

    -- The full agent_name in v2_session_map. Stored explicitly so
    -- the daemon can look up the row without recomputing the prefix
    -- rule (`tab-<X>` vs bare). Also useful for audit/debug.
    agent_name    TEXT NOT NULL,

    -- The CLI tool's own session id (e.g. claude --resume <uuid>).
    -- Stamped by the renderer via `set_session_id` once the tool
    -- reports its session id. Null when the tab is a generic shell
    -- or no resume capability exists.
    session_id    TEXT,

    -- The argv[0] and rest of args, as the daemon would have spawned
    -- the PTY originally. `args_json` is JSON-serialized
    -- `Vec<String>` so it round-trips cleanly through serde.
    command       TEXT,
    args_json     TEXT,

    -- Where the tab was rooted. Persisted because the renderer's
    -- spawn request after a daemon restart may not include cwd —
    -- the daemon falls back to the project path otherwise.
    cwd           TEXT,

    last_seen_at  INTEGER NOT NULL DEFAULT (unixepoch()),

    PRIMARY KEY (project_id, pane_group_id)
);

CREATE INDEX idx_workspace_tab_sessions_agent_name
    ON workspace_tab_sessions(project_id, agent_name);
