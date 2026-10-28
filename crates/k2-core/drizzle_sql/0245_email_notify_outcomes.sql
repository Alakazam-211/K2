-- 0245_email_notify_outcomes — Email Notifications part 2, slice N6
-- (prd-email-notifications-part2-v1 §4 N6, vs-live EN-A3, EN-A14, EN-A20,
-- EN-A21; 0.46.6).
--
-- * `email_notify_outcomes` — what happened to each recipient of a send:
--   one row per (log row, recipient, kind). The recipient is kept as the
--   SHA-256 of the lower-case address plus its domain (per-provider
--   counts); the plain address stays only in the log row (30 days).
--   `source`: `dsn` (a delivery report in the sender's mailbox), `arf` (a
--   complaint report), `webhook` (the mail server's events, later) or `k2`
--   (K2 decided, e.g. a suppressed recipient never handed off).
-- * `email_suppressions` — addresses K2 won't send to. A hard bounce is
--   box-wide; a complaint or an unsubscribe is per sender. Hash + domain,
--   never the address.
-- * `email_sender_pauses` — an auto-pause of an App's (or agent's) sender
--   (never the system sender). Live while `resumed_at` is NULL; a resume
--   needs the owner's fresh confirm and starts a new counting window.
-- New tables only: renumber freely on a slip (and edit the migration-order
-- tests). The store is `crate::email_outcomes`.
CREATE TABLE IF NOT EXISTS email_notify_outcomes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    log_id        TEXT NOT NULL,
    sender_id     TEXT NOT NULL,
    rcpt_sha256   TEXT NOT NULL,
    rcpt_domain   TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('delivered', 'delayed', 'bounced_hard', 'bounced_soft', 'complained', 'suppressed')),
    code          TEXT,
    source        TEXT NOT NULL CHECK (source IN ('dsn', 'arf', 'webhook', 'k2')),
    at            INTEGER NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS email_notify_outcomes_once
    ON email_notify_outcomes (log_id, rcpt_sha256, kind);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS email_notify_outcomes_sender ON email_notify_outcomes (sender_id, at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS email_notify_outcomes_rcpt ON email_notify_outcomes (rcpt_sha256);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS email_suppressions (
    address_sha256 TEXT NOT NULL,
    domain         TEXT NOT NULL,
    scope          TEXT NOT NULL CHECK (scope IN ('box', 'sender', 'app')),
    scope_ref      TEXT NOT NULL DEFAULT '',
    reason         TEXT NOT NULL CHECK (reason IN ('hard_bounce', 'complaint', 'unsubscribe', 'manual')),
    created_at     INTEGER NOT NULL,
    created_by     TEXT NOT NULL,
    PRIMARY KEY (address_sha256, scope, scope_ref),
    CHECK ((scope = 'box') = (scope_ref = ''))
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS email_sender_pauses (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    sender_id   TEXT NOT NULL,
    paused_at   INTEGER NOT NULL,
    reason      TEXT NOT NULL CHECK (reason IN ('hard_bounces', 'complaints')),
    rate_json   TEXT NOT NULL,
    resumed_at  INTEGER,
    resumed_by  TEXT
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS email_sender_pauses_live
    ON email_sender_pauses (sender_id) WHERE resumed_at IS NULL;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS email_sender_pauses_sender ON email_sender_pauses (sender_id, paused_at);
