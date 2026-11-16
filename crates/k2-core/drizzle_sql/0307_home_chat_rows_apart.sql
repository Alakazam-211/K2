-- 0307: people and channel rows on Home move out of `homes_items` into
-- their own table (0.46.13), so a server rolled back to 0.46.12 never sees
-- them. 0.46.12 reads every `homes_items` row of a person and takes any
-- kind that isn't `row` for a Home: a `person` or `channel` row (0306)
-- came back to its clients as an extra Home carrying the group's own id,
-- with no name and the row's order, and the group lost its name or moved.
--
-- `homes_chat_items` has the columns of `homes_items` (0193 + 0305) and
-- takes only the two new row kinds; Homes and agent rows stay where every
-- release reads them. The person's cursor (`homes_people.seq`) is shared,
-- so one pull still answers both tables in seq order. Checked in k2-core
-- (`crate::homes_sync`), like the rest of the item.
--
-- The rows a queue build already wrote into `homes_items` move here, every
-- field kept. Safe to run again (IF NOT EXISTS, OR IGNORE).
CREATE TABLE IF NOT EXISTS homes_chat_items (
  person      TEXT    NOT NULL,
  kind        TEXT    NOT NULL CHECK (kind IN ('person', 'channel')),
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
CREATE INDEX IF NOT EXISTS homes_chat_items_person_seq ON homes_chat_items (person, seq);
--> statement-breakpoint
INSERT OR IGNORE INTO homes_chat_items (person, kind, home_id, address, name, label, workspace_id, ord, updated_at,
                                        device_id, deleted, merged_into, seq, icon, recent_first, pin_order,
                                        last_messaged_at)
  SELECT person, kind, home_id, address, name, label, workspace_id, ord, updated_at, device_id,
         deleted, merged_into, seq, icon, recent_first, pin_order, last_messaged_at
  FROM homes_items WHERE kind IN ('person', 'channel');
--> statement-breakpoint
DELETE FROM homes_items WHERE kind IN ('person', 'channel');
