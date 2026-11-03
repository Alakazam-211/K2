-- 0261_mail_outbound_reply_to — where answers to an outbound go (0.46.8).
-- `k2 mail send --reply-to`: a JSON array of addresses for the Reply-To header.
-- Kept on the audit row, like bcc_json (0131), because it steers replies away
-- from the From address. NULL = none. ADD COLUMN only.
ALTER TABLE mail_outbound ADD COLUMN reply_to_json TEXT;
--> statement-breakpoint
-- The hex SHA-256 of the row's body file. A body that no longer matches is
-- neither shown nor sent. NULL on older rows.
ALTER TABLE mail_outbound ADD COLUMN body_sha256 TEXT;
--> statement-breakpoint
-- What a `k2 mail forward-message` forwarded, as JSON, for the outbox.
ALTER TABLE mail_outbound ADD COLUMN forward_of_json TEXT;
--> statement-breakpoint
-- A reply or forward whose original the sender check didn't clear: why.
ALTER TABLE mail_outbound ADD COLUMN source_check TEXT;
