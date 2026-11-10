-- 0290: the setup checklist (prd-setup-checklist-v1, contract
-- setup-checklist-contract.md). Every row's done state comes from real
-- daemon state; the only things kept per person are "hidden", the rows
-- they skipped and "finished". The person key is
-- `People::person_key` (`owner` for the owner token and the logins the
-- owner linked, else the lowercased login). Times are unix seconds.
-- The store is `crate::first_run`.
CREATE TABLE IF NOT EXISTS first_run_person (
    person_key TEXT PRIMARY KEY NOT NULL,
    hidden_at INTEGER,
    finished_at INTEGER,
    skipped_json TEXT NOT NULL DEFAULT '[]'
);
--> statement-breakpoint
-- One row at most. `existing_cutoff` is set only on a server that was
-- already in use when this migration ran (K2 Q-A): a person with no
-- `first_run_person` row counts as finished when they are the owner or a
-- login made before the cutoff. A fresh install has no row, so everyone
-- starts unfinished.
CREATE TABLE IF NOT EXISTS first_run_meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    existing_cutoff INTEGER
);
--> statement-breakpoint
INSERT OR IGNORE INTO first_run_meta (id, existing_cutoff)
SELECT 1, unixepoch()
WHERE EXISTS (SELECT 1 FROM projects WHERE id NOT IN ('_orphan', '_broadcast'))
   OR EXISTS (SELECT 1 FROM llm_accounts);
