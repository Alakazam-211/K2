-- 0316: read-only calendar links, slice S1 (prd-google-calendar-link-v1
-- §3.2/§3.3, vs-live-2 CV-20). 0316 is calendar S1's; the S2 tracker
-- tables (calendar_tracks, mirror, ledger) take 0317.
--
-- `calendar_link_calendars`  the calendars a `calendar-read` link exposes
--   besides its primary (which is always on). The owner opts each in
--   (`k2 cal calendars <address> --add <id|name>`, Settings). A link stored
--   `calendar` (0.46.1, read-write) ignores this table and keeps every
--   listed calendar readable (GC-23). `calendar_id` is the provider's own
--   id (a Google calendar id, a Graph calendar id); `name` is the display
--   name when it was added, for listings only.
-- `calendar_grant_prefs`     per-workspace privacy on a linked calendar:
--   `omit_description = 1` means that workspace's agents never see event
--   descriptions (the meeting link and meeting id are still read out of
--   them, nothing else). Keyed by project, because the primary workspace
--   has no `mail_inbox_grants` row.
--
-- Both rows go with their inbox: `inbox_id` references
-- `mail_external_inboxes(id)` ON DELETE CASCADE (unlink, and a deleted
-- workspace's links), and the unlink code deletes them too. A grant pref
-- also goes with its workspace. v1 links are always `linked`. No event
-- data is stored here. Safe to run again.
CREATE TABLE IF NOT EXISTS calendar_link_calendars (
  inbox_source TEXT NOT NULL DEFAULT 'linked' CHECK (inbox_source = 'linked'),
  inbox_id     TEXT NOT NULL REFERENCES mail_external_inboxes(id) ON DELETE CASCADE,
  calendar_id  TEXT NOT NULL,
  name         TEXT,
  -- 'user:<login>' | 'owner-token'
  added_by     TEXT NOT NULL,
  added_at     INTEGER NOT NULL,
  PRIMARY KEY (inbox_source, inbox_id, calendar_id)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS calendar_grant_prefs (
  inbox_source     TEXT NOT NULL DEFAULT 'linked' CHECK (inbox_source = 'linked'),
  inbox_id         TEXT NOT NULL REFERENCES mail_external_inboxes(id) ON DELETE CASCADE,
  project_id       TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  omit_description INTEGER NOT NULL DEFAULT 0 CHECK (omit_description IN (0, 1)),
  set_by           TEXT NOT NULL,
  set_at           INTEGER NOT NULL,
  PRIMARY KEY (inbox_source, inbox_id, project_id)
);
