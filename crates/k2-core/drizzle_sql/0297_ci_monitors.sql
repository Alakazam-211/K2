-- 0297: CI monitors (prd-compute-blacksmith-class-v1 §4.5.4, slice O1).
-- Renumber freely on a slip (and edit the migration-order tests). 0296 is
-- reserved for O2's job stats series; the trusted-branches column the PRD
-- first put here moved to B2's own migration (0300). The store is
-- `crate::ci_monitors`; the reader is the CI engine's run end (k2-daemon
-- `ci_engine::finish`).
--
-- Monitors are daemon state, set by CLI (`k2 ci monitor add`), not
-- `.k2/ci.toml`: they say how this server wants to hear about a repo, not
-- what the repo is. One row per monitor, per workspace.
--
-- `kind`: `fail-streak` (`branch`, `count` runs in a row failed), `slow`
-- (`check`: the p50 of its last 5 runs over `over_milli`/1000 × its 14-day
-- p50), `queue-wait` (a check job waited more than `over_secs` to start),
-- `new-flaky` (a test turned flaky; `check` optional).
-- `to_kind`/`to_target`: `workspace` + a workspace id (its live session,
-- else its inbox), or `thread` + a Thread address.
-- `muted_until`: no message before then. `last_fired_at`: the cooldown's
-- clock. `last_key`: what it last fired about (a run id), so one thing
-- fires once.
CREATE TABLE IF NOT EXISTS ci_monitors (
  workspace_id   TEXT NOT NULL,
  name           TEXT NOT NULL,
  kind           TEXT NOT NULL CHECK (kind IN ('fail-streak','slow','queue-wait','new-flaky')),
  check_name     TEXT,
  branch         TEXT,
  count          INTEGER,
  over_milli     INTEGER,
  over_secs      INTEGER,
  cooldown_secs  INTEGER NOT NULL DEFAULT 21600,
  to_kind        TEXT NOT NULL DEFAULT 'workspace' CHECK (to_kind IN ('workspace','thread')),
  to_target      TEXT NOT NULL,
  muted_until    INTEGER,
  last_fired_at  INTEGER,
  last_key       TEXT,
  fired          INTEGER NOT NULL DEFAULT 0,
  created_by     TEXT NOT NULL DEFAULT '',
  created_at     INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, name)
);
