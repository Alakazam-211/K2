-- 0256_mail_forwards — hosted mail forwarding to outside addresses
-- (prd-hostmail-external-forwarding-v1 §3.2, §3.6, §3.7; vs-live FW-A5,
-- FW-A6, FW-A8; 0.46.7).
--
-- `mail_forwards` is the truth for every hosted-mail forward (same-box and
-- outside). K2 writes the mailbox's Sieve `k2` script FROM these rows,
-- never the other way; a reconcile restores a script that drifted. One
-- live (not `removed`) row per mailbox. States:
--   pending_verify  an outside destination hasn't clicked its confirm link
--                   (`email_verified_addresses`, purpose `forward`); nothing
--                   is forwarded yet
--   active          the Sieve forward block is written
--   paused          keep-only (no redirect); `pause_reason` says why
--   removed         gone (kept for the audit list)
-- `mail_forward_events` is the audit list (kept 1 year): ids and counts
-- only, never message content. `mail_forward_queue` ties the mail server's
-- queue id (webhook `queueId`, decimal text) to a forward (kept 7 days).
-- `mail_forward_outcomes` counts what happened to each forwarded message
-- (kept 30 days) for the caps, the auto-pause and the box brake.
-- `mail_addresses.person_principal` is the box Login (person key) that owns
-- a person mailbox, set by the owner or an Admin; person self-service works
-- only when it's set.
-- New tables + one ADD COLUMN: renumber freely on a slip (and edit the
-- migration-order tests). The store is `crate::mail_forward`.
CREATE TABLE IF NOT EXISTS mail_forwards (
    id                   TEXT PRIMARY KEY NOT NULL,
    address_id           TEXT NOT NULL,
    address              TEXT NOT NULL,
    dest                 TEXT NOT NULL,
    dest_kind            TEXT NOT NULL CHECK (dest_kind IN ('internal', 'external')),
    verified_address_id  TEXT,
    keep                 INTEGER NOT NULL DEFAULT 1 CHECK (keep IN (0, 1)),
    state                TEXT NOT NULL CHECK (state IN ('pending_verify', 'active', 'paused', 'removed')),
    set_by               TEXT NOT NULL,
    pause_reason         TEXT,
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL,
    activated_at         INTEGER,
    paused_at            INTEGER,
    removed_at           INTEGER,
    CHECK ((state = 'paused') = (pause_reason IS NOT NULL)),
    CHECK ((state = 'removed') = (removed_at IS NOT NULL)),
    CHECK (dest_kind = 'internal' OR state <> 'active' OR verified_address_id IS NOT NULL)
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS mail_forwards_live ON mail_forwards (address_id) WHERE state <> 'removed';
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forwards_state ON mail_forwards (state, dest_kind);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forwards_verified ON mail_forwards (verified_address_id);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS mail_forward_events (
    id           TEXT PRIMARY KEY NOT NULL,
    forward_id   TEXT NOT NULL,
    event        TEXT NOT NULL,
    actor        TEXT NOT NULL,
    detail_json  TEXT NOT NULL DEFAULT '{}',
    at           INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forward_events_forward ON mail_forward_events (forward_id, at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forward_events_at ON mail_forward_events (at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS mail_forward_queue (
    queue_id    TEXT PRIMARY KEY NOT NULL,
    forward_id  TEXT NOT NULL,
    at          INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forward_queue_at ON mail_forward_queue (at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS mail_forward_outcomes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    forward_id  TEXT NOT NULL,
    queue_id    TEXT,
    kind        TEXT NOT NULL CHECK (kind IN ('queued', 'delivered', 'refused_user', 'refused_policy', 'failed')),
    code        TEXT,
    at          INTEGER NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS mail_forward_outcomes_once ON mail_forward_outcomes (queue_id, kind) WHERE queue_id IS NOT NULL;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forward_outcomes_forward ON mail_forward_outcomes (forward_id, at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forward_outcomes_at ON mail_forward_outcomes (at);
--> statement-breakpoint
ALTER TABLE mail_addresses ADD COLUMN person_principal TEXT;
