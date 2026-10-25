-- 0222 — live dual-run of one database (prd-app-pg-choice-dualrun-v1 §8,
-- PA42; 0.46.4). One additive table: a run lives for days (setting up,
-- syncing, streaming), which `sql_moves` can't hold (its `state` has no
-- long-lived value, and boot recovery finishes or undoes every open move
-- within 30 minutes). The cut-over itself is an ordinary `sql_moves` row
-- with `method = 'dual'` (already allowed by 0201), named by `move_id`.
--
-- One open run per database (the partial unique index). `objects_json`
-- names what the run made (slot, publication, subscription, role, freeze,
-- the FORCE RLS oids kept off on the target); `lag_json` is the monitor's
-- last sample. Never a password. `needs_helper`: the run ended but the
-- root helper couldn't drop its slot and DDL freeze yet; it stays open
-- (so K2 keeps refusing `db_dual_running`) until the teardown runs.
CREATE TABLE IF NOT EXISTS sql_dual_runs (
    id             TEXT PRIMARY KEY NOT NULL,
    database_id    TEXT NOT NULL,
    db             TEXT NOT NULL,
    from_major     INTEGER NOT NULL,
    to_major       INTEGER NOT NULL,
    state          TEXT NOT NULL DEFAULT 'setting_up'
                   CHECK (state IN ('setting_up','syncing','streaming','stalled',
                                    'switching','switched','aborted','failed',
                                    'needs_helper')),
    objects_json   TEXT,
    replica_identity_full_json TEXT,
    lag_json       TEXT,
    move_id        TEXT,
    actor          TEXT,
    started_at     INTEGER NOT NULL,
    streaming_at   INTEGER,
    finished_at    INTEGER,
    last_error     TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_sql_dual_runs_open
    ON sql_dual_runs (database_id) WHERE state NOT IN ('switched','aborted','failed');
