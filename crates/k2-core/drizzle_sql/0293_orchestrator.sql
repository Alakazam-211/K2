-- 0293: K5 at first run, and the Chief of Staff (prd-setup-checklist-v1
-- §10, contract k5-first-run-contract.md). One row at most: the seat K2's
-- own agent sits in. `workspace_id` is K5's workspace, `chief_of_staff_id`
-- its first hire. An id whose `projects` row is gone counts as empty, so
-- deleting K5's workspace clears the seat (the reader checks; nothing is
-- rewritten here). `display_name` is the one home of the name "K5".
-- Changed only by the owner or through the `/cli/first-run/*` routes.
-- The store is `crate::orchestrator`.
CREATE TABLE IF NOT EXISTS orchestrator (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    workspace_id TEXT,
    display_name TEXT NOT NULL DEFAULT 'K5',
    chief_of_staff_id TEXT,
    created_by TEXT,
    created_at INTEGER,
    opt_out INTEGER NOT NULL DEFAULT 0
);
--> statement-breakpoint
-- The "Meet K5" offer, dismissed per person ("Not now"): unix seconds, NULL
-- while it may still show. `show` on the setup list leaves it alone.
ALTER TABLE first_run_person ADD COLUMN offer_dismissed_at INTEGER;
