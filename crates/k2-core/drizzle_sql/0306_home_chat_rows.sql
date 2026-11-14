-- 0306: people and channels on Home (prd-home-people-channels-v1 §5.2,
-- HP-9; 0.46.13). The synced Homes' items gain two row kinds beside the
-- agent `row`:
--
--   `person`   `address` = `<external id>::<serverId>`, the person's K2
--              external id on that server (`k2:owner` or
--              `k2:connect:<uuid>`). Opens the DM with them there.
--   `channel`  `address` = `<Sew channel id>::<serverId>`. Opens the
--              channel there.
--
-- Both carry the row fields (`label`, `ord`, `pin_order`,
-- `last_messaged_at`); pins count across every row kind of a Home.
--
-- SQLite can't change a CHECK, so `homes_items` is rebuilt: copied, dropped
-- and renamed, with its index made again. Every column and value is kept.
-- Checked in k2-core (`crate::homes_sync`), like the rest of the item.
CREATE TABLE homes_items_0306 (
  person      TEXT    NOT NULL,
  kind        TEXT    NOT NULL CHECK (kind IN ('home', 'row', 'person', 'channel')),
  home_id     TEXT    NOT NULL CHECK (length(home_id) BETWEEN 1 AND 128),
  address     TEXT    NOT NULL DEFAULT '' CHECK (length(address) <= 300),
  name        TEXT,
  label       TEXT,
  workspace_id TEXT,
  ord         TEXT    NOT NULL DEFAULT '',
  updated_at  INTEGER NOT NULL,
  device_id   TEXT    NOT NULL,
  deleted     INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
  merged_into TEXT,
  seq         INTEGER NOT NULL,
  icon        TEXT,
  recent_first INTEGER NOT NULL DEFAULT 0,
  pin_order   TEXT,
  last_messaged_at INTEGER,
  PRIMARY KEY (person, kind, home_id, address)
);
--> statement-breakpoint
INSERT INTO homes_items_0306 (person, kind, home_id, address, name, label, workspace_id, ord, updated_at, device_id,
                              deleted, merged_into, seq, icon, recent_first, pin_order, last_messaged_at)
  SELECT person, kind, home_id, address, name, label, workspace_id, ord, updated_at, device_id,
         deleted, merged_into, seq, icon, recent_first, pin_order, last_messaged_at
  FROM homes_items;
--> statement-breakpoint
DROP TABLE homes_items;
--> statement-breakpoint
ALTER TABLE homes_items_0306 RENAME TO homes_items;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS homes_items_person_seq ON homes_items (person, seq);
