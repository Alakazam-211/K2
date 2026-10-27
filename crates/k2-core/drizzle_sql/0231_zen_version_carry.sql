-- 0231_zen_version_carry — the manifest is part of a widget's version
-- (prd-widget-size-and-shared-libs-v1 vs-live L1, slice S0; 0.46.5).
-- From 0.46.5 a widget's version hash covers its manifest's run fields
-- (caps, reads, sources, libraries, entry, colorScheme) as well as its
-- bundled document, so every widget gets a new hash. The daemon's one-time
-- carry (`k2_core::zen::version_carry`) copies each existing signature
-- (`zen_widget_authors`) from the old hash to the new one, once, at the
-- first boot, and writes what it did here. Additive; no foreign keys.
-- Times: INTEGER seconds since the Unix epoch (UTC).

-- One row: the carry ran (and what it did, as JSON). Its presence is what
-- stops a second run: after it, a saved copy in the old format is refused.
CREATE TABLE IF NOT EXISTS zen_version_carry_runs (
  id       INTEGER PRIMARY KEY CHECK (id = 1),
  ran_at   INTEGER NOT NULL,
  report   TEXT NOT NULL
);

-- One row per version the carry looked at: a folder's live version, its
-- last good copy, or a locked Garden's frozen copy (`place`).
CREATE TABLE IF NOT EXISTS zen_version_carry (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  widget     TEXT NOT NULL CHECK (length(widget) BETWEEN 1 AND 60),
  place      TEXT NOT NULL CHECK (place IN ('live', 'last_good', 'frozen')),
  garden     TEXT,
  old_hash   TEXT NOT NULL CHECK (length(old_hash) = 64),
  new_hash   TEXT NOT NULL CHECK (length(new_hash) = 64),
  carried    INTEGER NOT NULL CHECK (carried IN (0, 1)),
  -- 'owner' | 'agent:<projects.id>' when a signature was carried.
  maker      TEXT,
  -- How the live manifest differs from the one saved with the signed
  -- version (Rosson 2026-10-26: carry anyway, log each difference).
  differs    TEXT,
  at         INTEGER NOT NULL,
  CHECK ((place = 'frozen') = (garden IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS zen_version_carry_by_widget ON zen_version_carry(widget);
