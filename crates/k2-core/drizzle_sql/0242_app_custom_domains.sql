-- 0242_app_custom_domains — custom domains that actually serve an App
-- (prd-app-custom-domains-v1 §5; vs-live CD-A4/CD-A22; 0.46.6).
-- Additive columns on `domain_names` (0120). Only `app:<id>` rows use them;
-- every other role keeps them NULL / 0.
--   state          waiting_dns | verifying | issuing | live | failing | external
--   verify_token   per name + per box: `_k2-domain.<host> TXT "k2-verify=<token>"`
--   verified_at    last successful TXT + DNS proof (unix secs)
--   checked_at     last DNS check (unix secs)
--   path           direct | connect | NULL: what DNS showed at the last check
--   is_primary     the one name K2_APP_ORIGIN uses once it is served
--   last_error     plain words, shown in the CLI and Settings
--   failing_since  first check of the current run of failed re-checks (7-day rule)
-- Existing `app:` rows are GRANDFATHERED as `external`: K2 never routed them,
-- but a box may serve them through its own proxy to the App's front port
-- (PRD path 4), and the App has them in K2_APP_ORIGIN(S). They stay origins,
-- K2 never routes, checks or issues for them, and the owner moves one into
-- the App TLS door with `k2 app domain check`. Each App's alphabetically
-- first such name (on an attached apex) was its K2_APP_ORIGIN before 0.46.6,
-- so it is marked primary: the App's origin doesn't change at the upgrade.
-- Names bound after the upgrade start at `waiting_dns`.
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
   SET state = 'external', verify_token = lower(hex(randomblob(16)))
 WHERE role LIKE 'app:%' AND state IS NULL;
--> statement-breakpoint
UPDATE domain_names
   SET is_primary = 1
 WHERE state = 'external'
   AND hostname = (SELECT MIN(n2.hostname) FROM domain_names n2
                     JOIN domain_bindings b ON b.apex = n2.apex
                    WHERE n2.role = domain_names.role AND n2.state = 'external')
   AND NOT EXISTS (SELECT 1 FROM domain_names n3 WHERE n3.role = domain_names.role AND n3.is_primary = 1);
