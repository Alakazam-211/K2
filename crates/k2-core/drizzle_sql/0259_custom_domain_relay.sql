-- 0259_custom_domain_relay — custom domains through K2's relays, Path B
-- (prd-custom-domains-path-b-0467-v1 PB-3; vs-live PB-A15; 0.46.8).
-- Additive columns on `domain_names` (0120, 0242). Only `app:<id>` rows whose
-- DNS points at this box's k2.dev name (`path = 'connect'`) use them; every
-- other row keeps them NULL.
--   relay_state       claimed | verified | released | refused | NULL (never claimed):
--                     this box's claim on the name at K2 Connect's control plane,
--                     which decides relay routing (the box's own DNS check only
--                     informs the owner, PB-A15)
--   relay_error       the control plane's last refusal or release reason, plain words
--   relay_checked_at  last claim / verify / list answer (unix secs)
ALTER TABLE domain_names ADD COLUMN relay_state TEXT;
--> statement-breakpoint
ALTER TABLE domain_names ADD COLUMN relay_error TEXT;
--> statement-breakpoint
ALTER TABLE domain_names ADD COLUMN relay_checked_at INTEGER;
