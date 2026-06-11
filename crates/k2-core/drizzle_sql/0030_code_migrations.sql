-- Tracks one-time code-side migrations (filesystem rewrites, in-memory
-- state transitions, etc.) so they don't re-run on every launch.
--
-- Distinct from _migrations (schema migrations) which tracks these SQL
-- files themselves. code_migrations is for idempotent-but-expensive
-- runtime passes whose only job is to get from state A to state B once —
-- e.g. the 0.26-era "rename pod-member → agent-template across every
-- project's AGENT.md" rewrite.
--
-- Usage from Rust:
--   if !has_code_migration_applied(&conn, "legacy_agent_types_v1") {
--       do_the_rewrite();
--       mark_code_migration_applied(&conn, "legacy_agent_types_v1");
--   }
CREATE TABLE IF NOT EXISTS code_migrations (
    id TEXT PRIMARY KEY,
    applied_at INTEGER NOT NULL DEFAULT (unixepoch()),
    -- Optional free-form notes captured at application time (version,
    -- counts, etc.). Kept on the row for debugging; nothing reads it.
    notes TEXT
);
