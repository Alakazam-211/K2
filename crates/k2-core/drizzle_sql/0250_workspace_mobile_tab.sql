-- 0250_workspace_mobile_tab — the pinned Mobile tab (Rosson 2026-10-29,
-- 0.46.7). Per-workspace "Show the Mobile tab": NULL = auto (pinned while
-- this server can run a phone, i.e. at least one usable sim lane), 0 = off,
-- 1 = on. Daemon-owned so every client of the room agrees. Written via
-- `/cli/workspace/set {mobile_tab_enabled}` / `k2 workspace mobile-tab`.
ALTER TABLE projects ADD COLUMN mobile_tab_enabled INTEGER;
