-- 0300: the branches whose CI checks may save a workspace's shared
-- ("trusted") build cache (prd-compute-blacksmith-class-v1 §4.1.5, B2).
--
-- Server state, never read from `.k2/ci.toml` (that file is read at the
-- commit being checked, so a PR could name its own branch). NULL = the
-- default: the first non-glob branch of a watch a person set, else `main`.
-- An agent may narrow the list (`k2 ci cache trust --remove`); widening
-- (`--add`, `--set`) is a person's. `trusted_by` / `trusted_at` say who
-- changed it last. A workspace with no `ci_workspaces` row gets one (watch
-- off) when the list is first set.
ALTER TABLE ci_workspaces ADD COLUMN trusted_branches_json TEXT;
--> statement-breakpoint
ALTER TABLE ci_workspaces ADD COLUMN trusted_by TEXT;
--> statement-breakpoint
ALTER TABLE ci_workspaces ADD COLUMN trusted_at INTEGER;
