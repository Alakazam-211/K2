-- 0236_app_custom_domains — custom domains that actually serve an App
-- (prd-app-custom-domains-v1 §5; vs-live CD-A4/CD-A22; 0.46.6).
-- Additive columns on `domain_names` (0120). Only `app:<id>` rows use them;
-- every other role keeps them NULL / 0.
--   state          waiting_dns | verifying | issuing | live | failing
--   verify_token   per name + per box: `_k2-domain.<host> TXT "k2-verify=<token>"`
--   verified_at    last successful TXT + DNS proof (unix secs)
--   checked_at     last DNS check (unix secs)
--   path           direct | connect | NULL: what DNS showed at the last check
--   is_primary     the one name K2_APP_ORIGIN uses once it is live
--   last_error     plain words, shown in the CLI and Settings
--   failing_since  first check of the current run of failed re-checks (7-day rule)
-- Existing `app:` rows were never served by anything (pre-0.46.6 nothing
-- routed them), so they start at `waiting_dns` with a fresh token.
ALTER TABLE domain_names ADD COLUMN state TEXT;
--> statement-breakpoint
ALTER TABLE domain_names ADD COLUMN verify_token TEXT;
--> statement-breakpoint
ALTER TABLE domain_names ADD COLUMN verified_at INTEGER;
--> statement-breakpoint
ALTER TABLE domain_names ADD COLUMN checked_at INTEGER;
--> statement-breakpoint
ALTER TABLE domain_names ADD COLUMN path TEXT;
--> statement-breakpoint
ALTER TABLE domain_names ADD COLUMN is_primary INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE domain_names ADD COLUMN last_error TEXT;
--> statement-breakpoint
ALTER TABLE domain_names ADD COLUMN failing_since INTEGER;
--> statement-breakpoint
UPDATE domain_names
   SET state = 'waiting_dns', verify_token = lower(hex(randomblob(16)))
 WHERE role LIKE 'app:%' AND state IS NULL;
