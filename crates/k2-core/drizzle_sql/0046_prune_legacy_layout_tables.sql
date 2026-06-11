-- 0046: drop legacy terminal layout tables (0000-era, never shipped).
--
-- terminal_tabs and terminal_panes were created in migration 0000 as
-- part of a normalized layout design that never materialized — the
-- renderer went with JSON-in-workspace_layouts.layout_json instead.
-- Both tables have accumulated zero reads and zero writes across the
-- entire codebase since creation. Confirmed by schema necessity audit
-- 2026-05-23 plus the rendered-or-used heuristic check (neither table
-- is rendered anywhere, neither table is used in any function).

DROP INDEX IF EXISTS terminal_panes_tab_id;
DROP TABLE IF EXISTS terminal_panes;
DROP INDEX IF EXISTS terminal_tabs_workspace_id;
DROP TABLE IF EXISTS terminal_tabs;
