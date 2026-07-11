-- K2 Mail — LINKED (external IMAP) inbox SEND support
-- (prd-email-server-v1 §17.5). Linked inboxes gain an OPT-IN send path
-- over SMTP submission, authenticated with the SAME vaulted app-password
-- already used for IMAP (`ext-inbox-<id>` — never a column here). The
-- SMTP server is normally DERIVED from the provider / IMAP host at send
-- time (gmail → smtp.gmail.com:587 STARTTLS, fastmail → smtp.fastmail.com
-- :465 implicit, generic imap.<domain> → smtp.<domain>:587 STARTTLS);
-- these NULLABLE columns are the OPTIONAL explicit override for hosts
-- that don't follow that convention. NULL = derive. `smtp_tls` is
-- 'implicit-tls' | 'starttls' (validated in the ops layer, mirroring the
-- IMAP `tls` column — no CHECK here because SQLite ADD COLUMN keeps it
-- simple; plaintext SMTP is never a thing K2 will speak). Additive;
-- runs once by migration name.
ALTER TABLE mail_external_inboxes ADD COLUMN smtp_host TEXT;
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN smtp_port INTEGER;
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN smtp_tls TEXT;
