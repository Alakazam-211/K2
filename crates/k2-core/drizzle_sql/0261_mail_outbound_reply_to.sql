-- 0261_mail_outbound_reply_to — where answers to an outbound go (0.46.8).
-- `k2 mail send --reply-to`: a JSON array of addresses for the Reply-To header.
-- Kept on the audit row, like bcc_json (0131), because it steers replies away
-- from the From address. NULL = none. ADD COLUMN only.
ALTER TABLE mail_outbound ADD COLUMN reply_to_json TEXT;
