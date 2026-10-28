-- 0247_email_agent_notices — Email Notifications part 2, slice N10a
-- (prd-email-notifications-part2-v1 §4 N10, vs-live EN-A4–EN-A7; 0.46.6).
--
-- An agent emails a person on this box ("the nightly build failed") from
-- its workspace's own send-only sender, never the system sender and never
-- the agent's mailbox.
-- * `email_agent_senders` — the workspace's sender and its switch (default
--   off; the owner turns it on with a confirm, the agent may turn it off).
--   A sender held here is the workspace's: never an App's or the system's.
--   (A side table instead of EN-A4's `email_senders` rebuild: additive,
--   and the 0227 CHECKs stay as they are.)
-- * `email_notice_mutes` — a person turned one workspace's notices off.
--   (A side table instead of EN-A7's `notify_workspace_prefs` column, whose
--   rows also pin the push switch.)
-- * `email_notify_log.project_id` — the workspace an agent notice came
--   from (its own budget bucket).
-- Renumber freely on a slip (and edit the migration-order tests). The
-- store is `crate::email_notify`.
CREATE TABLE IF NOT EXISTS email_agent_senders (
    project_id  TEXT PRIMARY KEY NOT NULL,
    sender_id   TEXT NOT NULL UNIQUE,
    enabled     INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    granted_by  TEXT NOT NULL,
    granted_at  INTEGER NOT NULL,
    off_reason  TEXT,
    updated_at  INTEGER NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS email_notice_mutes (
    principal   TEXT NOT NULL,
    project_id  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (principal, project_id)
);
--> statement-breakpoint
ALTER TABLE email_notify_log ADD COLUMN project_id TEXT;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS email_notify_log_project ON email_notify_log (project_id, created_at);
