-- 0246_email_sender_envelope — Email Notifications part 2, slice N7
-- (prd-email-notifications-part2-v1 §4 N7, vs-live EN-A8, EN-A11; 0.46.6).
--
-- A send-only apex sender: `address` (noreply@brand.com) is the header
-- From K2 stamps and what Apps see; `envelope_address`
-- (noreply@bounce.brand.com) is the mail-server account behind it, its
-- Identity and the envelope MAIL FROM, on a bounce subdomain the box
-- hosts. The apex is never a mail-server domain (probe P4b, 2026-10-28).
-- NULL for every other sender (the address is its own envelope).
-- ADD COLUMN only: renumber freely on a slip (and edit the migration-order
-- tests). The store is `crate::email_notify`.
ALTER TABLE email_senders ADD COLUMN envelope_address TEXT;
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS email_senders_envelope ON email_senders (envelope_address) WHERE envelope_address IS NOT NULL;
