-- 0305: Home groups (0.46.13). The synced Homes (0193, `homes_items`) gain:
--
--   `icon`             Home only: a Lucide icon name (`[a-z0-9-]{1,64}`), or
--                      NULL for none (the client draws a numbered dot).
--   `recent_first`     Home only: 1 = its unpinned rows sort by the last time
--                      the person sent to them, newest first; 0 = manual
--                      order (every existing Home starts on 0).
--   `pin_order`        Row only: a fractional-index key among the Home's
--                      pinned rows, or NULL when the row is not pinned (at
--                      most 9 pinned per Home, checked in k2-core).
--   `last_messaged_at` Row only: epoch ms of the person's last send to that
--                      agent from any device, or NULL.
--
-- A push that leaves one of these fields out (an older client) keeps the
-- stored value; JSON null clears it. Checked in k2-core
-- (`crate::homes_sync`), not SQL, like the rest of the item.
-- 0304 is taken by another branch; renumber only with K2 (and edit the
-- migration-order tests).
ALTER TABLE homes_items ADD COLUMN icon TEXT;
--> statement-breakpoint
ALTER TABLE homes_items ADD COLUMN recent_first INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE homes_items ADD COLUMN pin_order TEXT;
--> statement-breakpoint
ALTER TABLE homes_items ADD COLUMN last_messaged_at INTEGER;
