-- 0219: Garden access, one list per Garden (prd-garden-access-v1 rev 4,
-- slice A1). Rides 0.46.3 behind the default-off `gardenAccess` switch if
-- it is green in time, else 0.46.4; it runs with the switch off.
--
-- Additive only. With the switch off (the default in every build) nothing
-- reads or writes these tables and they stay empty: the G4 pass that first
-- fills them runs only on an off→on flip of the switch.
--
-- Rev 4 (Rosson 2026-10-22): no per-Garden sight limits, so rev 2's
-- `zen_garden_sight` and `zen_widget_legacy_reach` are not created.
--
-- * `zen_garden_access` — the list: which workspaces (agents) may edit a
--   Garden. `project_id` is `projects.id`. The `project_id` index serves
--   "on at least one Garden's list" (rev 3 Q6) and the G7 cleanup.
-- * `zen_garden_made` — which agent made a Garden ("made by", the first
--   card's words, the 5-a-day budget). Grants nothing.
-- * `zen_garden_access_asks` — K2's own cards' asks store, per (Garden,
--   agent, kind): `access` (the garden.access card) or `delete` (the
--   garden.delete card). The last ask, the last "Not now" (24 h quiet) and
--   the waiting card.
-- * `zen_garden_events` — History (Settings → Gardens, rev 5): who, when,
--   what (`kind`) and the detail (file, short hash, snapshot, card id,
--   route, old/new names). `garden_id` is the Garden (`'*'` for the
--   computer-wide look), `widget` the widget folder; a widget write logs one
--   row with `widget` set and `garden_id` NULL. Kept when the Garden or the
--   workspace is deleted; kept 2 years, pruned by the daemon's daily sweep
--   (the `at` index), with no count cap.
CREATE TABLE IF NOT EXISTS zen_garden_access (
  garden_id   TEXT NOT NULL CHECK (length(garden_id) BETWEEN 1 AND 64),
  project_id  TEXT NOT NULL CHECK (length(project_id) >= 1),
  added_by    TEXT NOT NULL CHECK (added_by IN ('owner', 'card', 'migration')),
  added_at    INTEGER NOT NULL,
  card_id     TEXT,
  PRIMARY KEY (garden_id, project_id)
) WITHOUT ROWID;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS zen_garden_access_by_project ON zen_garden_access(project_id);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS zen_garden_made (
  garden_id        TEXT PRIMARY KEY CHECK (length(garden_id) BETWEEN 1 AND 64),
  made_by_project  TEXT NOT NULL,
  made_at          INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS zen_garden_made_by_project ON zen_garden_made(made_by_project, made_at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS zen_garden_access_asks (
  garden_id        TEXT NOT NULL,
  project_id       TEXT NOT NULL,
  kind             TEXT NOT NULL DEFAULT 'access' CHECK (kind IN ('access', 'delete')),
  last_ask_at      INTEGER,
  last_not_now_at  INTEGER,
  card_id          TEXT,
  PRIMARY KEY (garden_id, project_id, kind)
) WITHOUT ROWID;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS zen_garden_access_asks_by_project ON zen_garden_access_asks(project_id);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS zen_garden_events (
  id           INTEGER PRIMARY KEY,
  garden_id    TEXT,
  widget       TEXT,
  at           INTEGER NOT NULL,
  actor        TEXT NOT NULL CHECK (length(actor) BETWEEN 1 AND 160),
  kind         TEXT NOT NULL CHECK (kind IN ('made', 'changed', 'widget_changed', 'renamed', 'reordered', 'theme',
                 'reset', 'template', 'locked', 'unlocked', 'sync', 'access_added', 'access_removed', 'access_asked',
                 'access_not_now', 'delete_asked', 'delete_kept', 'deleted', 'restored', 'disk_edit',
                 'lock_put_back', 'signed', 'imported')),
  detail_json  TEXT NOT NULL CHECK (length(detail_json) <= 2048),
  CHECK (garden_id IS NOT NULL OR widget IS NOT NULL)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS zen_garden_events_by_garden ON zen_garden_events(garden_id, at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS zen_garden_events_by_widget ON zen_garden_events(widget, at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS zen_garden_events_by_at ON zen_garden_events(at);
