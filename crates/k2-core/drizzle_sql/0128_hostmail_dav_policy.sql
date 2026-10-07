-- Hosted-mail calendars S2 (prd-hostmail-calendars-v1 CAL28/CAL31):
-- the owner's DAV policy for every hosted address, as JSON
-- {"calendars":"on|off","files":"on|off","appliedAt":<unix>,
--  "backfilledAt":<unix>|null}. NULL = never applied (Stalwart's
-- default: CalDAV, CardDAV and WebDAV files all on). The row is
-- deleted on hostmail uninstall, so the policy resets with it.
-- Additive ALTER; do not reuse enable_progress_json / bans_migrate_json.
ALTER TABLE mail_server ADD COLUMN dav_policy_json TEXT;
