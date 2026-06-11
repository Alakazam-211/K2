-- Per-project record of the K2SO version that last regenerated the
-- project's SKILL.md + per-agent skill files. Compared against the
-- current binary's CARGO_PKG_VERSION on startup — if they match, the
-- regen pass is skipped entirely.
--
-- Default NULL for existing rows, which triggers a fresh regen on
-- first post-upgrade launch (correct — we don't know what version
-- last touched them).
ALTER TABLE projects ADD COLUMN skill_regen_version TEXT;
