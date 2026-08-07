-- Per-workspace concurrent host-session (live cell) cap.
--
-- NULL = inherit the daemon default (env K2_SANDBOX_WORKSPACE_CELL_CAP,
-- or product default 15). When set, must be a positive integer; the write
-- path clamps to MAX_HOST_SESSION_CELL_CAP (512) so agents cannot raise
-- the ceiling to unbounded. See
-- `k2_core::workspace::settings::get_host_session_cell_cap` and
-- `k2 workspace host-session-cell-cap get|set`.
ALTER TABLE projects ADD COLUMN host_session_cell_cap INTEGER;
