-- 0181: agent-proposed clones, held for a person
-- (prd-clone-carries-threads-and-vault-v1 §5.2 / §4.4, CL21; the PRD lists
-- the table in 0180 with `overlay_imports`, which the Threads slice owns —
-- this lane takes 0181 so the two land independently; the integrator may
-- renumber on slips, never below a shipped max).
--
-- An agent prepares a clone of its OWN workspace (`k2 clone propose`); a
-- person (Owner/Admin, with a fresh vault confirm `clone-approve` bound to
-- the id) approves it and runs it under their own credential and clone
-- gate. `options_json` holds names and choices only (threads, sinceDays,
-- per-secret NAME -> copy|blank|skip, dest), never a value. Open proposals
-- expire after 7 days. `IF NOT EXISTS`, so a same-shaped table created by
-- another lane is left alone.
CREATE TABLE IF NOT EXISTS clone_proposals (
  id            TEXT PRIMARY KEY,
  project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  proposed_by   TEXT NOT NULL,
  target        TEXT NOT NULL,
  options_json  TEXT NOT NULL,
  why           TEXT,
  status        TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','approved','denied','withdrawn','expired','done','failed')),
  decided_by    TEXT,
  decided_at    INTEGER,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
  expires_at    INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_clone_proposals_open ON clone_proposals(project_id, status);
