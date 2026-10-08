-- 0136: hosted-mail backup S8 B1 (prd-hostmail-backup-v1 §4, §6.1, §6.3).
--
-- B1 runs NO backup. It stores the per-box choice and the churn observer's
-- daily numbers so `k2 hostmail backup plan` can estimate from real data.
--
-- mail_server.backup_config_json — the chosen mode. NULL = unconfigured
--   (the default; the doctor reminds). Shape:
--   {"mode":"local|offsite-only|both|none","reason","keepDaily","keepWeekly",
--    "offsiteKeepDaily","offsiteKeepWeekly","offsiteKeepMonthly","window",
--    "maxLocalGb","configuredBy","configuredWorkspace","configuredAt",
--    "planHash","history":[{"at","by","workspace","from","to"}]}
-- mail_server.backup_state_json — the churn observer: last run / error and
--   up to 30 daily SUMMARIES (bytes created / deleted, store totals, free
--   space). Never a full inventory.
ALTER TABLE mail_server ADD COLUMN backup_config_json TEXT;
--> statement-breakpoint
ALTER TABLE mail_server ADD COLUMN backup_state_json TEXT;
--> statement-breakpoint
-- The LATEST inventory of Stalwart's immutable store files (`*.sst`,
-- `*.blob` under /var/lib/stalwart), replaced whole on each observation;
-- the next observation diffs against it. Its own table, not JSON in the
-- singleton row: a large store has tens of thousands of files, and this
-- keeps the mail_server row (read by every status call) small.
CREATE TABLE IF NOT EXISTS mail_backup_inventory (
    rel_path TEXT PRIMARY KEY NOT NULL,
    bytes    INTEGER NOT NULL,
    mtime    INTEGER NOT NULL
);
