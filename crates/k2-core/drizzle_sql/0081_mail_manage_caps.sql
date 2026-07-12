-- K2 Mail — INBOX MANAGEMENT + DELETE capabilities
-- (prd-email-server-v1 §17.5). Two per-workspace booleans, ORTHOGONAL to
-- the read < draft < send `level`:
--   * can_manage — move a message to another folder, create/rename
--     folders, mark read/unread, flag/unflag, archive.
--   * can_delete — delete a message = MOVE to Trash (never a permanent
--     EXPUNGE). can_delete REQUIRES can_manage (validated in the ops
--     layer; clearing manage clears delete).
--
-- Both are stored PER WORKSPACE and default OFF (opt-in): on
-- `mail_inbox_grants` for granted workspaces, and as `primary_can_manage`
-- / `primary_can_delete` on BOTH provisioning tables for the PRIMARY's
-- own caps (`mail_addresses` for hosted, `mail_external_inboxes` for
-- linked). Enforcement (`crate::mail::access::can_manage`/`can_delete`)
-- reads the primary's cols when project == owner, else the grant row's
-- cols — a no-access workspace still answers the SAME masked `not_found`.
--
-- Additive, idempotent ADD COLUMNs (NOT NULL DEFAULT 0 — SQLite fills the
-- default for every existing row); runs once by migration name.
ALTER TABLE mail_inbox_grants ADD COLUMN can_manage INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE mail_inbox_grants ADD COLUMN can_delete INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE mail_addresses ADD COLUMN primary_can_manage INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE mail_addresses ADD COLUMN primary_can_delete INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN primary_can_manage INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN primary_can_delete INTEGER NOT NULL DEFAULT 0;
