-- 0257: prompt stash sync between a person's devices (stash-sync-v1,
-- 0.46.7; .k2/prds/prd-stash-sync-v1.md, contract-stash-sync-v1.md).
--
-- One stash per PERSON on this server (the owner token and every login the
-- owner linked are the person `owner`; any other login is its own person),
-- like Homes (0193). Every device keeps its own working copy and syncs with
-- this server when it can; the server is only the sync peer. Devices are
-- the Homes registry (`homes_devices`): there is no stash device table.
--
-- `stash_items` — one row per stash record (the shared v1 record as JSON
-- in `record`; no attachment bytes, ever). Last writer wins per id on
-- (`updated_at`, `updated_by`), a full tie to the tombstone. A delete is a
-- tombstone (`deleted_at` set, text/attachments/quotes cleared), kept 90
-- days. `seq` is the person's cursor value when the record last changed.
CREATE TABLE IF NOT EXISTS stash_items (
  person      TEXT    NOT NULL,
  id          TEXT    NOT NULL CHECK (length(id) = 36),
  record      TEXT    NOT NULL,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  updated_by  TEXT    NOT NULL,
  deleted_at  INTEGER,
  seq         INTEGER NOT NULL,
  PRIMARY KEY (person, id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS stash_items_person_seq ON stash_items (person, seq);
--> statement-breakpoint

-- `stash_people` — the person's cursor (`seq`, bumped once per push that
-- changed something) and the tombstone floor: a pull with `since` below it
-- gets the full live set, because tombstones older than it were collected.
CREATE TABLE IF NOT EXISTS stash_people (
  person TEXT PRIMARY KEY,
  seq    INTEGER NOT NULL DEFAULT 0,
  floor  INTEGER NOT NULL DEFAULT 0
);
