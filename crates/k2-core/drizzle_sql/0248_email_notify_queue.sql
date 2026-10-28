-- 0248_email_notify_queue — Email Notifications part 2, the mail
-- server's delivery webhook (prd-email-notifications-part2-v1 §4 N6,
-- vs-live EN-A25, EN-A27; 0.46.6).
--
-- `email_notify_queue` ties the mail server's queue id (the webhook's
-- `queueId`, decimal: a u64 that may not fit SQLite's signed INTEGER) to
-- the K2 send it carries (`email_notify_log.id`). K2 learns the pair once,
-- on the first event for a message, by reading the queued message's
-- envelope id (K2 submits with ENVID=<log id>); later events (`delivered`,
-- `delayed`) join on it. A message that already left the queue never gets
-- a row: its recipients stay `submitted` (K2 never guesses). Kept 7 days.
-- New table only: renumber freely on a slip (and edit the migration-order
-- tests). The store is `crate::email_outcomes`.
CREATE TABLE IF NOT EXISTS email_notify_queue (
    queue_id  TEXT PRIMARY KEY NOT NULL,
    log_id    TEXT NOT NULL,
    at        INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS email_notify_queue_at ON email_notify_queue (at);
