-- 0117: last-known nested-subdomain target on the 0074 attribution
-- table. Additive — do not rewrite frozen 0074 SQL. Empty string =
-- unknown (cache overlay fills this when the in-memory map has the
-- label). Existing rows backfill to ''.
ALTER TABLE subdomain_workspaces ADD COLUMN target TEXT NOT NULL DEFAULT '';
