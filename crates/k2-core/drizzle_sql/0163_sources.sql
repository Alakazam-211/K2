-- 0163_sources — Garden v3 data sources and triggers (prd-garden-v3-external-data-cards-v1).
-- Times: INTEGER seconds since the Unix epoch (UTC). Durations: INTEGER seconds.

-- One global change counter for `sources_changed {rev}` and `GET /cli/sources/changes`.
CREATE TABLE IF NOT EXISTS source_rev (
  id   INTEGER PRIMARY KEY CHECK (id = 1),
  rev  INTEGER NOT NULL,
  reaped_rev INTEGER NOT NULL DEFAULT 0   -- highest rev of a reaped tombstone; changes?since older than it answer full
);
--> statement-breakpoint
INSERT OR IGNORE INTO source_rev (id, rev) VALUES (1, 0);
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS sources (
  id                 TEXT PRIMARY KEY,
  name               TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 40 AND name NOT GLOB '*[^a-z0-9-]*'),
  label              TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 80),
  kind               TEXT NOT NULL DEFAULT 'http-json' CHECK (kind IN ('http-json')),
  preset             TEXT CHECK (preset IS NULL OR length(preset) <= 40),
  spec_json          TEXT NOT NULL,                 -- FetchSpecJson + Extract + Rules (§3.2), canonical JSON
  spec_hash          TEXT NOT NULL CHECK (length(spec_hash) = 64),  -- sha256 hex, §3.2 SourceSpec::hash
  origins_json       TEXT NOT NULL,                 -- JSON array of "scheme://host:port", sorted, unique
  lan                INTEGER NOT NULL DEFAULT 0 CHECK (lan IN (0, 1)),   -- an exact IP or http:// target; Owner only
  secret_name        TEXT CHECK (secret_name IS NULL OR length(secret_name) <= 128),
  url_secret_name    TEXT CHECK (url_secret_name IS NULL OR length(url_secret_name) <= 128),
  secret_bound       INTEGER NOT NULL DEFAULT 0 CHECK (secret_bound IN (0, 1)),  -- a vault grant exists now
  every_secs         INTEGER NOT NULL CHECK (every_secs BETWEEN 30 AND 86400),
  timeout_secs       INTEGER NOT NULL DEFAULT 10 CHECK (timeout_secs BETWEEN 1 AND 30),
  max_bytes          INTEGER NOT NULL DEFAULT 1048576 CHECK (max_bytes BETWEEN 1024 AND 5242880),
  fail_after         INTEGER NOT NULL DEFAULT 3 CHECK (fail_after BETWEEN 1 AND 10),
  on_target_error    TEXT NOT NULL DEFAULT 'crit' CHECK (on_target_error IN ('crit', 'unknown')),
  link               TEXT CHECK (link IS NULL OR length(link) <= 500),
  viewers            TEXT NOT NULL CHECK (viewers IN ('member', 'admin')),
  agent_access       TEXT NOT NULL CHECK (agent_access IN ('values', 'shape')),
  keep               TEXT NOT NULL DEFAULT 'values' CHECK (keep IN ('values', 'states')),
  pinned             INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
  pinned_by          TEXT,
  pinned_at          INTEGER,
  status             TEXT NOT NULL CHECK (status IN ('proposed', 'active', 'paused', 'removed')),
  pause_reason       TEXT CHECK (pause_reason IS NULL OR pause_reason IN
                       ('paused_by_person', 'secret_missing', 'secret_disabled', 'vault_unavailable')),
  made_by            TEXT NOT NULL,                 -- 'owner' | 'login:<user>' | 'agent:<projectId>'
  made_by_project    TEXT,                          -- set iff made_by starts with 'agent:'
  approved_by        TEXT,
  approved_at        INTEGER,
  approved_spec_hash TEXT,
  simulate_state     TEXT CHECK (simulate_state IS NULL OR simulate_state IN ('ok', 'warn', 'crit', 'stale', 'unknown', 'slow')),
  simulate_until     INTEGER,
  simulate_by        TEXT,
  mute_until         INTEGER,
  mute_by            TEXT,
  mute_reason        TEXT CHECK (mute_reason IS NULL OR length(mute_reason) <= 200),
  rev                INTEGER NOT NULL,
  created_at         INTEGER NOT NULL,
  updated_at         INTEGER NOT NULL,
  removed_at         INTEGER,
  CHECK ((status = 'removed') = (removed_at IS NOT NULL)),
  CHECK ((simulate_state IS NULL) = (simulate_until IS NULL)),
  CHECK ((mute_until IS NULL) = (mute_by IS NULL)),
  CHECK ((made_by LIKE 'agent:%') = (made_by_project IS NOT NULL)),
  CHECK (secret_bound = 0 OR secret_name IS NOT NULL OR url_secret_name IS NOT NULL)
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS sources_live_name ON sources(name) WHERE status != 'removed';
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS sources_removed_name ON sources(name, removed_at) WHERE status = 'removed';
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS sources_rev ON sources(rev);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS sources_made_by_project ON sources(made_by_project) WHERE made_by_project IS NOT NULL;
--> statement-breakpoint

-- Latest result, one row per source (created with the source, phase 'pending').
CREATE TABLE IF NOT EXISTS source_state (
  source_id         TEXT PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
  health            TEXT NOT NULL DEFAULT 'unknown' CHECK (health IN ('ok', 'warn', 'crit', 'unknown')),
  phase             TEXT NOT NULL DEFAULT 'pending' CHECK (phase IN ('pending', 'live', 'stale', 'paused')),
  reason            TEXT,                          -- Reason::code() (§3.3) or NULL
  http_status       INTEGER,
  soft_health       TEXT CHECK (soft_health IS NULL OR soft_health IN ('ok', 'warn', 'crit', 'unknown')),
  soft_count        INTEGER NOT NULL DEFAULT 0,    -- source-level (single-url and fields/aggregate rules)
  self_health       TEXT CHECK (self_health IS NULL OR self_health IN ('ok', 'warn', 'crit', 'unknown')),  -- hard source-level reading; NULL until rated
  fetch_failures    INTEGER NOT NULL DEFAULT 0,    -- consecutive cycles with no success on any target
  backoff_k         INTEGER NOT NULL DEFAULT 0,
  hard_since        INTEGER,
  recovered_at      INTEGER,
  updated_at        INTEGER,                       -- last success on any target
  checked_at        INTEGER,                       -- last attempt
  next_at           INTEGER,                       -- display only; the scheduler uses Instant
  gap_kind          TEXT CHECK (gap_kind IS NULL OR gap_kind IN ('host_slept', 'daemon_restarted')),
  gap_from          INTEGER,
  gap_to            INTEGER,
  incident_id       TEXT,                          -- open source-level incident, or NULL
  projection_json   TEXT CHECK (projection_json IS NULL OR length(projection_json) <= 65536),
  counts_json       TEXT NOT NULL DEFAULT '{"ok":0,"warn":0,"crit":0,"unknown":0}',
  etag              TEXT,                          -- single-url sources only
  last_modified     TEXT,
  vault_denied      INTEGER NOT NULL DEFAULT 0 CHECK (vault_denied IN (0, 1)),   -- parked after a denial [GV68]
  rev               INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS source_state_rev ON source_state(rev);
--> statement-breakpoint

-- One row per item: fan-out targets (is_target = 1) and pointer items (is_target = 0). <= 200 per source.
CREATE TABLE IF NOT EXISTS source_items (
  source_id      TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  item_key       TEXT NOT NULL CHECK (length(item_key) BETWEEN 1 AND 200),
  is_target      INTEGER NOT NULL DEFAULT 0 CHECK (is_target IN (0, 1)),
  origin         TEXT,                             -- fan-out target's origin
  health         TEXT NOT NULL DEFAULT 'unknown' CHECK (health IN ('ok', 'warn', 'crit', 'unknown')),
  soft_health    TEXT CHECK (soft_health IS NULL OR soft_health IN ('ok', 'warn', 'crit', 'unknown')),
  soft_count     INTEGER NOT NULL DEFAULT 0,
  reason         TEXT,
  http_status    INTEGER,
  hard_since     INTEGER,
  recovered_at   INTEGER,
  updated_at     INTEGER,
  checked_at     INTEGER,
  etag           TEXT,
  last_modified  TEXT,
  incident_id    TEXT,
  PRIMARY KEY (source_id, item_key)
) WITHOUT ROWID;
--> statement-breakpoint

-- Trend history. v NULL = gap marker. res 'raw' kept 48 h, 'hour' kept 35 days.
CREATE TABLE IF NOT EXISTS source_points (
  source_id  TEXT NOT NULL,
  series     TEXT NOT NULL CHECK (length(series) <= 40),
  res        TEXT NOT NULL CHECK (res IN ('raw', 'hour')),
  t          INTEGER NOT NULL,
  v          REAL,
  PRIMARY KEY (source_id, series, res, t)
) WITHOUT ROWID;
--> statement-breakpoint

-- Transitions; pruned to the newest 200 per source.
CREATE TABLE IF NOT EXISTS source_events (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  source_id    TEXT NOT NULL,
  at           INTEGER NOT NULL,
  scope        TEXT NOT NULL CHECK (scope IN ('source', 'item')),
  item_key     TEXT,
  kind         TEXT NOT NULL CHECK (kind IN
                 ('went_ok', 'went_warn', 'went_crit', 'went_unknown', 'recovered',
                  'went_stale', 'went_live', 'paused', 'resumed', 'gap')),
  from_health  TEXT,
  to_health    TEXT,
  reason       TEXT,
  incident_id  TEXT,
  simulated    INTEGER NOT NULL DEFAULT 0 CHECK (simulated IN (0, 1)),
  CHECK ((scope = 'item') = (item_key IS NOT NULL))
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS source_events_by_source ON source_events(source_id, id);
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS source_triggers (
  id                  TEXT PRIMARY KEY,
  source_id           TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  on_kind             TEXT NOT NULL CHECK (on_kind IN ('went_crit', 'went_warn', 'recovered', 'stale_for', 'item_went_crit')),
  stale_for_secs      INTEGER CHECK (stale_for_secs IS NULL OR stale_for_secs BETWEEN 300 AND 604800),
  target_project_id   TEXT NOT NULL,                -- projects.id [GV62]
  note                TEXT CHECK (note IS NULL OR length(note) <= 500),
  cooldown_secs       INTEGER NOT NULL DEFAULT 1800 CHECK (cooldown_secs BETWEEN 300 AND 86400),
  daily_cap           INTEGER NOT NULL DEFAULT 12 CHECK (daily_cap BETWEEN 1 AND 48),
  status              TEXT NOT NULL CHECK (status IN ('proposed', 'active', 'paused', 'undeliverable')),
  made_by             TEXT NOT NULL,
  made_by_project     TEXT,
  approved_by         TEXT,
  approved_at         INTEGER,
  last_fire_at        INTEGER,
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL,
  CHECK ((on_kind = 'stale_for') = (stale_for_secs IS NOT NULL)),
  CHECK ((made_by LIKE 'agent:%') = (made_by_project IS NOT NULL))
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS source_triggers_by_source ON source_triggers(source_id);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS source_triggers_by_project ON source_triggers(target_project_id);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS source_triggers_one_per_kind
  ON source_triggers(source_id, on_kind, target_project_id, IFNULL(stale_for_secs, 0));
--> statement-breakpoint

-- Fire/suppress log, kept 30 days. 'pending' is written before delivery [GV63].
CREATE TABLE IF NOT EXISTS source_trigger_fires (
  id                 TEXT PRIMARY KEY,
  trigger_id         TEXT NOT NULL,
  source_id          TEXT NOT NULL,
  target_project_id  TEXT NOT NULL,
  incident_id        TEXT NOT NULL,
  item_key           TEXT,
  at                 INTEGER NOT NULL,
  outcome            TEXT NOT NULL CHECK (outcome IN
                       ('pending', 'delivered', 'inbox', 'undeliverable', 'suppressed', 'coalesced', 'test')),
  suppress_reason    TEXT CHECK (suppress_reason IS NULL OR suppress_reason IN
                       ('cooldown', 'daily_cap', 'agent_cap', 'muted', 'simulated', 'paused',
                        'gap', 'deduped', 'draining')),
  coalesced_into     TEXT,                          -- fire id that carried this one
  delivered_at       INTEGER,
  session_id         TEXT,
  woke               INTEGER CHECK (woke IS NULL OR woke IN (0, 1)),
  note_bytes         INTEGER,
  detail             TEXT,                          -- a fixed code (MsgReason / inbox error kind), never note text
  CHECK ((outcome = 'suppressed') = (suppress_reason IS NOT NULL)),
  CHECK ((outcome = 'coalesced') = (coalesced_into IS NOT NULL))
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS source_fires_by_trigger ON source_trigger_fires(trigger_id, at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS source_fires_by_project ON source_trigger_fires(target_project_id, at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS source_fires_pending ON source_trigger_fires(at) WHERE outcome = 'pending';
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS source_fires_once_per_incident
  ON source_trigger_fires(trigger_id, incident_id, IFNULL(item_key, ''))
  WHERE outcome IN ('pending', 'delivered', 'inbox', 'coalesced');
--> statement-breakpoint

-- Audit for sources and triggers (kept 90 days), plus the hourly poller rollup [GV68].
CREATE TABLE IF NOT EXISTS source_audit (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  at           INTEGER NOT NULL,
  actor        TEXT NOT NULL,                       -- 'owner' | 'login:<u>' | 'agent:<pid>' | 'shell:<pid>' | 'daemon'
  actor_kind   TEXT NOT NULL CHECK (actor_kind IN ('owner', 'login', 'agent', 'shell', 'daemon')),
  action       TEXT NOT NULL,                       -- §4.6 list
  source_id    TEXT,
  trigger_id   TEXT,
  detail_json  TEXT NOT NULL DEFAULT '{}'           -- names, codes, counts; never values, URLs with queries or secrets
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS source_audit_at ON source_audit(at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS source_audit_source ON source_audit(source_id, at);
