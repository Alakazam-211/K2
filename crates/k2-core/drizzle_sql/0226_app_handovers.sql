-- 0226 — App handover (prd-app-handoff-v1 rev 2; 0.46.5). One App's
-- maintenance moves from one workspace (agent) to another on the same box.
-- Releases the never-created reservation 0199 (`app_transfers`).
--
-- `app_handovers`: one row per handover. `plan_json` / `plan_hash` are the
-- plan the card showed (the approval recomputes it and refuses a
-- different hash: `stale`). `journal_json` holds the k2.db before-values
-- (label rows, room lists, secret scopes, the database row) until the undo
-- window ends; never a credential (retired password hashes live in
-- skin.db's `app_handover_journal`, deleted at window end).
-- `steps_done` drives boot recovery (§4.8): a row `applied` with
-- `steps_done < 6` is finished forward before the re-host wave.
--
-- `app_ref_aliases`: the immutable Seed's `k2v://ws/<from>/NAME` refs
-- resolve as `ws/<to>/<to_name>` for this service only (§4.5). Removed on
-- undo.
--
-- Additive only: a K2 without this migration ignores both tables.
CREATE TABLE IF NOT EXISTS app_handovers (
    id               TEXT PRIMARY KEY NOT NULL,
    app_id           TEXT NOT NULL,
    service_id       TEXT NOT NULL,
    from_workspace   TEXT NOT NULL,
    to_workspace     TEXT NOT NULL,
    state            TEXT NOT NULL CHECK (state IN
                     ('offered','accepted','held','applying','applied','final',
                      'undone','declined','cancelled','denied','stale','expired','aborted')),
    started_by       TEXT NOT NULL,
    accepted_by      TEXT,
    accepted_at      INTEGER,
    request_id       TEXT,
    approved_by      TEXT,
    approved_at      INTEGER,
    plan_json        TEXT NOT NULL,
    plan_hash        TEXT NOT NULL,
    options_json     TEXT NOT NULL DEFAULT '{}',
    brief            TEXT,
    steps_done       INTEGER NOT NULL DEFAULT 0,
    rekey_state      TEXT,
    journal_json     TEXT,
    window_until     INTEGER,
    reason           TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS app_handovers_one_open ON app_handovers(app_id)
    WHERE state IN ('offered','accepted','held','applying');
CREATE INDEX IF NOT EXISTS app_handovers_ws ON app_handovers(from_workspace, to_workspace);

CREATE TABLE IF NOT EXISTS app_ref_aliases (
    service_id      TEXT NOT NULL,
    from_workspace  TEXT NOT NULL,
    to_workspace    TEXT NOT NULL,
    name            TEXT NOT NULL,
    to_name         TEXT NOT NULL,
    handover_id     TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (service_id, from_workspace, name)
);
