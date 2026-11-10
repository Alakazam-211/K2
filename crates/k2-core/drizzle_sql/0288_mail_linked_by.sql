-- 0288: who linked each linked inbox (0.46.11, prd-ocmail-admin-in-k2-v1
-- §7.1, slice OA3). A linked inbox is someone's own Gmail, Microsoft or IMAP
-- account; this records whose, so the mail reader can show a Member their
-- own linked inboxes (and only those). Set when a login or the owner token
-- links the inbox: the person key K2 uses for that login ('owner' for the
-- owner token). NULL for rows linked before 0288 and for links an agent
-- made: such an inbox is still a person's mailbox, open only to the Owner
-- and Admins. Written by `k2-daemon` `mail::external::set_linked_by`.
ALTER TABLE mail_external_inboxes ADD COLUMN linked_by_principal TEXT;
