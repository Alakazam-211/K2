-- 0193: Homes sync between a person's devices (homes-sync-v1, 0.46.1;
-- research-homes-sync-v1, Rosson's rulings on ticket 4f537560).
--
-- One Homes set per PERSON on this server (the owner token and every login
-- the owner linked are the person `owner`; any other login is its own
-- person). Every device keeps its own working copy and syncs with this
-- server when it can; the server is only the sync peer.
--
-- `homes_items` — one row per Home and one per Home row. Last writer wins
-- per item on (`updated_at`, `device_id`); a delete is a tombstone
-- (`deleted` = 1) so a stale device can't bring it back. A Home merged
-- into another on a first sync keeps its id as a tombstone with
-- `merged_into` (a redirect for selections and Garden `seedHome`).
-- `seq` is the person's cursor value when the item last changed.
-- Home: kind 'home', `home_id` = its id, `address` = ''.
-- Row:  kind 'row',  `home_id` = its Home, `address` = 'handle::<serverId>'.
CREATE TABLE IF NOT EXISTS homes_items (
  person      TEXT    NOT NULL,
  kind        TEXT    NOT NULL CHECK (kind IN ('home', 'row')),
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
  PRIMARY KEY (person, kind, home_id, address)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS homes_items_person_seq ON homes_items (person, seq);
--> statement-breakpoint

-- `homes_people` — the person's cursor (`seq`, bumped on every change) and
-- the tombstone floor: a pull with `since` below it gets the full set,
-- because tombstones older than the floor were collected.
CREATE TABLE IF NOT EXISTS homes_people (
  person TEXT PRIMARY KEY,
  seq    INTEGER NOT NULL DEFAULT 0,
  floor  INTEGER NOT NULL DEFAULT 0
);
--> statement-breakpoint

-- `homes_devices` — the devices syncing a person's Homes from this server
-- (Server settings lists and removes them). A removed device keeps its row
-- with `removed_at`; its pulls and pushes answer 410 until it registers
-- again.
CREATE TABLE IF NOT EXISTS homes_devices (
  person        TEXT    NOT NULL,
  device_id     TEXT    NOT NULL CHECK (length(device_id) BETWEEN 1 AND 128),
  name          TEXT    NOT NULL,
  platform      TEXT    NOT NULL,
  registered_at INTEGER NOT NULL,
  last_sync_at  INTEGER,
  removed_at    INTEGER,
  PRIMARY KEY (person, device_id)
);
