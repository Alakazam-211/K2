-- Design mode (prd-browser-design-mode-v1, Rosson 2026-10-16): per-workspace
-- switch for the pinned Design tab and the + menu's Design entry. Daemon-owned
-- so every client of the room agrees. Default ON.
ALTER TABLE projects ADD COLUMN design_mode_enabled INTEGER NOT NULL DEFAULT 1;
