-- Per-workspace mailbox quota defaults for new mints (NULL = inherit
-- the global AppSettings.mail_quota_bytes / mail_quota_messages, which
-- themselves default to 1 GB / 10k). 0 = unlimited (pass through to
-- Stalwart maxDiskQuota / maxEmails). Existing addresses are not
-- retrofitted — raise those with POST /cli/mail/quota.
-- Additive ALTER; do not rewrite frozen 0075/0118.
ALTER TABLE projects ADD COLUMN mail_quota_bytes INTEGER;
--> statement-breakpoint
ALTER TABLE projects ADD COLUMN mail_quota_messages INTEGER;
