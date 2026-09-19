-- Hostmail bans migrate window (prd-hostmail-bans-v1 B25):
-- snapshot of authBanRate + restoreAt for daemon auto-restore.
-- Additive ALTER; do not reuse enable_progress_json.
ALTER TABLE mail_server ADD COLUMN bans_migrate_json TEXT;
